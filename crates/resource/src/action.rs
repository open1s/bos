//! The unified message envelope: a single `ResourceAction` rkyv enum covers
//! all five resource classes. Responses mirror it via `ResourceOutput`.

use rkyv::{Archive, Deserialize, Serialize};
use rkyv::api::high::{to_bytes, from_bytes};
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
            Some(s) => STANDARD.decode(s).map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// A request to a resource. This is the single, typed entry point that the
/// message model exposes; everything an agent can do to any resource is one
/// of these variants.
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize)]
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
    List { pattern: Option<String> },
    /// Subscribe to push events (see [`ResourceEvent`]).
    Subscribe { events: Vec<String> },

    // ---- storage ----
    Read { offset: u64, len: u64 },
    Write {
        offset: u64,
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    MkDir { recursive: bool },
    Remove { recursive: bool },
    /// Shrink or extend a file to exactly `len` bytes (POSIX `ftruncate`).
    Truncate { len: u64 },
    /// Atomically rename to `new_uri` (must be a same-scheme URI on the same
    /// node; POSIX `rename`).
    Rename { new_uri: String },
    /// Stat-like metadata: size, kind, read-only flag, modification time.
    Stat,
    /// Advisory lock (`flock` semantics). `exclusive` = write lock;
    /// `exclusive = false` = shared read lock.
    Lock { exclusive: bool },
    /// Release a held advisory lock.
    Unlock,

    // ---- network ----
    Bind { addr: String },
    Connect { addr: String },
    Send {
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    Recv { max: u64 },

    // ---- compute ----
    Spawn { args: Vec<String>, env: Vec<(String, String)> },
    Kill { signal: i32 },
    Wait,

    // ---- system ----
    SendSignal { signal: i32 },
    EnvGet { key: String },
    EnvSet { key: String, value: String },
    PipeOpen { name: String },

    // ---- multi-hop routing ----
    /// Forward `payload` (an rkyv-serialized inner `ResourceAction`) targeting
    /// `uri` on a downstream node. Only meaningful on a `relay://<node>`
    /// resource: the handler decodes the payload and re-invokes it downstream.
    /// Bytes instead of `Box<ResourceAction>` because recursive enum inference
    /// in rkyv is not worth the complexity for a transport hop.
    Forward {
        uri: String,
        #[serde(with = "b64")]
        payload: Vec<u8>,
    },

    // ---- abstract / virtual ----
    Query {
        statement: String,
        #[serde(with = "b64_vec")]
        params: Vec<Vec<u8>>,
    },
    Invoke {
        tool: String,
        #[serde(with = "b64")]
        input: Vec<u8>,
    },
    Get { key: String },
    Put {
        key: String,
        #[serde(with = "b64")]
        value: Vec<u8>,
    },

    // ---- policy administration (admin identity only) ----
    PolicyUpdate { doc: String },
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
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize)]
#[rkyv(derive(Debug))]
pub enum ResourceOutput {
    Opened,
    Closed,
    Status {
        state: ResourceStateLabel,
    },
    Listed {
        entries: Vec<String>,
    },
    Subscribed,

    ReadOk {
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    WriteOk {
        written: u64,
    },
    MkDirOk,
    Removed,
    Truncated,
    Renamed,
    StatOk {
        size: u64,
        is_dir: bool,
        readonly: bool,
        /// Seconds since Unix epoch (`SystemTime`); -1 if unavailable.
        modified_secs: i64,
    },
    Locked,
    Unlocked,

    Bound,
    Connected,
    Sent {
        sent: u64,
    },
    RecvOk {
        #[serde(with = "b64")]
        data: Vec<u8>,
    },

    Spawned {
        pid: u32,
    },
    Killed,
    Exited {
        code: i32,
    },

    SignalSent,
    EnvGot {
        value: String,
    },
    EnvSet,
    PipeOpened,

    QueryOk {
        #[serde(with = "b64_vec")]
        rows: Vec<Vec<u8>>,
    },
    Invoked {
        #[serde(with = "b64")]
        output: Vec<u8>,
    },
    Got {
        #[serde(with = "b64_opt")]
        value: Option<Vec<u8>>,
    },
    Put,

    PolicyUpdated,

    /// Streaming header: large `ReadOk`/`RecvOk` bodies are sent as raw bytes
    /// on the same QUIC stream *after* this frame. `kind`: 1 = ReadOk, 2 = RecvOk.
    /// `total` is the byte length of the body that follows.
    Streaming { kind: u8, total: Option<u64> },
}

/// Push events delivered over the subscription stream.
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize)]
#[rkyv(derive(Debug))]
pub enum ResourceEvent {
    Data(#[serde(with = "b64")] Vec<u8>),
    StateChanged(ResourceStateLabel),
    Exited(i32),
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