//! CBOR backend (direct, not via serde)
//!
//! This module is only available with the `cbor-backend` feature.

use crate::error::QserdeError;
use serde::{de::DeserializeOwned, Serialize};

/// CBOR backend using ciborium
///
/// This provides direct CBOR serialization using ciborium.
#[derive(Clone, Default)]
pub struct CborBackend;

impl CborBackend {
    /// Serialize a value to CBOR bytes
    pub fn serialize<T>(&self, value: &T) -> Result<Vec<u8>, QserdeError>
    where
        T: Serialize,
    {
        use ciborium::ser::into_writer;
        let mut buf = Vec::new();
        into_writer(value, &mut buf).map_err(|e| QserdeError::serialize(e.to_string()))?;
        Ok(buf)
    }

    /// Deserialize bytes from CBOR
    pub fn deserialize<T>(&self, bytes: &[u8]) -> Result<T, QserdeError>
    where
        T: DeserializeOwned,
    {
        use ciborium::de::from_reader;
        from_reader(bytes).map_err(|e| QserdeError::deserialize(e.to_string()))
    }
}
