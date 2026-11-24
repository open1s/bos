//! The review read model: the desktop Review tab, answered over the contract.
//!
//! The Review tab is not a second store. A turn captures its changes at the tool
//! boundary through [`crate::review::store::ReviewRuntime`], the store persists
//! them under the workspace, and the desktop *reads* that store into the panel
//! it draws. The terminal reads the same store through the same reader, so the
//! two interfaces cannot disagree about which files a turn touched, what the
//! deterministic risk said or how a diff looked.
//!
//! The conversion lives here rather than in either interface: the store speaks
//! its own enums and the contract speaks strings, and exactly one place should
//! decide how one becomes the other.

use std::path::Path;

use crate::review::diff;
use crate::review::domain::{
    self, Change, ChangeKind, ChangeSetStatus, Decision, FileDiff, Finding, Review, ReviewDecision,
    Verification,
};
use crate::review::store::ReviewStore;

use super::contract::{
    ReviewChangeView, ReviewDecisionView, ReviewDiffLineView, ReviewDiffView, ReviewFindingView,
    ReviewNodeView, ReviewReport, ReviewVerificationView,
};

/// Read one session's latest sealed review for the contract.
///
/// `change` names the change whose prepared diff the report should carry. It is
/// either the change id the list printed, or the path a reader typed - a
/// terminal asks `/review src/app.rs` rather than pasting an opaque id - and
/// the two are resolved against the same changeset, so a diff is never built
/// from a change another turn owns.
pub(crate) fn report(root: &Path, session: &str, change: Option<&str>) -> ReviewReport {
    let store = ReviewStore::open(root);
    let changesets = match store.load_changesets_result(session) {
        Ok(changesets) => changesets,
        Err(error) => {
            return ReviewReport {
                state: "unreadable".to_string(),
                error: Some(error.to_string()),
                session: session.to_string(),
                ..ReviewReport::default()
            };
        }
    };
    let Some(set) = changesets.into_iter().last() else {
        return ReviewReport {
            state: "none".to_string(),
            session: session.to_string(),
            ..ReviewReport::default()
        };
    };
    // An open manifest is a turn still writing: the desktop's Review tab draws
    // nothing until that turn seals, because half a changeset read as a whole
    // one would understate what the turn touched.
    if matches!(set.status, ChangeSetStatus::Open) {
        return ReviewReport {
            state: "none".to_string(),
            error: Some(
                "the turn that changed these files has not sealed its changeset yet".to_string(),
            ),
            session: session.to_string(),
            ..ReviewReport::default()
        };
    }
    // A read must not write: the desktop's tab creates the review record when it
    // opens, but a query may be answered by a client that only lists. A missing
    // record is therefore the review the store would open, kept in memory.
    let mut review = store
        .load_review(&set.id)
        .unwrap_or_else(|| Review::open(&set));
    if let Some(digest) = &set.digest {
        review.refresh(digest);
    }
    let mut report = ReviewReport {
        state: "ready".to_string(),
        error: None,
        session: session.to_string(),
        change_set: Some(set.id.clone()),
        turn: set.turn.clone(),
        task: set.task.clone(),
        capture: string_of(&set.capture),
        changeset_status: string_of(&set.status),
        digest: set.digest.clone(),
        status: string_of(&review.status),
        risk: string_of(&review.risk.level),
        requires_human: review.risk.requires_human,
        findings: review.findings.iter().map(finding_view).collect(),
        verifications: review.verifications.iter().map(verification_view).collect(),
        decisions: review.decisions.iter().map(decision_view).collect(),
        changes: set.changes.iter().map(change_view).collect(),
        diff: None,
    };
    if let Some(named) = change.map(str::trim).filter(|named| !named.is_empty()) {
        match set
            .changes
            .iter()
            .find(|change| change.id == named || change.path.workspace_relative == named)
        {
            Some(found) => report.diff = Some(diff_view(&diff::build(&store, found))),
            None => {
                report.error = Some(format!(
                    "no change is named '{named}' in this changeset; /review lists {} change(s)",
                    report.changes.len()
                ));
            }
        }
    }
    report
}

/// The contract's word for one of the store's label enums.
///
/// Every label the contract names is the store's own serde name, so the words
/// a reader sees are derived from the stored record rather than retyped here.
fn string_of<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => String::new(),
    }
}

fn change_view(change: &Change) -> ReviewChangeView {
    let mut tools: Vec<String> = Vec::new();
    for revision in &change.revisions {
        if let Some(tool) = &revision.tool
            && !tools.contains(&tool.name)
        {
            tools.push(tool.name.clone());
        }
    }
    ReviewChangeView {
        id: change.id.clone(),
        path: change.path.workspace_relative.clone(),
        kind: domain::kind_name(&change.kind).to_string(),
        from: match &change.kind {
            ChangeKind::Renamed { from } => Some(from.workspace_relative.clone()),
            _ => None,
        },
        tools,
        revisions: change.revisions.len(),
        partial: change.revisions.iter().any(|revision| {
            revision.before.reason().is_some() || revision.after.reason().is_some()
        }),
    }
}

fn finding_view(finding: &Finding) -> ReviewFindingView {
    ReviewFindingView {
        change: finding.change_id.clone(),
        severity: string_of(&finding.severity),
        kind: string_of(&finding.kind),
        status: string_of(&finding.status),
        title: finding.title.clone(),
        detail: finding.detail.clone(),
    }
}

fn verification_view(verification: &Verification) -> ReviewVerificationView {
    ReviewVerificationView {
        kind: verification.kind.clone(),
        status: verification.status.clone(),
        command: verification.command.clone(),
        exit_code: verification.exit_code,
        summary: verification.summary.clone(),
    }
}

fn decision_view(decision: &ReviewDecision) -> ReviewDecisionView {
    ReviewDecisionView {
        decision: match decision.decision {
            Decision::Approve => "approve",
            Decision::Reject => "reject",
            Decision::RequestChanges => "request_changes",
        }
        .to_string(),
        actor: decision.actor.clone(),
        rationale: decision.rationale.clone(),
        digest: decision.digest.clone(),
        at_ms: decision.at_ms,
    }
}

fn diff_view(diff: &FileDiff) -> ReviewDiffView {
    ReviewDiffView {
        path: diff.path.clone(),
        kind: diff.kind.clone(),
        binary: diff.binary,
        added: diff.added,
        removed: diff.removed,
        old_bytes: diff.old_bytes,
        new_bytes: diff.new_bytes,
        structural: diff
            .structural
            .iter()
            .map(|node| ReviewNodeView {
                id: node.id.clone(),
                parent: node.parent.clone(),
                kind: node.kind.clone(),
                name: node.qualified_name.clone(),
                change: node.change.clone(),
                old_line: node.old_line,
                new_line: node.new_line,
                detail: node.detail.clone(),
            })
            .collect(),
        lines: diff
            .lines
            .iter()
            .map(|line| ReviewDiffLineView {
                mark: line.mark.clone(),
                old_line: line.old_line,
                new_line: line.new_line,
                text: line.text.clone(),
            })
            .collect(),
        note: diff.note.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::review::domain::ChangeOperation;
    use crate::review::store::ReviewRuntime;

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("bsh-runtime-review-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// One turn that rewrites a file, captured and sealed exactly the way the
    /// executor's turn does it.
    fn sealed_turn(root: &Path, session: &str, path: &Path) -> ReviewReport {
        let runtime = ReviewRuntime::new(root, session).unwrap();
        let before = runtime.before(path);
        fs::write(path, "fn new() {}\n").unwrap();
        runtime.record(path, ChangeOperation::Write, before, "write_file");
        runtime.seal().expect("a sealed changeset");
        report(root, session, None)
    }

    #[test]
    fn a_sealed_turn_reads_as_the_change_list() {
        let root = scratch("list");
        let path = root.join("src").join("lib.rs");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "fn old() {}\n").unwrap();
        let report = sealed_turn(&root, "session-a", &path);
        assert_eq!(report.state, "ready");
        assert_eq!(report.changes.len(), 1);
        let change = &report.changes[0];
        assert_eq!(change.path, "src/lib.rs");
        assert_eq!(change.kind, "modified");
        assert_eq!(change.revisions, 1);
        assert_eq!(change.tools, vec!["write_file".to_string()]);
        assert!(!change.partial, "a tool-boundary capture is exact");
        assert_eq!(report.capture, "exact");
        assert_eq!(report.changeset_status, "sealed");
        assert!(report.digest.is_some());
        assert_eq!(
            report.risk, "low",
            "one exact change whose only finding is a missing verification reads low"
        );
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.kind.contains("verification")
                    || finding.title.contains("verif")),
            "the missing verification is the finding that was read: {:?}",
            report.findings
        );
    }

    #[test]
    fn naming_a_change_carries_its_prepared_diff() {
        let root = scratch("diff");
        let path = root.join("src").join("lib.rs");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "fn old() {}\n").unwrap();
        let report = sealed_turn(&root, "session-b", &path);
        let id = report.changes[0].id.clone();
        let diff = super::report(&root, "session-b", Some(&id))
            .diff
            .expect("a prepared diff");
        assert_eq!(diff.path, "src/lib.rs");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        let old = diff
            .lines
            .iter()
            .find(|line| line.mark == "-")
            .expect("the removed row");
        assert_eq!(old.text, "fn old() {}");
        let new = diff
            .lines
            .iter()
            .find(|line| line.mark == "+")
            .expect("the added row");
        assert_eq!(new.text, "fn new() {}");
        // A path is as good an address as the id, because that is what a reader
        // types after `/review`.
        let by_path = super::report(&root, "session-b", Some("src/lib.rs"));
        assert!(by_path.diff.is_some());
        assert!(by_path.error.is_none());
        // Naming a change the changeset does not hold is refused with the list.
        let missing = super::report(&root, "session-b", Some("nope.rs"));
        assert!(missing.diff.is_none());
        assert!(
            missing
                .error
                .as_deref()
                .is_some_and(|error| error.contains("nope.rs"))
        );
    }

    #[test]
    fn no_changeset_is_none_and_an_open_turn_is_not_read() {
        let root = scratch("none");
        let report = super::report(&root, "session-c", None);
        assert_eq!(report.state, "none");
        assert!(report.changes.is_empty());
        // A turn still holding its changeset open must not be read as "nothing
        // changed" - the desktop waits for the seal, and so does this reader.
        let path = root.join("a.txt");
        fs::write(&path, "one").unwrap();
        let runtime = ReviewRuntime::new(&root, "session-c").unwrap();
        let before = runtime.before(&path);
        fs::write(&path, "two").unwrap();
        runtime.record(&path, ChangeOperation::Write, before, "write_file");
        let open = super::report(&root, "session-c", None);
        assert_eq!(open.state, "none");
        assert!(
            open.error
                .as_deref()
                .is_some_and(|error| error.contains("seal"))
        );
        // Sealing the same turn makes it readable, without a second capture.
        assert!(runtime.seal().is_some());
        let sealed = super::report(&root, "session-c", None);
        assert_eq!(sealed.state, "ready");
        assert_eq!(sealed.changes.len(), 1);
        assert!(sealed.digest.is_some());
    }

    #[test]
    fn a_corrupt_manifest_is_unreadable_rather_than_empty() {
        let root = scratch("corrupt");
        let store = ReviewStore::open(&root);
        store.ensure().unwrap();
        let changesets = crate::review::domain::review_root(&root).join("changesets");
        fs::write(changesets.join("broken.json"), b"{not json").unwrap();
        let report = super::report(&root, "session-d", None);
        assert_eq!(report.state, "unreadable");
        assert!(
            report
                .error
                .as_deref()
                .is_some_and(|error| error.contains("broken.json")),
            "the refusing record is named: {:?}",
            report.error
        );
    }
}
