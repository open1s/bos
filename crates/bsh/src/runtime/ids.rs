//! Typed identifiers for the runtime domain.
//!
//! Every entity that appears in the Application Contract gets its own type so
//! a session id cannot be passed where an execution id is expected, and so a
//! wire representation stays a single documented string.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a fresh random identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// The underlying UUID.
            #[must_use]
            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

uuid_id!(
    WorkspaceId,
    "Identity of the local workspace a Host serves."
);
uuid_id!(SessionId, "Identity of an agent session owned by the Host.");
uuid_id!(
    ExecutionId,
    "Identity of one turn/execution owned by the Host."
);
uuid_id!(TaskId, "Identity of a delegated or scheduled task.");
uuid_id!(ApprovalId, "Identity of one pending approval request.");
uuid_id!(EventId, "Identity of one committed journal event.");
uuid_id!(MessageId, "Identity of one projected chat message.");
uuid_id!(
    CommandId,
    "Client-supplied idempotency key of one mutating command."
);
uuid_id!(
    ClientInstanceId,
    "Host-issued identity of one client connection/process."
);
uuid_id!(
    HostInstanceId,
    "Identity of one running Runtime Host process."
);

/// An operation reference returned when the Host durably accepts a command.
///
/// It is opaque to clients: use it to query completion. For a turn it is the
/// execution id; for a created session it is the session id; for a control it
/// is the decided approval id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    /// Wrap a domain identifier as an operation reference.
    #[must_use]
    pub fn new(value: impl ToString) -> Self {
        Self(value.to_string())
    }

    /// The operation reference as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl WorkspaceId {
    /// Derive the stable workspace identity from a canonical path.
    ///
    /// A Host that restarts against the same workspace must serve the same
    /// workspace identity, otherwise every client's stored ids would address
    /// a different namespace after the restart.
    #[must_use]
    pub fn derived(seed: &str) -> Self {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(seed.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // Keep the value a well-formed RFC 9562 name-based UUID.
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(Uuid::from_bytes(bytes))
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for OperationId {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.to_string()))
    }
}

/// A content digest over the exact action an approval decision authorizes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionDigest(String);

impl ActionDigest {
    /// Digest the canonical serialization of one action description.
    #[must_use]
    pub fn of<T: Serialize>(action: &T) -> Self {
        use sha2::{Digest, Sha256};
        // Canonical means "the bytes this crate serializes", not RFC 8785.
        // Both sides of an approval digest this same structure, so the exact
        // byte order only has to be stable across versions.
        let bytes = serde_json::to_vec(action).unwrap_or_default();
        let digest = Sha256::digest(bytes);
        let mut text = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    /// The digest as lowercase hexadecimal.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActionDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A stable hash of one command payload, used for idempotency comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PayloadHash(String);

impl PayloadHash {
    /// Hash the canonical serialization of a command payload.
    #[must_use]
    pub fn of<T: Serialize>(payload: &T) -> Self {
        Self(ActionDigest::of(payload).0)
    }

    /// The hash as lowercase hexadecimal.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PayloadHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
