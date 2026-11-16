//! rkyv codec used for the bus wire format.
//!
//! [`Codec`] serializes values with rkyv and [`DEFAULT_CODEC`] is the shared
//! instance the bus uses.

use rkyv::{
    rancor::{Error, Strategy},
    ser::{allocator::ArenaHandle, sharing::Share, Serializer},
    util::AlignedVec,
    Archive, Serialize,
};

/// Stateless rkyv codec for the bus wire format.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Codec;

impl Default for Codec {
    fn default() -> Self {
        Codec
    }
}

impl Codec {
    /// Serialize `value` into rkyv bytes.
    pub fn encode<T>(&self, value: &T) -> anyhow::Result<Vec<u8>>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    {
        Ok(rkyv::to_bytes::<Error>(value)?.into_vec())
    }

    /// Deserialize rkyv `data` back into a `T`.
    ///
    /// The bytes must come from [`Codec::encode`] for the same type.
    pub fn decode<T>(&self, data: &[u8]) -> anyhow::Result<T>
    where
        T: Archive,
        T::Archived: rkyv::Deserialize<T, rkyv::api::high::HighDeserializer<Error>>
            + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, Error>>,
    {
        // Validate the archive before rebuilding it. These bytes arrive from
        // Zenoh peers; from_bytes_unchecked made a malformed payload an
        // out-of-bounds read (and an empty one a SIGSEGV).
        rkyv::from_bytes::<T, Error>(data)
            .map_err(|e| anyhow::anyhow!("rkyv validation/deserialization failed: {}", e))
    }
}

/// Shared [`Codec`] instance for callers that do not need their own.
pub static DEFAULT_CODEC: Codec = Codec;

/// Backward-compatible alias for [`Codec`].
pub type RkyvCodec = Codec;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_payloads_are_rejected_instead_of_read_out_of_bounds() {
        let codec = Codec;
        assert!(codec.decode::<String>(&[]).is_err());
        assert!(codec.decode::<String>(&[0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        let bytes = codec.encode(&"hello".to_string()).expect("encode");
        assert_eq!(codec.decode::<String>(&bytes).expect("decode"), "hello");
    }
}
