//! Security-sensitive audit trail.
//!
//! Records who asked for what, which policy decision applied, and the outcome.
//! Secrets, prompts and tool output are never written here.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::ids::{SessionId, WorkspaceId};

/// One audited event.
#[derive(Debug, Serialize)]
pub(crate) struct AuditRecord {
    pub at: DateTime<Utc>,
    pub principal: String,
    pub action: &'static str,
    pub workspace_id: WorkspaceId,
    pub session_id: Option<SessionId>,
    pub decision: &'static str,
    pub detail: String,
}

impl AuditRecord {
    pub fn new(
        principal: impl Into<String>,
        action: &'static str,
        workspace_id: WorkspaceId,
        decision: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            at: Utc::now(),
            principal: principal.into(),
            action,
            workspace_id,
            session_id: None,
            decision,
            detail: detail.into(),
        }
    }

    pub fn session(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

/// Append-only audit file. Write failures are surfaced, never silently ignored
/// by the caller that asked for the audit.
pub(crate) struct AuditLog {
    path: std::path::PathBuf,
    file: Mutex<File>,
}

impl AuditLog {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        hostkit::fspriv::private_permissions(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    pub fn record(&self, record: &AuditRecord) -> std::io::Result<()> {
        let line = serde_json::to_vec(record)
            .map_err(|error| std::io::Error::other(format!("audit serialize: {error}")))?;
        let mut file = self.file.lock().expect("audit lock");
        file.write_all(&line)?;
        file.write_all(b"\n")?;
        file.sync_data()
    }

    /// Best-effort variant for paths where the operation already failed and
    /// the audit is diagnostic rather than part of the decision.
    pub fn try_record(&self, record: &AuditRecord) {
        if let Err(error) = self.record(record) {
            log::warn!(
                "audit record could not be written to {}: {error}",
                self.path.display()
            );
        }
    }

    /// Path for diagnostics.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A fresh correlation id for a request that lacks one.
pub(crate) fn new_correlation() -> String {
    Uuid::new_v4().to_string()
}
