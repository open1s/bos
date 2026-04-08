use rkyv::{
    api::high::HighDeserializer,
    rancor::{Error as RkyvError, Strategy},
    ser::{allocator::ArenaHandle, sharing::Share, Serializer},
    util::AlignedVec,
    Archive as RkyvArchive,
};
use thiserror::Error;

pub use rkyv;
pub use serde_derive::{archive, snapshot, Archive, Snapshot};

type SerializeStrategy<'a> = Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, RkyvError>;
type DeserializeStrategy = HighDeserializer<RkyvError>;

pub mod prelude {
    pub use crate::{
        archive, dump, load, snapshot, Archive, Archived, Deserialize, DeserializeExt, Result,
        Serialize, Snapshot,
    };
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to serialize value with rkyv: {0}")]
    Serialize(#[source] RkyvError),
    #[error("failed to deserialize value with rkyv: {0}")]
    Deserialize(#[source] RkyvError),
}

pub type Result<T> = core::result::Result<T, Error>;

pub trait Serialize: RkyvArchive
where
    for<'a> Self: rkyv::Serialize<SerializeStrategy<'a>>,
{
    #[inline]
    fn serialize(&self) -> Result<Vec<u8>>
    where
        Self: Sized,
    {
        to_bytes(self)
    }

    #[inline]
    fn to_bytes(&self) -> Result<Vec<u8>>
    where
        Self: Sized,
    {
        to_bytes(self)
    }

    #[inline]
    fn dump(&self) -> Result<Vec<u8>>
    where
        Self: Sized,
    {
        to_bytes(self)
    }
}

impl<T> Serialize for T
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
}

pub trait Deserialize: RkyvArchive + Sized
where
    Self::Archived: rkyv::Deserialize<Self, DeserializeStrategy>,
{
    #[inline]
    fn deserialize(bytes: &[u8]) -> Result<Self> {
        from_bytes(bytes)
    }

    #[inline]
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        from_bytes(bytes)
    }

    #[inline]
    fn load(bytes: &[u8]) -> Result<Self> {
        from_bytes(bytes)
    }
}

impl<T> Deserialize for T
where
    T: RkyvArchive + Sized,
    T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
{
}

pub trait DeserializeExt {
    fn load<T>(&self) -> Result<T>
    where
        T: RkyvArchive,
        T::Archived: rkyv::Deserialize<T, DeserializeStrategy>;
}

impl DeserializeExt for [u8] {
    #[inline]
    fn load<T>(&self) -> Result<T>
    where
        T: RkyvArchive,
        T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
    {
        from_bytes(self)
    }
}

impl DeserializeExt for Vec<u8> {
    #[inline]
    fn load<T>(&self) -> Result<T>
    where
        T: RkyvArchive,
        T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
    {
        self.as_slice().load()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived<T> {
    bytes: Vec<u8>,
    _marker: core::marker::PhantomData<fn() -> T>,
}

impl<T> Archived<T> {
    #[inline]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            _marker: core::marker::PhantomData,
        }
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl<T> Archived<T>
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
    #[inline]
    pub fn from_value(value: &T) -> Result<Self> {
        to_bytes(value).map(Self::new)
    }
}

impl<T> Archived<T>
where
    T: RkyvArchive,
    T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
{
    #[inline]
    pub fn deserialize(&self) -> Result<T> {
        from_bytes(&self.bytes)
    }

    #[inline]
    pub fn load(&self) -> Result<T> {
        self.deserialize()
    }
}

pub fn to_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
    rkyv::to_bytes::<RkyvError>(value)
        .map(|bytes| bytes.into_vec())
        .map_err(Error::Serialize)
}

pub fn from_bytes<T>(bytes: &[u8]) -> Result<T>
where
    T: RkyvArchive,
    T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
{
    unsafe { rkyv::from_bytes_unchecked::<T, RkyvError>(bytes) }.map_err(Error::Deserialize)
}

#[inline]
pub fn dump<T>(value: &T) -> Result<Vec<u8>>
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
    to_bytes(value)
}

#[inline]
pub fn load<T>(bytes: &[u8]) -> Result<T>
where
    T: RkyvArchive,
    T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
{
    from_bytes(bytes)
}
