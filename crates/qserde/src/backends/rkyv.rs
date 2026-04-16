//! Rkyv backend - zero-copy serialization
//!
//! This module is only available with the `rkyv-backend` feature.

use crate::error::QserdeError;
use rkyv::api::high::HighSerializer;
use rkyv::rancor::Error as RkyvError;
use rkyv::ser::allocator::ArenaHandle;
use rkyv::util::AlignedVec;

/// Rkyv backend - zero-copy serialization
///
/// This provides a convenient wrapper around rkyv serialization.
/// Use this for zero-copy serialization and deserialization.
#[derive(Clone, Default)]
pub struct RkyvBackend;

impl RkyvBackend {
    /// Serialize a value to bytes using rkyv
    pub fn serialize<T>(&self, value: &T) -> Result<Vec<u8>, QserdeError>
    where
        T: rkyv::Archive,
        for<'a> T: rkyv::Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, RkyvError>>,
    {
        rkyv::to_bytes::<RkyvError>(value)
            .map(|b| b.into_vec())
            .map_err(|e| QserdeError::serialize(e.to_string()))
    }

    /// Deserialize bytes to a value using rkyv (zero-copy)
    pub fn deserialize<T>(&self, bytes: &[u8]) -> Result<T, QserdeError>
    where
        T: rkyv::Archive,
        T::Archived: rkyv::Deserialize<T, rkyv::api::high::HighDeserializer<RkyvError>>,
    {
        unsafe { rkyv::from_bytes_unchecked::<T, RkyvError>(bytes) }
            .map_err(|e| QserdeError::deserialize(e.to_string()))
    }
}
