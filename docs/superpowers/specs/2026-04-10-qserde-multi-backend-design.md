---
name: qserde multi-backend design
description: Refactor qserde to support all Rust types and multiple serialization backends
type: project
---

# qserde Multi-Backend Serialization Design

## Overview

Refactor qserde to support all Rust types (sized + unsized) with a pluggable multi-backend architecture supporting rkyv, serde, bincode, cbor, and postcard.

## Goals

- Support all sized types and unsized types (DSTs)
- Pluggable backends: rkyv, serde, bincode, cbor, postcard
- Unified trait-based API with runtime backend selection
- Backward compatibility with existing qserde API
- no_std support (partial)

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ qserde                                                     │
├─────────────────────────────────────────────────────────────┤
│ #[qserde] ──────► Derive Macro ──────► Trait Implementations│
│ (qserde_derive)                                            │
├─────────────────────────────────────────────────────────────┤
│ Core Traits Layer                                          │
│ ┌─────────────┐ ┌─────────────┐ ┌─────────────┐            │
│ │ Serialize   │ │ Deserialize │ │ Backend     │            │
│ └─────────────┘ └─────────────┘ └─────────────┘            │
├─────────────────────────────────────────────────────────────┤
│ Backend Implementations                                    │
│ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐     │
│ │ Rkyv   │ │ Serde  │ │Bincode │ │ Cbor   │ │Postcard│     │
│ └────────┘ └────────┘ └────────┘ └────────┘ └────────┘     │
└─────────────────────────────────────────────────────────────┘
```

## Core Traits

```rust
/// The main trait that all backends implement
pub trait Backend: Clone + Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn serialize<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>, Self::Error>
    where T: Serialize;

    fn deserialize<T: ?Sized>(&self, bytes: &[u8]) -> Result<T, Self::Error>
    where T: Deserialize;
}

/// Marker trait for serializable types
pub trait Serialize {
    fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>;
}

/// Marker trait for deserializable types
pub trait Deserialize: Sized {
    fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>;
}
```

Key design decisions:
- `?Sized` bounds to support unsized types (`dyn Trait`, `[T]`, `str`)
- `Clone + Send + Sync + 'static` — backends are shareable across threads
- Error is associated type for backend-specific error types

## Backend Implementations

### Rkyv Backend
- Zero-copy deserialization
- no_std + alloc support

### Serde Backend
- Delegates to serde impls
- Requires specifying a format: `SerdeJsonBackend`, `SerdeCborBackend`, etc.
- Each format is a separate implementation of the Backend trait

### Bincode Backend
- Compact binary format
- no_std + alloc support

### Cbor Backend
- Concise Binary Object Representation
- Uses serde underneath

### Postcard Backend
- Optimized for no_std / embedded
- Full no_std support

## Derive Macro

```rust
// User code
#[qserde(backend = rkyv)] // Optional: specify default, or pick at runtime
#[derive(Debug, Serialize, Deserialize)]
struct User {
    id: u64,
    name: String,
    email: Option<String>,
}

// Usage with runtime backend selection
let bytes = user.serialize(&RkyvBackend)?;
let user2 = User::deserialize(&BincodeBackend, &bytes)?;

// Or using prelude shortcuts
let bytes = user.serialize::<RkyvBackend>()?;
```

## Unsized Types Support

- Must use behind a pointer (Box, Rc, Arc, &, &mut)
- For `dyn Trait`: serialize data + vtable (backend support varies)
- For slices `[T]`: serialize length + data
- For `str`: serialize as UTF-8 bytes

## Error Handling

```rust
// Unified error enum
#[derive(Debug, Error)]
pub enum QserdeError {
    #[error("serialization failed: {0}")]
    Serialize(String),
    #[error("deserialization failed: {0}")]
    Deserialize(String),
    #[error("backend not supported: {0}")]
    UnsupportedBackend(String),
}
```

## Feature Flags

```toml
[features]
default = ["std"]
std = []
alloc = []  # Requires alloc for Vec, Box, String

# Backend features
rkyv-backend = ["rkyv"]
serde-backend = ["serde"]
bincode-backend = ["bincode"]
cbor-backend = ["serde", "cbor"]
postcard-backend = ["postcard"]

# All backends
all-backends = ["rkyv-backend", "serde-backend", "bincode-backend", "cbor-backend", "postcard-backend"]
```

## Migration Path

Keep existing API backward compatible:

```rust
// Old API (still works)
let bytes = user.dump()?;
let user2 = bytes.load::<User>()?;

// New API (extensible)
let bytes = user.serialize(&RkyvBackend)?;
let user2 = User::deserialize(&BincodeBackend, &bytes)?;
```

## File Structure

```
crates/qserde/
├── Cargo.toml
└── src/
    ├── lib.rs         # Core traits, re-exports
    ├── backends/
    │   ├── mod.rs     # Backend trait, re-exports
    │   ├── rkyv.rs    # RkyvBackend
    │   ├── serde.rs   # SerdeBackend
    │   ├── bincode.rs # BincodeBackend
    │   ├── cbor.rs    # CborBackend
    │   └── postcard.rs# PostcardBackend
    ├── error.rs       # Error types
    └── ergonomic.rs   # Convenience methods (keep existing)

crates/qserde_derive/
├── Cargo.toml
└── src/
    └── lib.rs         # #[qserde] derive macro
```