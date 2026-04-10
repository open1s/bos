//! Postcard backend - optimized for no_std/embedded
//!
//! This module is only available with the `postcard-backend` feature.

use crate::error::QserdeError;
use serde::{Serialize, de::DeserializeOwned};

/// Postcard backend for no_std/embedded
///
/// This provides serialization using postcard, optimized for
/// no_std and embedded systems.
#[derive(Clone, Default)]
pub struct PostcardBackend;

impl PostcardBackend {
    /// Serialize a value to postcard bytes
    pub fn serialize<T>(&self, value: &T) -> Result<Vec<u8>, QserdeError>
    where
        T: Serialize,
    {
        postcard::to_stdvec(value)
            .map_err(|e| QserdeError::serialize(e.to_string()))
    }

    /// Deserialize bytes from postcard
    pub fn deserialize<T>(&self, bytes: &[u8]) -> Result<T, QserdeError>
    where
        T: DeserializeOwned,
    {
        postcard::from_bytes(bytes).map_err(|e| QserdeError::deserialize(e.to_string()))
    }
}