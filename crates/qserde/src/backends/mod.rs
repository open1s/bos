//! Serialization backend implementations
//!
//! This module provides a pluggable backend system for serialization,
//! supporting multiple formats: rkyv, serde/json, bincode, cbor, and postcard.

#[cfg(feature = "bincode-backend")]
mod bincode;
#[cfg(feature = "cbor-backend")]
mod cbor;
#[cfg(feature = "postcard-backend")]
mod postcard;
#[cfg(feature = "rkyv-backend")]
mod rkyv;
#[cfg(feature = "serde-backend")]
mod serde_;

#[cfg(feature = "bincode-backend")]
pub use bincode::BincodeBackend;
#[cfg(feature = "cbor-backend")]
pub use cbor::CborBackend;
#[cfg(feature = "postcard-backend")]
pub use postcard::PostcardBackend;
#[cfg(feature = "rkyv-backend")]
pub use rkyv::RkyvBackend;
#[cfg(all(feature = "serde-backend", feature = "cbor-backend"))]
pub use serde_::SerdeCborBackend;
#[cfg(feature = "serde-backend")]
pub use serde_::SerdeJsonBackend;
