//! RPC type definitions

use serde::{Deserialize, Serialize};

/// Response envelope wrapping all RPC responses.
///
/// Carries either a success value or a service-side error.
/// This is the standard response format for all RPC calls.
///
/// # Example
/// ```rust,ignore
/// // Success response
/// let response: RpcResponse<i32> = RpcResponse::ok(42);
///
/// // Error response
/// let response: RpcResponse<i32> = RpcResponse::err(404, "Not found");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RpcResponse<T> {
    Ok(T),
    Err { code: u32, message: String },
}

impl<T> RpcResponse<T> {
    /// Create a successful response.
    pub fn ok(value: T) -> Self {
        RpcResponse::Ok(value)
    }

    /// Create an error response.
    pub fn err(code: u32, message: impl Into<String>) -> Self {
        RpcResponse::Err {
            code,
            message: message.into(),
        }
    }

    /// Check if the response is successful.
    pub fn is_ok(&self) -> bool {
        matches!(self, RpcResponse::Ok(_))
    }

    /// Check if the response is an error.
    pub fn is_err(&self) -> bool {
        matches!(self, RpcResponse::Err { .. })
    }

    /// Convert the response into a Result.
    ///
    /// Returns `Ok(T)` for success responses, or `Err((code, message))`
    /// for error responses.
    pub fn into_result(self) -> Result<T, (u32, String)> {
        match self {
            RpcResponse::Ok(v) => Ok(v),
            RpcResponse::Err { code, message } => Err((code, message)),
        }
    }
}
