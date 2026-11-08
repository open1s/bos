//! Resource leases and the chunked stream protocol.
//!
//! A lease is a time-bounded, holder-bound, quota-bounded grant to move bytes
//! over the federation transport (ADR-009, RFC section 14.1). The stream is a
//! sequence of messages keyed by the lease, not an extra transport method:
//! open, chunk, credit, close. Credit is the backpressure mechanism, so a slow
//! reader never makes the sender buffer a whole artifact.
//!
//! Nothing here authenticates a message; the envelope around it does. This
//! module enforces the lease, the order, the quota, and the checksum.

use std::collections::BTreeMap;

use capability::{CapabilityId, NodeId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A lease's stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LeaseId(String);

impl LeaseId {
    /// Build a lease id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LeaseId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One granted resource lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceLease {
    /// Stable id.
    pub id: LeaseId,
    /// The capability the lease is for.
    pub capability: CapabilityId,
    /// The Node allowed to use it.
    pub holder: NodeId,
    /// When it was granted, in milliseconds since the Unix epoch.
    pub granted_at_ms: u64,
    /// When it stops being valid.
    pub expires_at_ms: Option<u64>,
    /// Bytes the holder may move under this lease.
    pub max_bytes: Option<u64>,
    /// Bytes already moved.
    pub used_bytes: u64,
}

impl ResourceLease {
    /// Whether the lease is still valid at a point in time.
    pub fn is_valid(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_none_or(|expires| now_ms < expires)
    }

    /// Bytes still available, when the lease is bounded.
    pub fn remaining(&self) -> Option<u64> {
        self.max_bytes
            .map(|max| max.saturating_sub(self.used_bytes))
    }
}

/// Why a lease or a stream operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    /// No lease with that id.
    #[error("lease {lease} is unknown")]
    UnknownLease {
        /// The lease asked for.
        lease: LeaseId,
    },
    /// The caller is not the lease holder.
    #[error("lease {lease} belongs to {holder}, not to the caller")]
    WrongHolder {
        /// The lease.
        lease: LeaseId,
        /// Who holds it.
        holder: NodeId,
    },
    /// The lease expired.
    #[error("lease {lease} expired")]
    Expired {
        /// The lease.
        lease: LeaseId,
    },
    /// The lease quota would be exceeded.
    #[error("lease {lease} has {remaining} bytes left, {requested} requested")]
    QuotaExceeded {
        /// The lease.
        lease: LeaseId,
        /// Bytes requested.
        requested: u64,
        /// Bytes left.
        remaining: u64,
    },
    /// A chunk arrived out of order.
    #[error("lease {lease} expected chunk {expected}, got {found}")]
    OutOfOrder {
        /// The lease.
        lease: LeaseId,
        /// The next expected sequence number.
        expected: u64,
        /// What arrived.
        found: u64,
    },
    /// The receiver granted less credit than the chunk needs.
    #[error("lease {lease} has {remaining} bytes of credit, {requested} needed")]
    CreditExhausted {
        /// The lease.
        lease: LeaseId,
        /// Bytes the chunk carries.
        requested: u64,
        /// Credit available.
        remaining: u64,
    },
    /// The stream checksum did not match.
    #[error("lease {lease} checksum mismatch: expected {expected}, computed {found}")]
    ChecksumMismatch {
        /// The lease.
        lease: LeaseId,
        /// What the sender declared.
        expected: String,
        /// What the bytes hash to.
        found: String,
    },
    /// The request contradicts the protocol.
    #[error("invalid resource request: {message}")]
    Invalid {
        /// What is wrong.
        message: String,
    },
    /// A transfer checkpoint could not be read or written.
    #[error("cannot access the transfer checkpoints: {message}")]
    Checkpoint {
        /// What happened.
        message: String,
    },
    /// The lease state could not be persisted.
    #[error("the lease store could not be written: {message}")]
    Storage {
        /// The I/O failure.
        message: String,
    },
}

/// The authoritative leases this Node granted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LeaseStore {
    leases: BTreeMap<LeaseId, ResourceLease>,
    /// When set, every mutation is written here before it is acknowledged.
    #[serde(skip)]
    path: Option<std::path::PathBuf>,
}

impl LeaseStore {
    /// An empty, in-memory store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a store from a persistence file, dropping leases that expired.
    ///
    /// A missing file is an empty store. Revoked leases are absent by
    /// construction (revocation rewrites the file), so recovery never revives
    /// one.
    pub fn open(path: impl Into<std::path::PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let mut store = match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_json(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::new(),
            Err(error) => return Err(error),
        };
        let now = capability::unix_ms();
        let before = store.leases.len();
        store
            .leases
            .retain(|_, lease| lease.expires_at_ms.is_none_or(|expiry| expiry > now));
        store.path = Some(path);
        if store.leases.len() != before {
            store
                .persist()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        }
        Ok(store)
    }

    /// Point an in-memory store at a persistence file without loading it.
    pub fn with_persistence(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Parse a store from its file contents.
    pub fn from_json(text: &str) -> std::io::Result<Self> {
        serde_json::from_str(text)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Serialize the store for its persistence file.
    pub fn to_json(&self) -> std::io::Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Write the store atomically, private to the account.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, self.to_json()?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, path)
    }

    fn persist(&self) -> Result<(), ResourceError> {
        self.save().map_err(|error| ResourceError::Storage {
            message: error.to_string(),
        })
    }

    /// Grant a lease. A duplicate id is refused.
    ///
    /// The new lease is written before this returns; a write failure rolls the
    /// in-memory grant back and fails the call.
    pub fn grant(&mut self, lease: ResourceLease) -> Result<(), ResourceError> {
        if self.leases.contains_key(&lease.id) {
            return Err(ResourceError::Invalid {
                message: format!("lease {} already exists", lease.id),
            });
        }
        let id = lease.id.clone();
        self.leases.insert(id.clone(), lease);
        if let Err(error) = self.persist() {
            self.leases.remove(&id);
            return Err(error);
        }
        Ok(())
    }

    /// Revoke a lease. The removal is persisted before it is acknowledged.
    pub fn revoke(&mut self, id: &LeaseId) -> Result<bool, ResourceError> {
        let Some(lease) = self.leases.remove(id) else {
            return Ok(false);
        };
        if let Err(error) = self.persist() {
            self.leases.insert(id.clone(), lease);
            return Err(error);
        }
        Ok(true)
    }

    /// One lease.
    pub fn get(&self, id: &LeaseId) -> Option<&ResourceLease> {
        self.leases.get(id)
    }

    /// Every lease.
    pub fn iter(&self) -> impl Iterator<Item = &ResourceLease> {
        self.leases.values()
    }

    /// Check one transfer against the lease, without charging it.
    pub fn authorize(
        &self,
        id: &LeaseId,
        holder: &NodeId,
        now_ms: u64,
        bytes: u64,
    ) -> Result<(), ResourceError> {
        let lease = self
            .leases
            .get(id)
            .ok_or_else(|| ResourceError::UnknownLease { lease: id.clone() })?;
        if &lease.holder != holder {
            return Err(ResourceError::WrongHolder {
                lease: id.clone(),
                holder: lease.holder.clone(),
            });
        }
        if !lease.is_valid(now_ms) {
            return Err(ResourceError::Expired { lease: id.clone() });
        }
        if let Some(remaining) = lease.remaining() {
            if bytes > remaining {
                return Err(ResourceError::QuotaExceeded {
                    lease: id.clone(),
                    requested: bytes,
                    remaining,
                });
            }
        }
        Ok(())
    }

    /// Authorize and charge a transfer against the lease.
    pub fn charge(
        &mut self,
        id: &LeaseId,
        holder: &NodeId,
        now_ms: u64,
        bytes: u64,
    ) -> Result<(), ResourceError> {
        self.authorize(id, holder, now_ms, bytes)?;
        let previous = self
            .leases
            .get(id)
            .map(|lease| lease.used_bytes)
            .unwrap_or_default();
        if let Some(lease) = self.leases.get_mut(id) {
            lease.used_bytes = lease.used_bytes.saturating_add(bytes);
        }
        if let Err(error) = self.persist() {
            if let Some(lease) = self.leases.get_mut(id) {
                lease.used_bytes = previous;
            }
            return Err(error);
        }
        Ok(())
    }
}

/// One resumable transfer's checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferRecord {
    /// The lease the transfer runs under.
    pub lease: String,
    /// The resource being read.
    pub uri: String,
    /// The peer serving it.
    pub peer: String,
    /// The next byte to request.
    pub position: u64,
    /// When the checkpoint was written, in milliseconds since the Unix epoch.
    pub updated_at_ms: u64,
}

/// Durable transfer checkpoints, so a restart resumes at the last byte.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransferStore {
    #[serde(default)]
    records: std::collections::BTreeMap<String, TransferRecord>,
}

impl TransferStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the checkpoints; a missing file is an empty store.
    pub fn load(path: &std::path::Path) -> Result<Self, ResourceError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| ResourceError::Checkpoint {
                message: format!("{}: {error}", path.display()),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(error) => Err(ResourceError::Checkpoint {
                message: format!("{}: {error}", path.display()),
            }),
        }
    }

    /// Write the checkpoints atomically.
    pub fn save(&self, path: &std::path::Path) -> Result<(), ResourceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| ResourceError::Checkpoint {
                message: format!("{}: {error}", parent.display()),
            })?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|error| ResourceError::Checkpoint {
                message: error.to_string(),
            })?;
        let temporary = path.with_extension("json.part");
        std::fs::write(&temporary, text).map_err(|error| ResourceError::Checkpoint {
            message: format!("{}: {error}", temporary.display()),
        })?;
        std::fs::rename(&temporary, path).map_err(|error| ResourceError::Checkpoint {
            message: format!("{}: {error}", path.display()),
        })
    }

    fn key(lease: &LeaseId, uri: &str) -> String {
        format!("{}:{uri}", lease.as_str())
    }

    /// Record one transfer's position.
    pub fn checkpoint(&mut self, record: TransferRecord) {
        self.records.insert(
            Self::key(&LeaseId::new(record.lease.clone()), &record.uri),
            record,
        );
    }

    /// Forget a completed transfer.
    pub fn remove(&mut self, lease: &LeaseId, uri: &str) {
        self.records.remove(&Self::key(lease, uri));
    }

    /// The next byte to request for one transfer, or 0 when it is new.
    pub fn position(&self, lease: &LeaseId, uri: &str) -> u64 {
        self.records
            .get(&Self::key(lease, uri))
            .map(|record| record.position)
            .unwrap_or(0)
    }

    /// One transfer's checkpoint.
    pub fn get(&self, lease: &LeaseId, uri: &str) -> Option<&TransferRecord> {
        self.records.get(&Self::key(lease, uri))
    }

    /// Every checkpoint.
    pub fn records(&self) -> impl Iterator<Item = &TransferRecord> {
        self.records.values()
    }
}

/// The offset to request: a nonzero request wins, else the stored checkpoint.
pub fn resume_offset(requested: u64, stored: u64) -> u64 {
    if requested == 0 {
        stored
    } else {
        requested
    }
}

/// The messages a resource stream is made of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceStreamMessage {
    /// Start or resume a stream at a byte offset.
    ///
    /// The open carries the receiver's initial credit, so the sender may begin
    /// without a second round trip.
    Open {
        /// The lease the stream runs under.
        lease: LeaseId,
        /// The resource being read.
        uri: String,
        /// The byte offset to start at.
        offset: u64,
        /// Credit the receiver grants up front.
        credit: u64,
    },
    /// One ordered chunk.
    Chunk {
        /// The lease.
        lease: LeaseId,
        /// Sequence number, starting at 0 per stream.
        seq: u64,
        /// The bytes.
        bytes: Vec<u8>,
    },
    /// Receiver-granted credit.
    Credit {
        /// The lease.
        lease: LeaseId,
        /// Bytes the sender may now send.
        bytes: u64,
    },
    /// End of stream, with its total length and checksum.
    Close {
        /// The lease.
        lease: LeaseId,
        /// Total bytes in the stream.
        total: u64,
        /// Hex SHA-256 of the whole resource.
        checksum: String,
    },
    /// The stream stopped early.
    Error {
        /// The lease.
        lease: LeaseId,
        /// Why it stopped.
        message: String,
    },
}

impl ResourceStreamMessage {
    /// The lease this message belongs to.
    pub fn lease(&self) -> &LeaseId {
        match self {
            Self::Open { lease, .. }
            | Self::Chunk { lease, .. }
            | Self::Credit { lease, .. }
            | Self::Close { lease, .. }
            | Self::Error { lease, .. } => lease,
        }
    }
}

/// Hex SHA-256 of a resource's bytes.
pub fn checksum(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The sending half of one stream.
#[derive(Debug, Clone)]
pub struct StreamSender {
    lease: LeaseId,
    seq: u64,
    credit: u64,
    chunk_size: usize,
    sent: u64,
}

impl StreamSender {
    /// A sender with no credit yet, so nothing can be sent until the receiver
    /// grants some.
    pub fn new(lease: LeaseId, chunk_size: usize) -> Self {
        Self {
            lease,
            seq: 0,
            credit: 0,
            chunk_size: chunk_size.max(1),
            sent: 0,
        }
    }

    /// Apply a credit grant from the receiver.
    pub fn grant(&mut self, message: &ResourceStreamMessage) -> Result<(), ResourceError> {
        match message {
            ResourceStreamMessage::Credit { lease, bytes } if lease == &self.lease => {
                self.credit = self.credit.saturating_add(*bytes);
                Ok(())
            }
            ResourceStreamMessage::Open { lease, credit, .. } if lease == &self.lease => {
                self.credit = self.credit.saturating_add(*credit);
                Ok(())
            }
            ResourceStreamMessage::Credit { .. } | ResourceStreamMessage::Open { .. } => {
                Err(ResourceError::Invalid {
                    message: "the credit belongs to another lease".to_string(),
                })
            }
            _ => Err(ResourceError::Invalid {
                message: "only a credit or open message grants credit".to_string(),
            }),
        }
    }

    /// Credit still available.
    pub fn credit(&self) -> u64 {
        self.credit
    }

    /// Bytes sent so far.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Produce the next chunk, or None when credit is exhausted.
    pub fn next_chunk(&mut self, data: &[u8]) -> Option<ResourceStreamMessage> {
        if self.credit == 0 || data.is_empty() {
            return None;
        }
        let length = self.chunk_size.min(self.credit as usize).min(data.len());
        let bytes = data[..length].to_vec();
        let message = ResourceStreamMessage::Chunk {
            lease: self.lease.clone(),
            seq: self.seq,
            bytes,
        };
        self.seq += 1;
        self.credit -= length as u64;
        self.sent += length as u64;
        Some(message)
    }

    /// The close message for the bytes sent so far.
    pub fn close(&self, data: &[u8]) -> ResourceStreamMessage {
        ResourceStreamMessage::Close {
            lease: self.lease.clone(),
            total: data.len() as u64,
            checksum: checksum(data),
        }
    }
}

/// The receiving half of one stream.
#[derive(Debug, Clone)]
pub struct StreamReceiver {
    lease: LeaseId,
    uri: String,
    offset: u64,
    expected_seq: u64,
    bytes: Vec<u8>,
    credit: u64,
    window: u64,
}

impl StreamReceiver {
    /// A receiver starting at a byte offset, with a credit window.
    pub fn new(lease: LeaseId, uri: impl Into<String>, offset: u64, window: u64) -> Self {
        Self {
            lease,
            uri: uri.into(),
            offset,
            expected_seq: 0,
            bytes: Vec::new(),
            credit: 0,
            window: window.max(1),
        }
    }

    /// Grant the initial credit window.
    pub fn open(&mut self) -> ResourceStreamMessage {
        self.credit = self.credit.saturating_add(self.window);
        ResourceStreamMessage::Open {
            lease: self.lease.clone(),
            uri: self.uri.clone(),
            offset: self.offset,
            credit: self.window,
        }
    }

    /// Accept one message from the sender.
    pub fn accept(&mut self, message: &ResourceStreamMessage) -> Result<(), ResourceError> {
        match message {
            ResourceStreamMessage::Chunk { lease, seq, bytes } => {
                if lease != &self.lease {
                    return Err(ResourceError::Invalid {
                        message: "the chunk belongs to another lease".to_string(),
                    });
                }
                if *seq != self.expected_seq {
                    return Err(ResourceError::OutOfOrder {
                        lease: self.lease.clone(),
                        expected: self.expected_seq,
                        found: *seq,
                    });
                }
                if bytes.len() as u64 > self.credit {
                    return Err(ResourceError::CreditExhausted {
                        lease: self.lease.clone(),
                        requested: bytes.len() as u64,
                        remaining: self.credit,
                    });
                }
                self.credit -= bytes.len() as u64;
                self.bytes.extend_from_slice(bytes);
                self.expected_seq += 1;
                Ok(())
            }
            ResourceStreamMessage::Close {
                lease,
                total,
                checksum: declared,
            } => {
                if lease != &self.lease {
                    return Err(ResourceError::Invalid {
                        message: "the close belongs to another lease".to_string(),
                    });
                }
                if *total != self.resume_offset() {
                    return Err(ResourceError::Invalid {
                        message: format!(
                            "the stream declared {total} bytes but {} arrived",
                            self.resume_offset()
                        ),
                    });
                }
                let found = checksum(&self.bytes);
                if &found != declared {
                    return Err(ResourceError::ChecksumMismatch {
                        lease: self.lease.clone(),
                        expected: declared.clone(),
                        found,
                    });
                }
                Ok(())
            }
            _ => Err(ResourceError::Invalid {
                message: "only chunks and closes are accepted here".to_string(),
            }),
        }
    }

    /// Grant more credit, returning the message to send back.
    pub fn grant(&mut self, bytes: u64) -> ResourceStreamMessage {
        self.credit = self.credit.saturating_add(bytes);
        ResourceStreamMessage::Credit {
            lease: self.lease.clone(),
            bytes,
        }
    }

    /// Replenish the window once half of it is consumed.
    pub fn replenish(&mut self) -> Option<ResourceStreamMessage> {
        if self.credit >= self.window / 2 {
            return None;
        }
        Some(self.grant(self.window - self.credit))
    }

    /// The bytes received so far.
    pub fn data(&self) -> &[u8] {
        &self.bytes
    }

    /// The offset a resumed stream continues from.
    pub fn resume_offset(&self) -> u64 {
        self.offset + self.bytes.len() as u64
    }

    /// Credit still available to the sender.
    pub fn credit(&self) -> u64 {
        self.credit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "federation-transfer-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir.join("transfers.json")
    }

    /// A checkpointed position survives a save/load cycle, and completion
    /// removes it.
    #[test]
    fn a_transfer_checkpoint_round_trips() {
        let path = scratch("round-trip");
        let lease = LeaseId::new("lease-1");
        let mut store = TransferStore::new();
        store.checkpoint(TransferRecord {
            lease: lease.as_str().to_string(),
            uri: "file:///artifact".to_string(),
            peer: "node-b".to_string(),
            position: 512,
            updated_at_ms: 1_000,
        });
        store.save(&path).expect("save");

        let loaded = TransferStore::load(&path).expect("load");
        assert_eq!(loaded.position(&lease, "file:///artifact"), 512);
        assert_eq!(
            loaded.get(&lease, "file:///artifact").expect("record").peer,
            "node-b"
        );

        let mut loaded = loaded;
        loaded.remove(&lease, "file:///artifact");
        assert_eq!(loaded.position(&lease, "file:///artifact"), 0);
        loaded.save(&path).expect("save");
        let reloaded = TransferStore::load(&path).expect("load");
        assert_eq!(reloaded.position(&lease, "file:///artifact"), 0);
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    /// A resume uses the stored position only when the caller did not name one.
    #[test]
    fn a_resume_offset_keeps_an_explicit_request() {
        assert_eq!(resume_offset(0, 512), 512);
        assert_eq!(resume_offset(128, 512), 128);
        assert_eq!(resume_offset(0, 0), 0);
    }

    /// A missing file is a fresh store; a corrupt one is an error.
    #[test]
    fn a_missing_or_corrupt_checkpoint_file_is_handled() {
        let path = scratch("corrupt");
        assert_eq!(
            TransferStore::load(&path)
                .expect("missing is empty")
                .records()
                .count(),
            0
        );
        std::fs::write(&path, "not json").expect("write");
        assert!(TransferStore::load(&path).is_err());
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }
}
