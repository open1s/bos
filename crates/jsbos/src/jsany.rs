use napi::bindgen_prelude::*;

/// A wrapper type that represents any JavaScript value.
/// Used with ThreadsafeFunction to accept/return arbitrary JS values.
#[derive(Clone)]
pub struct JSAny(pub serde_json::Value);

impl ToNapiValue for JSAny {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
        <serde_json::Value as ToNapiValue>::to_napi_value(env, val.0)
    }
}

impl FromNapiValue for JSAny {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
        let value = <serde_json::Value as FromNapiValue>::from_napi_value(env, napi_val)?;
        Ok(JSAny(value))
    }
}
