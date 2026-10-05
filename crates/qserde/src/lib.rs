//! Ergonomic, typed helpers on top of `rkyv`.
//!
//! Quick start:
//! ```rust
//! use qserde::prelude::*;
//!
//! #[qserde::Archive]
//! #[derive(Debug, PartialEq)]
//! struct User {
//!     id: u64,
//!     name: String,
//! }
//!
//! let user = User { id: 1, name: "Ada".into() };
//! let bytes = user.dump()?;
//! let restored = bytes.load::<User>()?;
//! assert_eq!(restored, user);
//! # Ok::<(), qserde::Error>(())
//! ```
#![warn(missing_docs)]

use rkyv::{
    api::high::HighDeserializer,
    rancor::{Error as RkyvError, Fallible, Strategy},
    ser::{allocator::ArenaHandle, sharing::Share, Serializer},
    util::AlignedVec,
    with::{ArchiveWith, DeserializeWith, SerializeWith},
    Archive as RkyvArchive, Place,
};
use thiserror::Error;

pub use qserde_derive::{archive, snapshot, Archive, Snapshot};
pub use rkyv;

type SerializeStrategy<'a> = Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, RkyvError>;
type DeserializeStrategy = HighDeserializer<RkyvError>;

pub mod backends;
pub mod error;
/// Common trait and helper imports.
pub mod prelude {
    pub use crate::{
        archive, decode, dump, encode, load, snapshot, Archive, Archived, Deserialize,
        DeserializeExt, Result, Serialize, SkipNone, Snapshot,
    };
}

/// Errors from the rkyv-backed helpers.
#[derive(Debug, Error)]
pub enum Error {
    /// rkyv could not serialize the value.
    #[error("failed to serialize value with rkyv: {0}")]
    Serialize(#[source] RkyvError),
    /// rkyv could not deserialize the bytes.
    #[error("failed to deserialize value with rkyv: {0}")]
    Deserialize(#[source] RkyvError),
}

/// Result alias for the helpers in this crate.
pub type Result<T> = core::result::Result<T, Error>;

/// Serialize a value into rkyv bytes.
///
/// Implemented for every archived type that rkyv can serialize, so it is
/// usually used through the [`prelude`] import.
pub trait Serialize: RkyvArchive
where
    for<'a> Self: rkyv::Serialize<SerializeStrategy<'a>>,
{
    /// Alias of [`Serialize::to_bytes`].
    #[inline]
    fn serialize(&self) -> Result<Vec<u8>>
    where
        Self: Sized,
    {
        to_bytes(self)
    }

    /// Serialize into rkyv bytes.
    #[inline]
    fn to_bytes(&self) -> Result<Vec<u8>>
    where
        Self: Sized,
    {
        to_bytes(self)
    }

    /// Alias of [`Serialize::to_bytes`].
    #[inline]
    fn dump(&self) -> Result<Vec<u8>>
    where
        Self: Sized,
    {
        to_bytes(self)
    }

    /// Serialize into a reusable [`Archived`] buffer.
    #[inline]
    fn snapshot(&self) -> Result<Archived<Self>>
    where
        Self: Sized,
    {
        Archived::from_value(self)
    }
}

impl<T> Serialize for T
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
}

/// Rebuild a value from rkyv bytes.
pub trait Deserialize: RkyvArchive + Sized
where
    Self::Archived: rkyv::Deserialize<Self, DeserializeStrategy>,
{
    /// Deserialize from rkyv bytes.
    #[inline]
    fn deserialize(bytes: &[u8]) -> Result<Self> {
        from_bytes(bytes)
    }

    /// Alias of [`Deserialize::deserialize`].
    #[inline]
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        from_bytes(bytes)
    }

    /// Alias of [`Deserialize::deserialize`].
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

/// Deserialize directly from any byte container.
pub trait DeserializeExt {
    /// Deserialize a `T` from `self`.
    fn load<T>(&self) -> Result<T>
    where
        T: RkyvArchive,
        T::Archived: rkyv::Deserialize<T, DeserializeStrategy>;

    /// Alias of [`DeserializeExt::load`].
    #[inline]
    fn decode<T>(&self) -> Result<T>
    where
        T: RkyvArchive,
        T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
    {
        self.load()
    }
}

impl<TBytes> DeserializeExt for TBytes
where
    TBytes: AsRef<[u8]> + ?Sized,
{
    #[inline]
    fn load<T>(&self) -> Result<T>
    where
        T: RkyvArchive,
        T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
    {
        from_bytes(self.as_ref())
    }
}

/// A serialized value kept in its rkyv byte form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived<T> {
    bytes: Vec<u8>,
    _marker: core::marker::PhantomData<fn() -> T>,
}

impl<T> Archived<T> {
    #[inline]
    /// Wrap already-serialized bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            _marker: core::marker::PhantomData,
        }
    }

    #[inline]
    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    /// Length of the byte buffer.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[inline]
    /// Consume the wrapper and return the bytes.
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
    /// Serialize `value` into a new wrapper.
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
    /// Rebuild the value from the stored bytes.
    pub fn deserialize(&self) -> Result<T> {
        from_bytes(&self.bytes)
    }

    #[inline]
    /// Alias of [`Archived::deserialize`].
    pub fn load(&self) -> Result<T> {
        self.deserialize()
    }
}

impl<T> AsRef<[u8]> for Archived<T> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<T> From<Vec<u8>> for Archived<T> {
    #[inline]
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl<T> From<Archived<T>> for Vec<u8> {
    #[inline]
    fn from(value: Archived<T>) -> Self {
        value.into_bytes()
    }
}

/// Serialize `value` into rkyv bytes.
pub fn to_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
    rkyv::to_bytes::<RkyvError>(value)
        .map(|bytes| bytes.into_vec())
        .map_err(Error::Serialize)
}

/// Deserialize a `T` from rkyv bytes.
pub fn from_bytes<T>(bytes: &[u8]) -> Result<T>
where
    T: RkyvArchive,
    T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
{
    unsafe { rkyv::from_bytes_unchecked::<T, RkyvError>(bytes) }.map_err(Error::Deserialize)
}

#[inline]
/// Alias of [`to_bytes`].
pub fn dump<T>(value: &T) -> Result<Vec<u8>>
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
    to_bytes(value)
}

#[inline]
/// Alias of [`from_bytes`].
pub fn load<T>(bytes: &[u8]) -> Result<T>
where
    T: RkyvArchive,
    T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
{
    from_bytes(bytes)
}

#[inline]
/// Alias of [`to_bytes`].
pub fn encode<T>(value: &T) -> Result<Vec<u8>>
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
    to_bytes(value)
}

#[inline]
/// Alias of [`from_bytes`].
pub fn decode<T>(bytes: &[u8]) -> Result<T>
where
    T: RkyvArchive,
    T::Archived: rkyv::Deserialize<T, DeserializeStrategy>,
{
    from_bytes(bytes)
}

#[inline]
/// Serialize `value` into an [`Archived`] buffer.
pub fn snapshot<T>(value: &T) -> Result<Archived<T>>
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
    Archived::from_value(value)
}

/// A wrapper that skips serializing a field and uses `None` for Option types during deserialization.
/// This is similar to `rkyv::with::Skip` but works with `Option<T>` where `T` doesn't need `Default`.
///
/// # Example
/// ```rust
/// use qserde::prelude::*;
///
/// #[qserde::Archive]
/// struct Example {
///     #[rkyv(with = qserde::SkipNone)]
///     optional_field: Option<Box<dyn std::any::Any>>,
/// }
/// ```
pub struct SkipNone;

impl<T> ArchiveWith<Option<T>> for SkipNone {
    type Archived = ();
    type Resolver = ();

    #[inline]
    fn resolve_with(_: &Option<T>, _: Self::Resolver, _: Place<Self::Archived>) {}
}

impl<T, S: Fallible + ?Sized> SerializeWith<Option<T>, S> for SkipNone {
    #[inline]
    fn serialize_with(_: &Option<T>, _: &mut S) -> core::result::Result<(), S::Error> {
        Ok(())
    }
}

impl<T, D: Fallible + ?Sized> DeserializeWith<(), Option<T>, D> for SkipNone {
    #[inline]
    fn deserialize_with(_: &(), _: &mut D) -> core::result::Result<Option<T>, D::Error> {
        Ok(None)
    }
}
