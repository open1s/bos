//! Endpoint discovery record (RFC-015).
//!
//! A client finds the Host through `<data_dir>/runtime.json`, then validates
//! the Host's identity against `Welcome.instance_id` before trusting the
//! connection. The record is a hint, never an authority: the socket itself is
//! owner-only and the Host re-derives the caller's identity from peer
//! credentials.

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runtime::contract::PROTOCOL_VERSION;
use crate::runtime::ids::{HostInstanceId, WorkspaceId};

/// What a running Host publishes for discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    /// Identity the Host also returns in `Welcome`.
    pub instance_id: HostInstanceId,
    /// Workspace this Host serves.
    pub workspace_id: WorkspaceId,
    /// Local IPC socket path.
    pub socket_path: PathBuf,
    /// Host process id, for diagnostics only. Never a liveness proof.
    pub pid: u32,
    /// Protocol version the Host speaks.
    pub protocol_version: u16,
    /// When the Host started.
    pub started_at: DateTime<Utc>,
}

/// Discovery failures.
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    /// The record is missing: the Host is not running.
    #[error("no Runtime Host at {path}; start `bsh host` first")]
    Missing {
        /// Record path.
        path: String,
    },
    /// The record cannot be read or written.
    #[error("runtime endpoint {path}: {source}")]
    Io {
        /// Record path.
        path: String,
        /// OS error.
        #[source]
        source: io::Error,
    },
    /// The record names another protocol dialect.
    #[error("runtime endpoint {path} speaks protocol {found}, this client speaks {expected}")]
    Protocol {
        /// Record path.
        path: String,
        /// Version in the record.
        found: u16,
        /// Version this client speaks.
        expected: u16,
    },
    /// The record is not valid JSON.
    #[error("runtime endpoint {path} is malformed: {detail}")]
    Decode {
        /// Record path.
        path: String,
        /// Parse failure.
        detail: String,
    },
}

impl Endpoint {
    /// Atomically publish the record.
    pub fn write(&self, path: &Path) -> Result<(), EndpointError> {
        let text = serde_json::to_vec_pretty(self).map_err(|error| EndpointError::Decode {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
        // Owner-only: the record is not a secret, but the directory and every
        // file a Host writes stay private to the Host user.
        hostkit::fspriv::write_private(path, &text).map_err(|source| EndpointError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    /// Read and validate a record.
    pub fn read(path: &Path) -> Result<Self, EndpointError> {
        let text = match std::fs::read(path) {
            Ok(bytes) => String::from_utf8(bytes).map_err(|error| EndpointError::Decode {
                path: path.display().to_string(),
                detail: error.to_string(),
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(EndpointError::Missing {
                    path: path.display().to_string(),
                });
            }
            Err(source) => {
                return Err(EndpointError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        let endpoint: Endpoint =
            serde_json::from_str(&text).map_err(|error| EndpointError::Decode {
                path: path.display().to_string(),
                detail: error.to_string(),
            })?;
        if endpoint.protocol_version != PROTOCOL_VERSION {
            return Err(EndpointError::Protocol {
                path: path.display().to_string(),
                found: endpoint.protocol_version,
                expected: PROTOCOL_VERSION,
            });
        }
        Ok(endpoint)
    }

    /// Read the record a Host at `data_dir` publishes.
    pub fn discover(data_dir: &Path) -> Result<Self, EndpointError> {
        Self::read(&data_dir.join("runtime.json"))
    }

    /// Remove the record if it still describes this instance.
    pub fn remove(&self, path: &Path) {
        // Never delete a record a newer Host has published: the process that
        // owns the lock may have replaced ours after a crash.
        if let Ok(current) = Self::read(path)
            && current.instance_id != self.instance_id
        {
            return;
        }
        let _ = std::fs::remove_file(path);
    }
}

// A Host always runs under the user's default umask for these paths; on Unix
// the socket and record inherit the data directory's 0700 mode.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn record(dir: &Path) -> Endpoint {
        Endpoint {
            instance_id: HostInstanceId::new(),
            workspace_id: WorkspaceId::derived("/tmp/workspace"),
            socket_path: dir.join("host.sock"),
            pid: 42,
            protocol_version: PROTOCOL_VERSION,
            started_at: Utc::now(),
        }
    }

    #[test]
    fn a_record_round_trips_and_reports_a_foreign_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        let written = record(dir.path());
        written.write(&path).unwrap();
        let read = Endpoint::read(&path).unwrap();
        assert_eq!(read.instance_id, written.instance_id);
        assert_eq!(read.workspace_id, written.workspace_id);

        let mut wrong = written.clone();
        wrong.protocol_version = PROTOCOL_VERSION + 1;
        wrong.write(&path).unwrap();
        assert!(matches!(
            Endpoint::read(&path),
            Err(EndpointError::Protocol { .. })
        ));
    }

    #[test]
    fn a_missing_record_says_start_the_host() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Endpoint::discover(dir.path()),
            Err(EndpointError::Missing { .. })
        ));
    }

    #[test]
    fn removal_refuses_to_delete_a_newer_hosts_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        let stale = record(dir.path());
        let live = record(dir.path());
        live.write(&path).unwrap();
        stale.remove(&path);
        assert!(path.exists());
    }
}
