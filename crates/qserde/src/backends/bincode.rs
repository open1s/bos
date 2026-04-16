//! Bincode backend
//!
//! This module is only available with the `bincode-backend` feature.

use crate::error::QserdeError;
use bincode::de::Decode;
use bincode::enc::Encode;

/// Bincode backend - compact binary format
///
/// This provides serialization using bincode 2.x with its native
/// Encode/Decode traits.
#[derive(Clone, Default)]
pub struct BincodeBackend;

impl BincodeBackend {
    /// Serialize a value to bincode bytes
    pub fn serialize<T>(&self, value: &T) -> Result<Vec<u8>, QserdeError>
    where
        T: Encode,
    {
        // Use encode_to_vec which handles allocation automatically
        let bytes = bincode::encode_to_vec(value, bincode::config::standard())
            .map_err(|e| QserdeError::serialize(e.to_string()))?;
        Ok(bytes)
    }

    /// Deserialize bytes from bincode
    pub fn deserialize<T>(&self, bytes: &[u8]) -> Result<T, QserdeError>
    where
        T: Decode<()>,
    {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .map(|(value, _)| value)
            .map_err(|e| QserdeError::deserialize(e.to_string()))
    }
}
