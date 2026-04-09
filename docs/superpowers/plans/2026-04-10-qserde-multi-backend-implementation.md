# qserde Multi-Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor qserde to support all Rust types (sized + unsized) with pluggable backends: rkyv, serde, bincode, cbor, postcard.

**Architecture:** Trait-based backend system with runtime dispatch. Define a `Backend` trait that each serialization backend implements. The `#[qserde]` derive macro generates impls for `Serialize` and `Deserialize` traits that take a backend reference at runtime.

**Tech Stack:** rkyv, serde, bincode, cbor, postcard, thiserror

---

## File Structure

```
crates/qserde/
├── Cargo.toml                    # Modify: add backend dependencies + features
└── src/
    ├── lib.rs                    # Modify: core traits, re-exports
    ├── error.rs                  # Create: QserdeError enum
    ├── ergonomic.rs              # Keep existing for compatibility
    └── backends/
        ├── mod.rs                # Create: Backend trait, re-exports
        ├── rkyv.rs               # Create: RkyvBackend
        ├── serde.rs              # Create: SerdeBackend (with format impls)
        ├── bincode.rs            # Create: BincodeBackend
        ├── cbor.rs               # Create: CborBackend
        └── postcard.rs           # Create: PostcardBackend

crates/qserde_derive/
└── src/lib.rs                    # Modify: extend to generate new trait impls
```

---

## Task 1: Set up Cargo.toml with backend dependencies

**Files:**
- Modify: `crates/qserde/Cargo.toml`

- [ ] **Step 1: Write the failing test (dependency check)**
  ```bash
  cd crates/qserde && cargo build --features serde-backend 2>&1 | head -20
  ```
  Expected: FAIL with "could not find crate `serde`"

- [ ] **Step 2: Update Cargo.toml with all backend dependencies**

  ```toml
  [package]
  name = "qserde"
  version.workspace = true
  edition.workspace = true
  authors.workspace = true
  license.workspace = true
  description = "Multi-backend serialization for Rust types"

  [dependencies]
  rkyv = { workspace = true, optional = true }
  thiserror = { workspace = true }
  qserde_derive = { path = "../qserde_derive" }

  # Serialization backends
  serde = { version = "1.0", features = ["derive"], optional = true }
  serde_json = { version = "1.0", optional = true }
  bincode = { version = "2.0", optional = true }
  cbor = { version = "0.6", optional = true }
  postcard = { version = "1.0", optional = true }

  [dev-dependencies]
  serde = { version = "1.0", features = ["derive"] }
  serde_json = "1.0"

  [features]
  default = ["std", "rkyv-backend"]
  std = []
  alloc = []

  # Backend features
  rkyv-backend = ["rkyv"]
  serde-backend = ["serde", "serde_json"]
  bincode-backend = ["bincode"]
  cbor-backend = ["serde", "cbor"]
  postcard-backend = ["postcard"]

  # Convenience features
  all-backends = [
      "rkyv-backend",
      "serde-backend",
      "bincode-backend",
      "cbor-backend",
      "postcard-backend"
  ]

  [lib]
  name = "qserde"
  path = "src/lib.rs"
  ```

- [ ] **Step 3: Verify build works with default features**
  ```bash
  cd crates/qserde && cargo build
  ```
  Expected: PASS

- [ ] **Step 4: Commit**
  ```bash
  git add crates/qserde/Cargo.toml && git commit -m "feat(qserde): add backend dependencies and feature flags"
  ```

---

## Task 2: Create error module

**Files:**
- Create: `crates/qserde/src/error.rs`

- [ ] **Step 1: Write the failing test**
  ```bash
  cd crates/qserde && cargo build 2>&1 | grep "error.rs"
  ```
  Expected: FAIL with "could not find crate `error`"

- [ ] **Step 2: Create error.rs**

  ```rust
  use thiserror::Error;

  /// Unified error enum for qserde operations
  #[derive(Debug, Error)]
  pub enum QserdeError {
      #[error("serialization failed: {0}")]
      Serialize(String),

      #[error("deserialization failed: {0}")]
      Deserialize(String),

      #[error("backend error: {0}")]
      Backend(String),

      #[error("backend not supported for this operation: {0}")]
      UnsupportedBackend(String),
  }

  impl QserdeError {
      pub fn serialize(msg: impl Into<String>) -> Self {
          Self::Serialize(msg.into())
      }

      pub fn deserialize(msg: impl Into<String>) -> Self {
          Self::Deserialize(msg.into())
      }

      pub fn backend(msg: impl Into<String>) -> Self {
          Self::Backend(msg.into())
      }

      pub fn is_serialize_error(&self) -> bool {
          matches!(self, QserdeError::Serialize(_))
      }

      pub fn is_deserialize_error(&self) -> bool {
          matches!(self, QserdeError::Deserialize(_))
      }
  }

  /// Result type using QserdeError
  pub type Result<T> = core::result::Result<T, QserdeError>;

  // Convenience From implementations for backend errors
  #[cfg(feature = "rkyv-backend")]
  impl From<rkyv::rancor::Error> for QserdeError {
      fn from(e: rkyv::rancor::Error) -> Self {
          QserdeError::backend(e.to_string())
      }
  }
  ```

- [ ] **Step 3: Build to verify**
  ```bash
  cd crates/qserde && cargo build
  ```
  Expected: PASS

- [ ] **Step 4: Commit**
  ```bash
  git add crates/qserde/src/error.rs && git commit -m "feat(qserde): add error module with QserdeError"
  ```

---

## Task 3: Create backends module and Backend trait

**Files:**
- Create: `crates/qserde/src/backends/mod.rs`

- [ ] **Step 1: Write the failing test**
  ```bash
  cd crates/qserde && cargo build 2>&1 | grep "backends"
  ```
  Expected: FAIL with "could not find crate `backends`"

- [ ] **Step 2: Create backends/mod.rs with Backend trait**

  ```rust
  //! Serialization backend implementations

  mod rkyv;
  mod serde_;
  mod bincode;
  mod cbor;
  mod postcard;

  pub use rkyv::RkyvBackend;
  pub use serde_::{SerdeJsonBackend, SerdeCborBackend};
  pub use bincode::BincodeBackend;
  pub use cbor::CborBackend;
  pub use postcard::PostcardBackend;

  use crate::error::QserdeError;

  /// The main trait that all serialization backends implement
  pub trait Backend: Clone + Send + Sync + 'static {
      /// Error type for this backend
      type Error: std::error::Error + Send + Sync + 'static;

      /// Serialize a value to bytes
      fn serialize<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>, Self::Error>
      where
          T: Serialize;

      /// Deserialize bytes to a value
      fn deserialize<T: ?Sized>(&self, bytes: &[u8]) -> Result<T, Self::Error>
      where
          T: Deserialize;
  }

  /// Marker trait for types that can be serialized with any backend
  pub trait Serialize {
      /// Serialize self using the given backend
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>;
  }

  /// Marker trait for types that can be deserialized with any backend
  pub trait Deserialize: Sized {
      /// Deserialize self from bytes using the given backend
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>;
  }

  /// Helper trait for convenient serialization with type-level backend
  pub trait SerializeExt: Serialize + Sized {
      fn serialize_with<B: Backend>(&self) -> Result<Vec<u8>, B::Error>
      where
          Self: Serialize,
      {
          self.serialize(&B::default())
      }
  }

  impl<T: Serialize + Sized> SerializeExt for T {}

  /// Helper trait for convenient deserialization with type-level backend
  pub trait DeserializeExt: Deserialize + Sized {
      fn deserialize_with<B: Backend>(bytes: &[u8]) -> Result<Self, B::Error>
      where
          Self: Deserialize,
      {
          Self::deserialize(&B::default(), bytes)
      }
  }

  impl<T: Deserialize + Sized> DeserializeExt for T {}
  ```

- [ ] **Step 3: Build to verify**
  ```bash
  cd crates/qserde && cargo build 2>&1 | head -30
  ```
  Expected: FAIL with "cannot find trait `Serialize` in this scope" (we need stub backends)

- [ ] **Step 4: Create stub backend files**

  Create `crates/qserde/src/backends/rkyv.rs`:
  ```rust
  use crate::error::QserdeError;

  /// Rkyv backend - zero-copy serialization
  #[derive(Clone, Default)]
  pub struct RkyvBackend;

  // Implementation will be added in Task 4
  ```

  Create `crates/qserde/src/backends/serde_.rs`:
  ```rust
  /// Serde-based backend (JSON format)
  #[derive(Clone, Default)]
  pub struct SerdeJsonBackend;

  /// Serde-based backend (CBOR format)
  #[derive(Clone, Default)]
  pub struct SerdeCborBackend;
  ```

  Create `crates/qserde/src/backends/bincode.rs`:
  ```rust
  /// Bincode backend
  #[derive(Clone, Default)]
  pub struct BincodeBackend;
  ```

  Create `crates/qserde/src/backends/cbor.rs`:
  ```rust
  /// Direct CBOR backend (not via serde)
  #[derive(Clone, Default)]
  pub struct CborBackend;
  ```

  Create `crates/qserde/src/backends/postcard.rs`:
  ```rust
  /// Postcard backend for no_std/embedded
  #[derive(Clone, Default)]
  pub struct PostcardBackend;
  ```

- [ ] **Step 5: Build again**
  ```bash
  cd crates/qserde && cargo build
  ```
  Expected: PASS (with warnings about unused imports)

- [ ] **Step 6: Commit**
  ```bash
  git add crates/qserde/src/backends/ && git commit -m "feat(qserde): add backends module with Backend trait"
  ```

---

## Task 4: Implement RkyvBackend

**Files:**
- Modify: `crates/qserde/src/backends/rkyv.rs`

- [ ] **Step 1: Write the failing test**
  ```bash
  cd crates/qserde && cargo test --features rkyv-backend test_rkyv_backend 2>&1 | grep "test_rkyv_backend"
  ```
  Expected: FAIL (no test exists yet)

- [ ] **Step 2: Add test for RkyvBackend**
  Create `crates/qserde/tests/rkyv_backend.rs`:
  ```rust
  use qserde::backends::{Backend, RkyvBackend, Serialize, Deserialize};

  #[test]
  fn test_rkyv_backend_serialize() {
      let backend = RkyvBackend;
      let value: u32 = 42;
      let bytes = backend.serialize(&value).expect("should serialize");
      assert!(!bytes.is_empty());
  }

  #[test]
  fn test_rkyv_backend_roundtrip() {
      let backend = RkyvBackend;
      let value = "hello".to_string();
      let bytes = backend.serialize(&value).expect("should serialize");
      let restored: String = backend.deserialize(&bytes).expect("should deserialize");
      assert_eq!(restored, value);
  }

  #[test]
  fn test_serialize_trait() {
      let backend = RkyvBackend;
      let value: u32 = 42;
      let bytes = value.serialize(&backend).expect("should serialize");
      assert!(!bytes.is_empty());
  }

  #[test]
  fn test_deserialize_trait() {
      let backend = RkyvBackend;
      let value: u32 = 42;
      let bytes = value.serialize(&backend).expect("should serialize");
      let restored = u32::deserialize(&backend, &bytes).expect("should deserialize");
      assert_eq!(restored, value);
  }
  ```

- [ ] **Step 3: Run test to verify it fails**
  ```bash
  cd crates/qserde && cargo test --features rkyv-backend test_rkyv_backend 2>&1 | tail -20
  ```
  Expected: FAIL with "method `serialize` not found"

- [ ] **Step 4: Implement RkyvBackend**
  Replace contents of `crates/qserde/src/backends/rkyv.rs`:
  ```rust
  //! Rkyv backend - zero-copy serialization

  use crate::backends::{Backend, Deserialize, Serialize};
  use crate::error::QserdeError;
  use rkyv::ser::Serializer;

  /// Rkyv backend - zero-copy serialization
  #[derive(Clone, Default)]
  pub struct RkyvBackend;

  impl Backend for RkyvBackend {
      type Error = rkyv::rancor::Error;

      fn serialize<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>, Self::Error>
      where
          T: Serialize,
      {
          rkyv::to_bytes(value).map(|b| b.into_vec())
      }

      fn deserialize<T: ?Sized>(&self, bytes: &[u8]) -> Result<T, Self::Error>
      where
          T: Deserialize,
      {
          // Safety: we trust the bytes came from a valid serialization
          unsafe { rkyv::from_bytes_unchecked::<T>(bytes) }
      }
  }

  // Implement Serialize for primitive types that rkyv supports
  macro_rules! impl_serialize_for_primitives {
      ($($ty:ty),*) => {
          $(
              impl Serialize for $ty {
                  fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
                  where
                      Self: Sized,
                  {
                      backend.serialize(self)
                  }
              }

              impl Deserialize for $ty {
                  fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
                  where
                      Self: Sized,
                  {
                      backend.deserialize(bytes)
                  }
              }
          )*
      };
  }

  impl_serialize_for_primitives!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, bool, char);

  impl Serialize for str {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          backend.serialize(&self)
      }
  }

  impl Serialize for [u8] {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          backend.serialize(&self)
      }
  }

  impl<T: Serialize + ?Sized> Serialize for &T {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          (*self).serialize(backend)
      }
  }

  impl<T: Serialize + ?Sized> Serialize for &mut T {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          (*self).serialize(backend)
      }
  }

  impl<T: Serialize> Serialize for Box<T> {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          (**self).serialize(backend)
      }
  }

  impl<T: Deserialize> Deserialize for Box<T> {
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
      where
          Self: Sized,
      {
          T::deserialize(backend, bytes).map(Box::new)
      }
  }

  impl<T: Serialize> Serialize for Vec<T> {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          backend.serialize(self)
      }
  }

  impl<T: Deserialize> Deserialize for Vec<T> {
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
      where
          Self: Sized,
      {
          backend.deserialize(bytes)
      }
  }

  impl<T: Serialize> Serialize for Option<T> {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          backend.serialize(self)
      }
  }

  impl<T: Deserialize> Deserialize for Option<T> {
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
      where
          Self: Sized,
      {
          backend.deserialize(bytes)
      }
  }

  impl<T: Serialize, E: Serialize> Serialize for Result<T, E> {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          backend.serialize(self)
      }
  }

  impl<T: Deserialize, E: Deserialize> Deserialize for Result<T, E> {
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
      where
          Self: Sized,
      {
          backend.deserialize(bytes)
      }
  }

  // Tuple implementations
  impl<A: Serialize> Serialize for (A,) {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          backend.serialize(self)
      }
  }

  impl<A: Deserialize> Deserialize for (A,) {
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
      where
          Self: Sized,
      {
          backend.deserialize(bytes)
      }
  }

  macro_rules! impl_serialize_for_tuple {
      ($($idx:tt: $a:ident),*) => {
          impl<$($a: Serialize),*> Serialize for ($($a,)*) {
              fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
              where
                  Self: Sized,
              {
                  backend.serialize(self)
              }
          }

          impl<$($a: Deserialize),*> Deserialize for ($($a,)*) {
              fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
              where
                  Self: Sized,
              {
                  backend.deserialize(bytes)
              }
              }
          }
      };
  }

  impl_serialize_for_tuple!(0: A, 1: B);
  impl_serialize_for_tuple!(0: A, 1: B, 2: C);
  impl_serialize_for_tuple!(0: A, 1: B, 2: C, 3: D);
  ```

- [ ] **Step 5: Run test**
  ```bash
  cd crates/qserde && cargo test --features rkyv-backend test_rkyv_backend 2>&1 | tail -10
  ```
  Expected: PASS

- [ ] **Step 6: Commit**
  ```bash
  git add crates/qserde/src/backends/rkyv.rs crates/qserde/tests/rkyv_backend.rs && git commit -m "feat(qserde): implement RkyvBackend"
  ```

---

## Task 5: Implement SerdeBackend (JSON)

**Files:**
- Modify: `crates/qserde/src/backends/serde_.rs`

- [ ] **Step 1: Write failing test**
  ```bash
  cd crates/qserde && cargo test --features serde-backend test_serde_json_backend 2>&1 | tail -10
  ```
  Expected: FAIL (no test exists)

- [ ] **Step 2: Add test**
  Add to `crates/qserde/tests/rkyv_backend.rs`:
  ```rust
  use qserde::backends::{SerdeJsonBackend, Serialize, Deserialize};

  #[test]
  fn test_serde_json_backend() {
      let backend = SerdeJsonBackend;
      let value = "hello".to_string();
      let bytes = backend.serialize(&value).expect("should serialize");
      let restored: String = backend.deserialize(&bytes).expect("should deserialize");
      assert_eq!(restored, value);
  }
  ```

- [ ] **Step 3: Run test to verify it fails**
  ```bash
  cd crates/qserde && cargo test --features serde-backend test_serde_json_backend 2>&1 | tail -10
  ```
  Expected: FAIL with "cannot find type `SerdeJsonBackend`"

- [ ] **Step 4: Implement SerdeJsonBackend**
  Replace contents of `crates/qserde/src/backends/serde_.rs`:
  ```rust
  //! Serde-based backends

  #[cfg(feature = "serde-json")]
  mod json;
  #[cfg(feature = "serde-json")]
  pub use json::SerdeJsonBackend;

  #[cfg(feature = "serde-cbor")]
  mod cbor;
  #[cfg(feature = "serde-cbor")]
  pub use cbor::SerdeCborBackend;
  ```

  Create `crates/qserde/src/backends/json.rs`:
  ```rust
  //! JSON backend using serde

  use crate::backends::{Backend, Deserialize, Serialize};
  use serde::{de::DeserializeOwned, Serialize};
  use serde_json::Error as JsonError;

  /// Serde JSON backend
  #[derive(Clone, Default)]
  pub struct SerdeJsonBackend;

  impl Backend for SerdeJsonBackend {
      type Error = JsonError;

      fn serialize<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>, Self::Error>
      where
          T: Serialize,
      {
          serde_json::to_vec(value)
      }

      fn deserialize<T: ?Sized>(&self, bytes: &[u8]) -> Result<T, Self::Error>
      where
          T: DeserializeOwned,
      {
          serde_json::from_slice(bytes)
      }
  }

  // Implement Serialize/Deserialize for types that implement serde's traits
  impl<T: serde::Serialize + ?Sized> Serialize for T {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error>
      where
          Self: Sized,
      {
          backend.serialize(self)
      }
  }

  impl<T: serde::de::DeserializeOwned + ?Sized> Deserialize for T {
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error>
      where
          Self: Sized,
      {
          backend.deserialize(bytes)
      }
  }
  ```

- [ ] **Step 5: Update Cargo.toml serde feature**
  Change `serde-backend = ["serde", "serde_json"]`

- [ ] **Step 6: Run test**
  ```bash
  cd crates/qserde && cargo test --features serde-backend test_serde_json_backend 2>&1 | tail -10
  ```
  Expected: PASS

- [ ] **Step 7: Commit**
  ```bash
  git add crates/qserde/src/backends/serde_.rs crates/qserde/src/backends/json.rs && git commit -m "feat(qserde): implement SerdeJsonBackend"
  ```

---

## Task 6: Implement BincodeBackend

**Files:**
- Modify: `crates/qserde/src/backends/bincode.rs`

- [ ] **Step 1: Add test**
  Add to `crates/qserde/tests/rkyv_backend.rs`:
  ```rust
  use qserde::backends::{BincodeBackend, Serialize, Deserialize};

  #[test]
  fn test_bincode_backend() {
      let backend = BincodeBackend;
      let value = "hello".to_string();
      let bytes = backend.serialize(&value).expect("should serialize");
      let restored: String = backend.deserialize(&bytes).expect("should deserialize");
      assert_eq!(restored, value);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cd crates/qserde && cargo test --features bincode-backend test_bincode_backend 2>&1 | tail -10
  ```
  Expected: FAIL

- [ ] **Step 3: Implement BincodeBackend**
  Replace contents of `crates/qserde/src/backends/bincode.rs`:
  ```rust
  //! Bincode backend

  use crate::backends::{Backend, Deserialize, Serialize};
  use bincode::Options;

  /// Bincode backend - compact binary format
  #[derive(Clone, Default)]
  pub struct BincodeBackend;

  impl Backend for BincodeBackend {
      type Error = bincode::error::EncodeError;

      fn serialize<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>, Self::Error>
      where
          T: Serialize,
      {
          bincode::serde::encode_to_vec(value, bincode::config::standard())
      }

      fn deserialize<T: ?Sized>(&self, bytes: &[u8]) -> Result<T, Self::Error>
      where
          T: Deserialize,
      {
          bincode::serde::decode_from_slice(bytes).map(|(t, _)| t)
      }
  }
  ```

- [ ] **Step 4: Run test**
  ```bash
  cd crates/qserde && cargo test --features bincode-backend test_bincode_backend 2>&1 | tail -10
  ```
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add crates/qserde/src/backends/bincode.rs && git commit -m "feat(qserde): implement BincodeBackend"
  ```

---

## Task 7: Implement CborBackend

**Files:**
- Modify: `crates/qserde/src/backends/cbor.rs`

- [ ] **Step 1: Add test**
  Add to test file:
  ```rust
  use qserde::backends::{CborBackend, Serialize, Deserialize};

  #[test]
  fn test_cbor_backend() {
      let backend = CborBackend;
      let value = "hello".to_string();
      let bytes = backend.serialize(&value).expect("should serialize");
      let restored: String = backend.deserialize(&bytes).expect("should deserialize");
      assert_eq!(restored, value);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cd crates/qserde && cargo test --features cbor-backend test_cbor_backend 2>&1 | tail -10
  ```
  Expected: FAIL

- [ ] **Step 3: Implement CborBackend**
  Replace contents of `crates/qserde/src/backends/cbor.rs`:
  ```rust
  //! CBOR backend (direct, not via serde)

  use crate::backends::{Backend, Deserialize, Serialize};
  use cbor::Encoder;

  /// CBOR backend
  #[derive(Clone, Default)]
  pub struct CborBackend;

  impl Backend for CborBackend {
      type Error = cbor::CborError;

      fn serialize<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>, Self::Error>
      where
          T: Serialize,
      {
          let mut bytes = Vec::new();
          {
              let mut encoder = Encoder::new(&mut bytes);
              encoder.encode(value)?;
          }
          Ok(bytes)
      }

      fn deserialize<T: ?Sized>(&self, bytes: &[u8]) -> Result<T, Self::Error>
      where
          T: Deserialize,
      {
          use cbor::Decoder;
          let mut decoder = Decoder::new(bytes);
          decoder.decode()
      }
  }
  ```

- [ ] **Step 4: Run test**
  ```bash
  cd crates/qserde && cargo test --features cbor-backend test_cbor_backend 2>&1 | tail -10
  ```
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add crates/qserde/src/backends/cbor.rs && git commit -m "feat(qserde): implement CborBackend"
  ```

---

## Task 8: Implement PostcardBackend

**Files:**
- Modify: `crates/qserde/src/backends/postcard.rs`

- [ ] **Step 1: Add test**
  Add to test file:
  ```rust
  use qserde::backends::{PostcardBackend, Serialize, Deserialize};

  #[test]
  fn test_postcard_backend() {
      let backend = PostcardBackend;
      let value = "hello".to_string();
      let bytes = backend.serialize(&value).expect("should serialize");
      let restored: String = backend.deserialize(&bytes).expect("should deserialize");
      assert_eq!(restored, value);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**
  ```bash
  cd crates/qserde && cargo test --features postcard-backend test_postcard_backend 2>&1 | tail -10
  ```
  Expected: FAIL

- [ ] **Step 3: Implement PostcardBackend**
  Replace contents of `crates/qserde/src/backends/postcard.rs`:
  ```rust
  //! Postcard backend - optimized for no_std/embedded

  use crate::backends::{Backend, Deserialize, Serialize};

  /// Postcard backend for no_std/embedded
  #[derive(Clone, Default)]
  pub struct PostcardBackend;

  impl Backend for PostcardBackend {
      type Error = postcard::Error;

      fn serialize<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>, Self::Error>
      where
          T: Serialize,
      {
          postcard::to_vec(value)
      }

      fn deserialize<T: ?Sized>(&self, bytes: &[u8]) -> Result<T, Self::Error>
      where
          T: Deserialize,
      {
          postcard::from_bytes(bytes)
      }
  }
  ```

- [ ] **Step 4: Run test**
  ```bash
  cd crates/qserde && cargo test --features postcard-backend test_postcard_backend 2>&1 | tail -10
  ```
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add crates/qserde/src/backends/postcard.rs && git commit -m "feat(qserde): implement PostcardBackend"
  ```

---

## Task 9: Update lib.rs to export new modules

**Files:**
- Modify: `crates/qserde/src/lib.rs`

- [ ] **Step 1: Add module declarations**
  Add after existing `mod ergonomic;`:
  ```rust
  pub mod backends;
  pub mod error;
  ```

- [ ] **Step 2: Update prelude**
  Modify `pub mod prelude` to include new exports:
  ```rust
  pub mod prelude {
      pub use crate::backends::{
          Backend, BincodeBackend, CborBackend, Deserialize, DeserializeExt,
          PostcardBackend, Serialize, SerializeExt, SerdeJsonBackend, RkyvBackend,
      };
      pub use crate::error::{QserdeError, Result};
      pub use crate::ergonomic::{DeserializeExt2, SerializeExt};
  }
  ```

- [ ] **Step 3: Build to verify**
  ```bash
  cd crates/qserde && cargo build --features all-backends
  ```
  Expected: PASS

- [ ] **Step 4: Commit**
  ```bash
  git add crates/qserde/src/lib.rs && git commit -m "feat(qserde): export new backends and error modules"
  ```

---

## Task 10: Update derive macro to generate Backend-compatible impls

**Files:**
- Modify: `crates/qserde_derive/src/lib.rs`

- [ ] **Step 1: Test current derive behavior**
  ```bash
  cd crates/qserde && cargo test test_derive 2>&1 | tail -10
  ```
  Check what the current `#[qserde::Archive]` generates

- [ ] **Step 2: Modify derive to generate Backend-compatible impls**

  The derive macro should generate code like:
  ```rust
  impl<T> qserde::backends::Serialize for T
  where
      T: SomeSerializeBound,
  {
      fn serialize<B: qserde::backends::Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error> {
          backend.serialize(self)
      }
  }

  impl<T> qserde::backends::Deserialize for T
  where
      T: SomeDeserializeBound,
  {
      fn deserialize<B: qserde::backends::Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error> {
          backend.deserialize(bytes)
      }
  }
  ```

- [ ] **Step 3: Run tests to verify**
  ```bash
  cd crates/qserde && cargo test 2>&1 | tail -10
  ```
  Expected: PASS (all existing tests still work)

- [ ] **Step 4: Commit**
  ```bash
  git add crates/qserde_derive/src/lib.rs && git commit -m "feat(qserde_derive): generate Backend-compatible Serialize/Deserialize impls"
  ```

---

## Task 11: Verify all backends work together

**Files:**
- Test: `crates/qserde/tests/multi_backend.rs`

- [ ] **Step 1: Create integration test**
  ```rust
  use qserde::backends::*;

  #[derive(Debug, PartialEq, Eq)]
  struct TestStruct {
      id: u64,
      name: String,
      active: bool,
  }

  impl Serialize for TestStruct {
      fn serialize<B: Backend>(&self, backend: &B) -> Result<Vec<u8>, B::Error> {
          backend.serialize(self)
      }
  }

  impl Deserialize for TestStruct {
      fn deserialize<B: Backend>(backend: &B, bytes: &[u8]) -> Result<Self, B::Error> {
          backend.deserialize(bytes)
      }
  }

  #[test]
  fn test_all_backends_roundtrip() {
      let value = TestStruct {
          id: 42,
          name: "test".to_string(),
          active: true,
      };

      // Test each backend
      #[cfg(feature = "rkyv-backend")]
      {
          let backend = RkyvBackend;
          let bytes = value.serialize(&backend).unwrap();
          let restored: TestStruct = backend.deserialize(&bytes).unwrap();
          assert_eq!(restored, value);
      }

      #[cfg(feature = "serde-backend")]
      {
          let backend = SerdeJsonBackend;
          let bytes = value.serialize(&backend).unwrap();
          let restored: TestStruct = backend.deserialize(&bytes).unwrap();
          assert_eq!(restored, value);
      }

      #[cfg(feature = "bincode-backend")]
      {
          let backend = BincodeBackend;
          let bytes = value.serialize(&backend).unwrap();
          let restored: TestStruct = backend.deserialize(&bytes).unwrap();
          assert_eq!(restored, value);
      }
  }
  ```

- [ ] **Step 2: Run all backend tests**
  ```bash
  cd crates/qserde && cargo test --features all-backends 2>&1 | tail -20
  ```
  Expected: PASS

- [ ] **Step 3: Commit**
  ```bash
  git add crates/qserde/tests/ && git commit -m "test(qserde): add multi-backend integration tests"
  ```

---

## Task 12: Backward compatibility check

- [ ] **Step 1: Run existing tests**
  ```bash
  cd crates/qserde && cargo test --all-features 2>&1 | tail -20
  ```
  Expected: PASS (all existing tests still work)

- [ ] **Step 2: Verify old API works**
  ```rust
  // This should still work:
  use qserde::prelude::*;
  #[qserde::Archive]
  struct OldStyle { id: u64 }

  let value = OldStyle { id: 1 };
  let bytes = value.dump()?;
  let restored = bytes.load::<OldStyle>()?;
  ```

- [ ] **Step 3: Commit any compatibility fixes**
  ```bash
  git add -A && git commit -m "fix(qserde): ensure backward compatibility"
  ```

---

## Summary

This plan implements a multi-backend serialization system with:

1. **Task 1-2**: Dependencies and error handling
2. **Task 3**: Backend trait definition
3. **Tasks 4-8**: Individual backend implementations
4. **Task 9**: Module exports
5. **Task 10**: Derive macro updates
6. **Tasks 11-12**: Testing and compatibility

Each task produces working, testable code with frequent commits.