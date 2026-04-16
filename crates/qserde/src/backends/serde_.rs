//! Serde-based backends
//!
//! This module is only available with the `serde-backend` feature.

use crate::error::QserdeError;
use serde::{de::DeserializeOwned, Serialize};

/// Serde JSON backend
///
/// This provides serialization using serde_json.
#[derive(Clone, Default)]
pub struct SerdeJsonBackend;

impl SerdeJsonBackend {
    /// Serialize a value to JSON bytes
    pub fn serialize<T>(&self, value: &T) -> Result<Vec<u8>, QserdeError>
    where
        T: Serialize,
    {
        serde_json::to_vec(value).map_err(|e| QserdeError::serialize(e.to_string()))
    }

    /// Deserialize bytes from JSON
    pub fn deserialize<T>(&self, bytes: &[u8]) -> Result<T, QserdeError>
    where
        T: DeserializeOwned,
    {
        serde_json::from_slice(bytes).map_err(|e| QserdeError::deserialize(e.to_string()))
    }
}

/// Serde CBOR backend
///
/// This provides serialization using ciborium (via serde).
/// Requires the `cbor-backend` feature.
#[cfg(feature = "cbor-backend")]
mod cbor_backend {
    use crate::error::QserdeError;
    use serde::{de::DeserializeOwned, Serialize};

    /// Serde CBOR backend
    #[derive(Clone, Default)]
    pub struct SerdeCborBackend;

    impl SerdeCborBackend {
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
}

#[cfg(feature = "cbor-backend")]
pub use cbor_backend::SerdeCborBackend;
