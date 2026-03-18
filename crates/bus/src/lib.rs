//! BrainOS Zenoh communication wrapper
//!
//! Provides common abstractions for Zenoh-based inter-component communication.

pub mod error;
pub mod publisher;
pub mod query;
pub mod queryable;
pub mod session;
pub mod subscriber;

#[cfg(feature = "python-extension")]
pub mod python;

pub use error::ZenohError;
pub use publisher::PublisherWrapper;
pub use query::QueryWrapper;
pub use queryable::QueryableWrapper;
pub use session::{SessionManager, SessionManagerBuilder};
pub use subscriber::SubscriberWrapper;

pub use zenoh::Session;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZenohConfig {
    pub mode: String,
    pub connect: Vec<String>,
    pub listen: Vec<String>,
    pub peer: Option<String>,
}

impl Default for ZenohConfig {
    fn default() -> Self {
        Self {
            mode: "peer".to_string(),
            connect: vec![],
            listen: vec![],
            peer: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct JsonCodec;

impl JsonCodec {
    pub fn encode<T: serde::Serialize>(&self, value: &T) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(value)?)
    }

    pub fn decode<T: serde::de::DeserializeOwned>(&self, data: &[u8]) -> anyhow::Result<T> {
        Ok(serde_json::from_slice(data)?)
    }
}

pub static DEFAULT_CODEC: JsonCodec = JsonCodec;
