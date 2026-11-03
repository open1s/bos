//! Runtime-neutral invocation contract, re-exported from bos-ext-runtime.
pub(crate) use bos_ext_runtime::contract::*;

pub(crate) mod builtin {
    pub(crate) use bos_ext_runtime::builtin::*;
}

pub(crate) mod cordis;
pub(crate) mod declarative;
pub(crate) mod extism;
pub(crate) mod mcp;
pub(crate) mod process;
pub(crate) mod wasm;
pub(crate) mod workflow;
