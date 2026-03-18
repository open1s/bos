//! RPC (Remote Procedure Call) module
//!
//! Provides typed request/response communication over Zenoh.
//!
//! # Example
//! ```rust,ignore
//! use brickos_bus::{RpcClient, RpcClientBuilder, RpcResponse};
//!
//! // Using builder pattern
//! let client = RpcClient::builder()
//!     .service("calculator")
//!     .method("add")
//!     .timeout(Duration::from_secs(5))
//!     .build()?;
//!
//! // Or using new()
//! let client = RpcClient::new("calculator".to_string(), "add".to_string());
//!
//! // Initialize with session
//! client.init(session).await?;
//!
//! // Call the service
//! let result: i32 = client.call(&[1, 2]).await?;
//! ```

pub mod client;
pub mod error;
pub mod types;

pub use client::{RpcClient, RpcClientBuilder};

pub use client::{RpcClient, RpcClientBuilder};
pub use error::{RpcError, RpcServiceError};
pub use types::RpcResponse;
