use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsonCodec;

impl JsonCodec {
    pub fn encode<T: Serialize>(&self, value: &T) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(value)?)
    }

    pub fn decode<T: DeserializeOwned>(&self, data: &[u8]) -> anyhow::Result<T> {
        Ok(serde_json::from_slice(data)?)
    }
}

pub static DEFAULT_CODEC: JsonCodec = JsonCodec;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Codec {
    Json(JsonCodec),
    Bincode,
}

impl Codec {
    pub fn encode<T: Serialize>(&self, value: &T) -> anyhow::Result<Vec<u8>> {
        match self {
            Codec::Json(c) => c.encode(value),
            Codec::Bincode => Ok(bincode::serialize(value)?),
        }
    }

    pub fn decode<T: DeserializeOwned>(&self, data: &[u8]) -> anyhow::Result<T> {
        match self {
            Codec::Json(c) => c.decode(data),
            Codec::Bincode => Ok(bincode::deserialize(data)?),
        }
    }
}

impl Default for Codec {
    fn default() -> Self {
        Codec::Json(JsonCodec)
    }
}

impl From<JsonCodec> for Codec {
    fn from(c: JsonCodec) -> Self {
        Codec::Json(c)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BincodeCodec;

impl BincodeCodec {
    pub fn encode<T: Serialize>(&self, value: &T) -> anyhow::Result<Vec<u8>> {
        Ok(bincode::serialize(value)?)
    }

    pub fn decode<T: DeserializeOwned>(&self, data: &[u8]) -> anyhow::Result<T> {
        Ok(bincode::deserialize(data)?)
    }
}
