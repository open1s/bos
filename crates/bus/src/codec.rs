use rkyv::{
    rancor::{Error, Strategy},
    ser::{allocator::ArenaHandle, sharing::Share, Serializer},
    util::AlignedVec,
    Archive, Serialize,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Codec;

impl Default for Codec {
    fn default() -> Self {
        Codec
    }
}

impl Codec {
    pub fn encode<T>(&self, value: &T) -> anyhow::Result<Vec<u8>>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    {
        Ok(rkyv::to_bytes::<Error>(value)?.into_vec())
    }

    pub fn decode<T>(&self, data: &[u8]) -> anyhow::Result<T>
    where
        T: Archive,
        T::Archived: rkyv::Deserialize<T, rkyv::api::high::HighDeserializer<Error>>,
    {
        unsafe {
            rkyv::from_bytes_unchecked::<T, Error>(data)
                .map_err(|e| anyhow::anyhow!("rkyv deserialization failed: {}", e))
        }
    }
}

pub static DEFAULT_CODEC: Codec = Codec;

pub type RkyvCodec = Codec;
