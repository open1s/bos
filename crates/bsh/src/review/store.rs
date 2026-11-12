//! Content-addressed snapshot storage and the per-turn capture runtime.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::domain::{
    hex_lower, local_actor, new_id, now_ms, review_root, ChangeOperation, ChangeSet,
    ChangeSetStatus, Decision, ResourceFamily, ResourcePath, Review, ReviewSnapshot, SnapshotRef,
    SnapshotSource, SnapshotState, ToolRef, UnavailableReason, MAX_BINARY_BYTES, MAX_TEXT_BYTES,
};

/// A workspace-scoped review store.
#[derive(Debug, Clone)]
pub(crate) struct ReviewStore {
    root: PathBuf,
    workspace: PathBuf,
}

impl ReviewStore {
    /// Open the store for a workspace. No I/O happens until a write or read.
    pub(crate) fn open(workspace: &Path) -> Self {
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        Self {
            root: review_root(&workspace),
            workspace,
        }
    }

    /// Create the directory skeleton.
    pub(crate) fn ensure(&self) -> io::Result<()> {
        fs::create_dir_all(self.root.join("objects"))?;
        fs::create_dir_all(self.root.join("changesets"))?;
        fs::create_dir_all(self.root.join("reviews"))?;
        Ok(())
    }

    /// Read one file as a snapshot image.
    pub(crate) fn capture(&self, path: &Path, source: SnapshotSource) -> SnapshotState {
        let path = &canonical_target(path);
        if path.strip_prefix(&self.workspace).is_err() {
            return SnapshotState::Unavailable(UnavailableReason::OutsideWorkspace);
        }
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return SnapshotState::Absent;
            }
            Err(error) => {
                return SnapshotState::Unavailable(UnavailableReason::ReadFailed(
                    error.to_string(),
                ));
            }
        };
        if !metadata.is_file() {
            return SnapshotState::Unavailable(UnavailableReason::ReadFailed(
                "not a regular file".to_string(),
            ));
        }
        let family = family_for(path, &[]);
        let limit = if family.is_text() {
            MAX_TEXT_BYTES
        } else {
            MAX_BINARY_BYTES
        };
        if metadata.len() > limit {
            return SnapshotState::Unavailable(UnavailableReason::TooLarge {
                bytes: metadata.len(),
                limit,
            });
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return SnapshotState::Unavailable(UnavailableReason::ReadFailed(
                    error.to_string(),
                ));
            }
        };
        let family = family_for(path, &bytes);
        let limit = if family.is_text() {
            MAX_TEXT_BYTES
        } else {
            MAX_BINARY_BYTES
        };
        if bytes.len() as u64 > limit {
            return SnapshotState::Unavailable(UnavailableReason::TooLarge {
                bytes: bytes.len() as u64,
                limit,
            });
        }
        let mode = mode_of(&metadata);
        match self.put_blob(&bytes, family, mode, source) {
            Ok(snapshot) => SnapshotState::Present(snapshot),
            Err(error) => {
                SnapshotState::Unavailable(UnavailableReason::ReadFailed(error.to_string()))
            }
        }
    }

    /// Store one content-addressed image.
    pub(crate) fn put_blob(
        &self,
        bytes: &[u8],
        family: ResourceFamily,
        mode: u32,
        source: SnapshotSource,
    ) -> io::Result<SnapshotRef> {
        self.ensure()?;
        let hash = hash_bytes(bytes);
        let hex = hash.strip_prefix("sha256:").unwrap_or(&hash);
        let dir = self.root.join("objects").join(&hex[..2]);
        fs::create_dir_all(&dir)?;
        let target = dir.join(&hex[2..]);
        if !target.exists() {
            let temporary = dir.join(format!(".tmp-{}", new_id()));
            fs::write(&temporary, bytes)?;
            match fs::rename(&temporary, &target) {
                Ok(()) => {}
                Err(error) if target.exists() => {
                    let _ = fs::remove_file(&temporary);
                    let _ = error;
                }
                Err(error) => {
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
            }
        }
        Ok(SnapshotRef {
            hash,
            bytes: bytes.len() as u64,
            media_type: media_type(family).to_string(),
            family,
            mode,
            source,
        })
    }

    /// Read a blob and verify its hash.
    pub(crate) fn read_blob(&self, hash: &str) -> io::Result<Vec<u8>> {
        let hex = hash.strip_prefix("sha256:").unwrap_or(hash);
        if hex.len() < 3 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad hash"));
        }
        let path = self.root.join("objects").join(&hex[..2]).join(&hex[2..]);
        let bytes = fs::read(&path)?;
        if hash_bytes(&bytes) != hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot hash mismatch",
            ));
        }
        Ok(bytes)
    }

    /// Persist one changeset manifest.
    pub(crate) fn save_changeset(&self, set: &ChangeSet) -> io::Result<()> {
        self.ensure()?;
        let path = self
            .root
            .join("changesets")
            .join(format!("{}.json", set.id));
        write_atomic(&path, &serde_json::to_vec_pretty(set)?)
    }

    /// Load every changeset for one session, oldest first.
    pub(crate) fn load_changesets(&self, session: &str) -> Vec<ChangeSet> {
        let dir = self.root.join("changesets");
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut sets: Vec<ChangeSet> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    return None;
                }
                let bytes = fs::read(&path).ok()?;
                let set: ChangeSet = serde_json::from_slice(&bytes).ok()?;
                (set.session == session).then_some(set)
            })
            .collect();
        sets.sort_by_key(|set| set.created_at_ms);
        sets
    }

    /// Load the newest changeset for one session.
    pub(crate) fn load_latest(&self, session: &str) -> Option<ChangeSet> {
        self.load_changesets(session).into_iter().last()
    }

    /// Load one changeset by id.
    pub(crate) fn load_changeset(&self, id: &str) -> Option<ChangeSet> {
        let path = self.root.join("changesets").join(format!("{id}.json"));
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Persist one review.
    pub(crate) fn save_review(&self, review: &Review) -> io::Result<()> {
        self.ensure()?;
        let path = self
            .root
            .join("reviews")
            .join(format!("{}.json", review.change_set_id));
        write_atomic(&path, &serde_json::to_vec_pretty(review)?)
    }

    /// Load the review for a changeset.
    pub(crate) fn load_review(&self, change_set_id: &str) -> Option<Review> {
        let path = self
            .root
            .join("reviews")
            .join(format!("{change_set_id}.json"));
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Load or create the review for a sealed changeset.
    pub(crate) fn review_for(&self, set: &ChangeSet) -> io::Result<Review> {
        if let Some(mut review) = self.load_review(&set.id) {
            if let Some(digest) = &set.digest {
                review.refresh(digest);
            }
            return Ok(review);
        }
        let review = Review::open(set);
        self.save_review(&review)?;
        Ok(review)
    }

    /// Load the newest snapshot for a session.
    pub(crate) fn load_snapshot(&self, session: &str) -> Option<ReviewSnapshot> {
        let changeset = self.load_latest(session)?;
        if matches!(changeset.status, ChangeSetStatus::Open) {
            return None;
        }
        let review = self.review_for(&changeset).ok()?;
        let mut review = review;
        if let Some(digest) = &changeset.digest {
            review.refresh(digest);
        }
        Some(ReviewSnapshot { changeset, review })
    }

    /// Attach a task id and attempt id to a sealed changeset.
    pub(crate) fn link_task(
        &self,
        change_set_id: &str,
        task: &str,
        attempt: &str,
    ) -> io::Result<()> {
        let Some(mut set) = self.load_changeset(change_set_id) else {
            return Ok(());
        };
        set.task = Some(task.to_string());
        set.attempt = Some(attempt.to_string());
        self.save_changeset(&set)
    }

    /// Load the snapshot for one changeset id.
    pub(crate) fn load_snapshot_for(&self, change_set_id: &str) -> Option<ReviewSnapshot> {
        let changeset = self.load_changeset(change_set_id)?;
        if matches!(changeset.status, ChangeSetStatus::Open) {
            return None;
        }
        let mut review = self.review_for(&changeset).ok()?;
        if let Some(digest) = &changeset.digest {
            review.refresh(digest);
        }
        Some(ReviewSnapshot { changeset, review })
    }

    /// Record a decision and append an audit event.
    pub(crate) fn decide(
        &self,
        change_set_id: &str,
        decision: Decision,
        rationale: String,
    ) -> Result<Review, String> {
        let set = self
            .load_changeset(change_set_id)
            .ok_or_else(|| "the change set no longer exists".to_string())?;
        let mut review = self
            .load_review(change_set_id)
            .ok_or_else(|| "the review no longer exists".to_string())?;
        let digest = set.digest.clone().unwrap_or_default();
        review.decide(decision, local_actor(), rationale.clone(), &digest)?;
        self.save_review(&review)
            .map_err(|error| error.to_string())?;
        let decision = match decision {
            Decision::Approve => "approve",
            Decision::Reject => "reject",
            Decision::RequestChanges => "request_changes",
        };
        let event = serde_json::json!({
            "at_ms": now_ms(),
            "session": set.session,
            "change_set": change_set_id,
            "review": review.id,
            "decision": decision,
            "actor": local_actor(),
            "digest": digest,
            "rationale": rationale,
        });
        let _ = self.append_audit(&event);
        Ok(review)
    }

    fn append_audit(&self, event: &serde_json::Value) -> io::Result<()> {
        use std::io::Write as _;
        self.ensure()?;
        let path = self.root.join("audit.jsonl");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{event}")?;
        Ok(())
    }
}

/// The per-turn capture runtime shared by the file tools.
#[derive(Debug)]
pub(crate) struct ReviewRuntime {
    store: ReviewStore,
    workspace: PathBuf,
    inner: Mutex<RuntimeInner>,
}

#[derive(Debug)]
struct RuntimeInner {
    set: ChangeSet,
    sealed: bool,
}

impl ReviewRuntime {
    /// Open a runtime for one turn.
    pub(crate) fn new(workspace: &Path, session: &str) -> io::Result<Arc<Self>> {
        let store = ReviewStore::open(workspace);
        store.ensure()?;
        let workspace = store.workspace.clone();
        Ok(Arc::new(Self {
            store,
            workspace,
            inner: Mutex::new(RuntimeInner {
                set: ChangeSet::new(session, None),
                sealed: false,
            }),
        }))
    }

    /// The changeset id, for task and audit links.
    pub(crate) fn change_set_id(&self) -> String {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set
            .id
            .clone()
    }

    /// Capture before a mutation.
    pub(crate) fn before(&self, path: &Path) -> SnapshotState {
        self.store.capture(path, SnapshotSource::ToolBefore)
    }

    /// Capture after a mutation and append the revision.
    pub(crate) fn record(
        &self,
        path: &Path,
        operation: ChangeOperation,
        before: SnapshotState,
        tool: &str,
    ) {
        let path = canonical_target(path);
        let path = path.as_path();
        let after = self.store.capture(path, SnapshotSource::ToolAfter);
        if SnapshotState::same_content(&before, &after) {
            return;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.sealed {
            return;
        }
        let relative = ResourcePath::new(&self.workspace, path);
        if inner.set.add_revision(
            relative,
            operation,
            before,
            after,
            Some(ToolRef::named(tool)),
        ) {
            if let Err(error) = self.store.save_changeset(&inner.set) {
                log::warn!("review manifest could not be saved: {error}");
            }
        }
    }

    /// Whether this turn wrote anything.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set
            .changes
            .is_empty()
    }

    /// Seal the changeset and create its review.
    pub(crate) fn seal(&self) -> Option<ReviewSnapshot> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.sealed {
            return None;
        }
        inner.sealed = true;
        if inner.set.changes.is_empty() {
            inner.set.status = ChangeSetStatus::Sealed;
            inner.set.sealed_at_ms = Some(now_ms());
            return None;
        }
        let _ = inner.set.seal();
        if let Err(error) = self.store.save_changeset(&inner.set) {
            log::warn!("review manifest could not be sealed: {error}");
            return None;
        }
        match self.store.review_for(&inner.set) {
            Ok(review) => Some(ReviewSnapshot {
                changeset: inner.set.clone(),
                review,
            }),
            Err(error) => {
                log::warn!("review could not be created: {error}");
                None
            }
        }
    }
}

/// Read a snapshot image as text, when it is text.
pub(crate) fn snapshot_text(store: &ReviewStore, state: &SnapshotState) -> Option<String> {
    let SnapshotState::Present(snapshot) = state else {
        return None;
    };
    if !snapshot.family.is_text() {
        return None;
    }
    let bytes = store.read_blob(&snapshot.hash).ok()?;
    String::from_utf8(bytes).ok()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", new_id()));
    fs::write(&temporary, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn canonical_target(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut cursor = path.to_path_buf();
    loop {
        if let Ok(real) = cursor.canonicalize() {
            let mut out = real;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let Some(name) = cursor.file_name().map(|name| name.to_os_string()) else {
            return path.to_path_buf();
        };
        tail.push(name);
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        cursor = parent.to_path_buf();
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex_lower(hasher.finalize().as_slice()))
}

fn mode_of(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0o644
    }
}

fn family_for(path: &Path, bytes: &[u8]) -> ResourceFamily {
    let ext = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "php"
            | "cs"
            | "sh"
            | "bash"
            | "zsh"
            | "html"
            | "css"
            | "scss"
            | "sql"
            | "lua"
            | "zig"
    ) {
        return ResourceFamily::Code;
    }
    if matches!(
        ext.as_str(),
        "toml" | "yaml" | "yml" | "json" | "ini" | "conf" | "env"
    ) {
        return ResourceFamily::Config;
    }
    if matches!(ext.as_str(), "md" | "markdown" | "mdx") {
        return ResourceFamily::Markdown;
    }
    if matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff"
    ) {
        return ResourceFamily::Image;
    }
    if ext == "pdf" {
        return ResourceFamily::Pdf;
    }
    if matches!(ext.as_str(), "doc" | "docx" | "rtf" | "odt") {
        return ResourceFamily::Document;
    }
    if matches!(ext.as_str(), "xls" | "xlsx" | "ods" | "csv") {
        return ResourceFamily::Spreadsheet;
    }
    if matches!(
        ext.as_str(),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar"
    ) {
        return ResourceFamily::Archive;
    }
    if bytes.is_empty() || std::str::from_utf8(bytes).is_ok() {
        return ResourceFamily::Text;
    }
    ResourceFamily::Binary
}

fn media_type(family: ResourceFamily) -> &'static str {
    match family {
        ResourceFamily::Text => "text/plain",
        ResourceFamily::Code => "text/plain",
        ResourceFamily::Config => "application/json",
        ResourceFamily::Markdown => "text/markdown",
        ResourceFamily::Image => "image/*",
        ResourceFamily::Pdf => "application/pdf",
        ResourceFamily::Document => "application/octet-stream",
        ResourceFamily::Spreadsheet => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        ResourceFamily::Archive => "application/zip",
        ResourceFamily::Binary => "application/octet-stream",
        ResourceFamily::Unknown => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::domain::CaptureMode;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("bsh-review-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn exact_capture_records_before_and_after() {
        let root = scratch("exact");
        let path = root.join("src").join("lib.rs");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "fn old() {}\n").unwrap();
        let runtime = ReviewRuntime::new(&root, "session").unwrap();
        let before = runtime.before(&path);
        fs::write(&path, "fn new() {}\n").unwrap();
        runtime.record(&path, ChangeOperation::Write, before, "write_file");
        let snapshot = runtime.seal().expect("a sealed changeset");
        assert_eq!(snapshot.changeset.changes.len(), 1);
        assert_eq!(snapshot.changeset.capture, CaptureMode::Exact);
        assert!(snapshot.changeset.digest.is_some());
        assert_eq!(snapshot.review.change_set_id, snapshot.changeset.id);
        assert!(!snapshot.review.findings.is_empty());
    }

    #[test]
    fn unchanged_write_is_not_a_change() {
        let root = scratch("unchanged");
        let path = root.join("a.txt");
        fs::write(&path, "same").unwrap();
        let runtime = ReviewRuntime::new(&root, "session").unwrap();
        let before = runtime.before(&path);
        runtime.record(&path, ChangeOperation::Write, before, "write_file");
        assert!(runtime.is_empty());
    }

    #[test]
    fn blob_corruption_is_detected() {
        let root = scratch("corrupt");
        let store = ReviewStore::open(&root);
        let snapshot = store
            .put_blob(
                b"hello",
                ResourceFamily::Text,
                0o644,
                SnapshotSource::ToolBefore,
            )
            .unwrap();
        let hex = snapshot.hash.strip_prefix("sha256:").unwrap();
        let path = store.root.join("objects").join(&hex[..2]).join(&hex[2..]);
        fs::write(path, b"tampered").unwrap();
        assert!(store.read_blob(&snapshot.hash).is_err());
    }

    #[test]
    fn a_moved_digest_refuses_the_old_decision() {
        let root = scratch("stale");
        let path = root.join("a.txt");
        fs::write(&path, "one").unwrap();
        let runtime = ReviewRuntime::new(&root, "session").unwrap();
        let before = runtime.before(&path);
        fs::write(&path, "two").unwrap();
        runtime.record(&path, ChangeOperation::Write, before, "write_file");
        let snapshot = runtime.seal().expect("sealed");
        let store = ReviewStore::open(&root);
        let mut set = store
            .load_changeset(&snapshot.changeset.id)
            .expect("the manifest");
        set.digest = Some("sha256:moved".to_string());
        store.save_changeset(&set).expect("the moved manifest");
        let refused = store.decide(
            &snapshot.changeset.id,
            Decision::Approve,
            "stale approval".into(),
        );
        assert!(refused.is_err(), "a decision cannot outlive its digest");
    }

    #[test]
    fn decision_is_bound_to_the_digest() {
        let root = scratch("decision");
        let path = root.join("a.txt");
        fs::write(&path, "one").unwrap();
        let runtime = ReviewRuntime::new(&root, "session").unwrap();
        let before = runtime.before(&path);
        fs::write(&path, "two").unwrap();
        runtime.record(&path, ChangeOperation::Write, before, "write_file");
        let snapshot = runtime.seal().expect("sealed");
        let decided = ReviewStore::open(&root)
            .decide(
                &snapshot.changeset.id,
                Decision::Approve,
                "looks right".into(),
            )
            .expect("decided");
        assert_eq!(
            decided.status,
            crate::review::domain::ReviewStatus::Approved
        );
        assert_eq!(decided.decisions.len(), 1);
    }
}
