//! The unified message envelope: a single `ResourceAction` rkyv enum covers
//! all five resource classes. Responses mirror it via `ResourceOutput`.
//!
//! `rkyv`s `Archive` derive emits a public `*Resolver` enum whose struct-variant
//! fields it does not document, and `missing_docs` cannot be scoped to that
//! generated item. The lint is therefore disabled for this module only; every
//! type and function below is still documented and reviewed manually.
#![allow(missing_docs)]

use rkyv::api::high::{from_bytes, to_bytes};
use rkyv::{Archive, Deserialize, Serialize};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

use crate::meta::ResourceStateLabel;
use crate::ResourceError;

/// Serde (de)serialization helpers that encode `Vec<u8>` as base64 strings, so
/// the JSON tool surface is LLM-friendly (binary is not raw number arrays).
mod b64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

mod b64_vec {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error> {
        let enc: Vec<String> = v.iter().map(|b| STANDARD.encode(b)).collect();
        enc.serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {
        let enc = Vec::<String>::deserialize(d)?;
        enc.into_iter()
            .map(|s| STANDARD.decode(s).map_err(serde::de::Error::custom))
            .collect()
    }
}

mod b64_opt {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_some(&STANDARD.encode(b)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => STANDARD
                .decode(s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// A request to a resource. This is the single, typed entry point that the
/// message model exposes; everything an agent can do to any resource is one
/// of these variants.
#[derive(
    Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize,
)]
#[rkyv(derive(Debug))]
pub enum ResourceAction {
    // ---- universal lifecycle / discovery ----
    /// Provision/open the resource.
    Open,
    /// Release/close the resource.
    Close,
    /// Query current status.
    Status,
    /// List children / entries (where meaningful).
    List {
        /// Optional glob or name filter.
        pattern: Option<String>,
    },
    /// Subscribe to push events (see [`ResourceEvent`]).
    Subscribe {
        /// Event kinds to subscribe to.
        events: Vec<String>,
    },

    // ---- storage ----
    /// Read `len` bytes starting at `offset`.
    Read {
        /// Byte offset to read from.
        offset: u64,
        /// Number of bytes to read.
        len: u64,
    },
    /// Write `data` starting at `offset`.
    Write {
        /// Byte offset to write at.
        offset: u64,
        /// Bytes to write.
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    /// Create a directory.
    MkDir {
        /// Create parent directories as needed.
        recursive: bool,
    },
    /// Remove a file or directory.
    Remove {
        /// Remove contents recursively.
        recursive: bool,
    },
    /// Shrink or extend a file to exactly `len` bytes (POSIX `ftruncate`).
    Truncate {
        /// Target length in bytes.
        len: u64,
    },
    /// Atomically rename to `new_uri` (must be a same-scheme URI on the same
    /// node; POSIX `rename`).
    Rename {
        /// Destination URI.
        new_uri: String,
    },
    /// Stat-like metadata: size, kind, read-only flag, modification time.
    Stat,
    /// Advisory lock (`flock` semantics). `exclusive` = write lock;
    /// `exclusive = false` = shared read lock.
    Lock {
        /// Whether to take an exclusive (write) lock.
        exclusive: bool,
    },
    /// Release a held advisory lock.
    Unlock,

    // ---- network ----
    /// Bind a listening endpoint.
    Bind {
        /// Address to bind.
        addr: String,
    },
    /// Connect to a remote endpoint.
    Connect {
        /// Address to connect to.
        addr: String,
    },
    /// Send bytes over the connection.
    Send {
        /// Bytes to send.
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    /// Receive bytes from the connection.
    Recv {
        /// Maximum bytes to receive.
        max: u64,
    },

    // ---- compute ----
    /// Spawn a process.
    Spawn {
        /// Command and arguments.
        args: Vec<String>,
        /// Environment variables as key/value pairs.
        env: Vec<(String, String)>,
    },
    /// Signal a process.
    Kill {
        /// Signal number.
        signal: i32,
    },
    /// Wait for a process to exit.
    Wait,

    // ---- system ----
    /// Send a signal to a process.
    SendSignal {
        /// Signal number.
        signal: i32,
    },
    /// Read an environment variable.
    EnvGet {
        /// Environment variable name.
        key: String,
    },
    /// Set an environment variable.
    EnvSet {
        /// Environment variable name.
        key: String,
        /// New value.
        value: String,
    },
    /// Open a named pipe.
    PipeOpen {
        /// Pipe name.
        name: String,
    },

    // ---- multi-hop routing ----
    /// Forward `payload` (an rkyv-serialized inner `ResourceAction`) targeting
    /// `uri` on a downstream node. Only meaningful on a `relay://<node>`
    /// resource: the handler decodes the payload and re-invokes it downstream.
    /// Bytes instead of `Box<ResourceAction>` because recursive enum inference
    /// in rkyv is not worth the complexity for a transport hop.
    Forward {
        /// Downstream URI to forward to.
        uri: String,
        /// rkyv-serialized inner action.
        #[serde(with = "b64")]
        payload: Vec<u8>,
    },

    // ---- abstract / virtual ----
    /// Run a query against the resource.
    Query {
        /// Query statement.
        statement: String,
        /// Encoded query parameters.
        #[serde(with = "b64_vec")]
        params: Vec<Vec<u8>>,
    },
    /// Invoke a tool exposed by the resource.
    Invoke {
        /// Tool name.
        tool: String,
        /// Encoded tool input.
        #[serde(with = "b64")]
        input: Vec<u8>,
    },
    /// Read a key.
    Get {
        /// Key to read.
        key: String,
    },
    /// Store a key.
    Put {
        /// Key to write.
        key: String,
        /// Value to store.
        #[serde(with = "b64")]
        value: Vec<u8>,
    },

    // ---- policy administration (admin identity only) ----
    /// Replace the policy document (admin only).
    PolicyUpdate {
        /// New policy document.
        doc: String,
    },
}

impl ResourceAction {
    /// Stable name used for policy matching (the `action` dimension).
    pub fn name(&self) -> &'static str {
        match self {
            ResourceAction::Open => "open",
            ResourceAction::Close => "close",
            ResourceAction::Status => "status",
            ResourceAction::List { .. } => "list",
            ResourceAction::Subscribe { .. } => "subscribe",
            ResourceAction::Read { .. } => "read",
            ResourceAction::Write { .. } => "write",
            ResourceAction::MkDir { .. } => "mkdir",
            ResourceAction::Remove { .. } => "remove",
            ResourceAction::Truncate { .. } => "truncate",
            ResourceAction::Rename { .. } => "rename",
            ResourceAction::Stat => "stat",
            ResourceAction::Lock { .. } => "lock",
            ResourceAction::Unlock => "unlock",
            ResourceAction::Bind { .. } => "bind",
            ResourceAction::Connect { .. } => "connect",
            ResourceAction::Send { .. } => "send",
            ResourceAction::Recv { .. } => "recv",
            ResourceAction::Spawn { .. } => "spawn",
            ResourceAction::Kill { .. } => "kill",
            ResourceAction::Wait => "wait",
            ResourceAction::SendSignal { .. } => "signal",
            ResourceAction::EnvGet { .. } => "env_get",
            ResourceAction::EnvSet { .. } => "env_set",
            ResourceAction::PipeOpen { .. } => "pipe_open",
            ResourceAction::Query { .. } => "query",
            ResourceAction::Invoke { .. } => "invoke",
            ResourceAction::Get { .. } => "get",
            ResourceAction::Put { .. } => "put",
            ResourceAction::Forward { .. } => "forward",
            ResourceAction::PolicyUpdate { .. } => "policy_update",
        }
    }

    /// Whether this action mutates policy (requires admin identity).
    pub fn is_policy_admin(&self) -> bool {
        matches!(self, ResourceAction::PolicyUpdate { .. })
    }
}

/// Successful response payload, mirroring [`ResourceAction`].
#[derive(
    Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize,
)]
#[rkyv(derive(Debug))]
pub enum ResourceOutput {
    /// The resource was opened.
    Opened,
    /// The resource was closed.
    Closed,
    /// Current resource state.
    Status {
        /// The state label.
        state: ResourceStateLabel,
    },
    /// Directory or listing entries.
    Listed {
        /// Entry names.
        entries: Vec<String>,
    },
    /// The subscription was accepted.
    Subscribed,

    /// Read succeeded.
    ReadOk {
        /// Bytes read.
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    /// Write succeeded.
    WriteOk {
        /// Number of bytes written.
        written: u64,
    },
    /// The directory was created.
    MkDirOk,
    /// The entry was removed.
    Removed,
    /// The file was truncated.
    Truncated,
    /// The entry was renamed.
    Renamed,
    /// Stat result.
    StatOk {
        /// Size in bytes.
        size: u64,
        /// Whether this is a directory.
        is_dir: bool,
        /// Whether the entry is read-only.
        readonly: bool,
        /// Seconds since Unix epoch (`SystemTime`); -1 if unavailable.
        modified_secs: i64,
    },
    /// The lock was acquired.
    Locked,
    /// The lock was released.
    Unlocked,

    /// The endpoint is bound.
    Bound,
    /// The connection is established.
    Connected,
    /// Send result.
    Sent {
        /// Number of bytes sent.
        sent: u64,
    },
    /// Receive result.
    RecvOk {
        /// Bytes received.
        #[serde(with = "b64")]
        data: Vec<u8>,
    },

    /// A process was spawned.
    Spawned {
        /// Process id.
        pid: u32,
    },
    /// The process was killed.
    Killed,
    /// The process exited.
    Exited {
        /// Exit code.
        code: i32,
    },

    /// The signal was sent.
    SignalSent,
    /// Environment lookup result.
    EnvGot {
        /// The value.
        value: String,
    },
    /// The environment variable was set.
    EnvSet,
    /// The pipe was opened.
    PipeOpened,

    /// Query result.
    QueryOk {
        /// Encoded rows.
        #[serde(with = "b64_vec")]
        rows: Vec<Vec<u8>>,
    },
    /// Tool invocation result.
    Invoked {
        /// Encoded tool output.
        #[serde(with = "b64")]
        output: Vec<u8>,
    },
    /// Key lookup result.
    Got {
        /// The value, if present.
        #[serde(with = "b64_opt")]
        value: Option<Vec<u8>>,
    },
    /// The key was stored.
    Put,

    /// The policy was updated.
    PolicyUpdated,

    /// Streaming header: large `ReadOk`/`RecvOk` bodies are sent as raw bytes
    /// on the same QUIC stream *after* this frame. `kind`: 1 = ReadOk, 2 = RecvOk.
    /// `total` is the byte length of the body that follows.
    Streaming {
        /// Stream kind: 1 = ReadOk, 2 = RecvOk.
        kind: u8,
        /// Byte length of the body that follows.
        total: Option<u64>,
    },
}

/// Push events delivered over the subscription stream.
#[derive(
    Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize,
)]
#[rkyv(derive(Debug))]
pub enum ResourceEvent {
    /// A chunk of streamed data.
    Data(#[serde(with = "b64")] Vec<u8>),
    /// The resource state changed.
    StateChanged(ResourceStateLabel),
    /// The process exited with this code.
    Exited(i32),
    /// A provider-defined event.
    Custom(String, #[serde(with = "b64")] Vec<u8>),
}

/// Serialize an action to bytes for transport.
pub fn encode_action(action: &ResourceAction) -> Result<Vec<u8>, ResourceError> {
    to_bytes::<rkyv::rancor::Error>(action)
        .map(|b| b.to_vec())
        .map_err(|e| ResourceError::Codec(e.to_string()))
}

/// Deserialize an action from transport bytes.
pub fn decode_action(bytes: &[u8]) -> Result<ResourceAction, ResourceError> {
    from_bytes::<_, rkyv::rancor::Error>(bytes).map_err(|e| ResourceError::Codec(e.to_string()))
}

/// Serialize an output to bytes for transport.
pub fn encode_output(out: &ResourceOutput) -> Result<Vec<u8>, ResourceError> {
    to_bytes::<rkyv::rancor::Error>(out)
        .map(|b| b.to_vec())
        .map_err(|e| ResourceError::Codec(e.to_string()))
}

/// Deserialize an output from transport bytes.
pub fn decode_output(bytes: &[u8]) -> Result<ResourceOutput, ResourceError> {
    from_bytes::<_, rkyv::rancor::Error>(bytes).map_err(|e| ResourceError::Codec(e.to_string()))
}

/// Serialize an event to bytes for transport.
pub fn encode_event(ev: &ResourceEvent) -> Result<Vec<u8>, ResourceError> {
    to_bytes::<rkyv::rancor::Error>(ev)
        .map(|b| b.to_vec())
        .map_err(|e| ResourceError::Codec(e.to_string()))
}

/// Deserialize an event from transport bytes.
pub fn decode_event(bytes: &[u8]) -> Result<ResourceEvent, ResourceError> {
    from_bytes::<_, rkyv::rancor::Error>(bytes).map_err(|e| ResourceError::Codec(e.to_string()))
}
