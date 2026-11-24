//! BrainOS communication layer: pub/sub, queryable, and caller/callable over Zenoh.
#![warn(missing_docs)]

mod callable;
mod caller;
pub mod codec;
pub mod error;
pub mod publisher;
pub mod query;
pub mod queryable;
pub mod session;
pub mod subscriber;
pub mod tls_client;
pub mod tls_terminator;

pub use callable::Callable;
pub use caller::Caller;
pub use codec::{Codec, DEFAULT_CODEC, RkyvCodec};
pub use error::ZenohError;
pub use publisher::Publisher;
pub use query::{Query, QueryWrapper, Queryable, TopicQueryable};
pub use queryable::QueryableWrapper;
pub use session::{Bus, BusConfig, TlsConfig};
pub use subscriber::{MAX_CONSECUTIVE_DECODE_FAILURES, RecvOutcome, Subscriber};
pub use zenoh::Session;
