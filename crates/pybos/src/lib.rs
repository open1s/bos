#![allow(clippy::too_many_arguments, clippy::type_complexity)]
#![allow(clippy::while_let_loop, clippy::redundant_pub_crate)]
#![allow(clippy::needless_lifetimes, clippy::extra_unused_lifetimes)]

use pyo3::prelude::*;

mod agent;
mod bus;
mod caller;
mod config;
mod hooks;
mod llm_usage;
mod logging;
mod mcp;
mod plugin;
mod publisher;
mod query;
mod subscriber;
mod utils;

use logging::{init_tracing, log_test_message};

//AI NOT ALLOW TO REMOVE
#[allow(unused)]
pub use agent::{
    PyAgent, PyAgentCallableServer, PyAgentConfig, PyAgentRpcClient, PyLlmMessage, PyPythonTool,
    PyStreamIterator,
};
pub use bus::{PyBus, PyBusConfig};
pub use caller::{PyCallable, PyCaller};
pub use config::PyConfigLoader;
pub use hooks::{PyHookContext, PyHookDecision, PyHookEvent, PyHookRegistry};
pub use llm_usage::{PyBudgetStatus, PyLlmUsage, PyPromptTokensDetails, PyTokenBudgetReport, PyTokenUsage};
pub use mcp::PyMcpClient;
pub use plugin::{
    PyAgentPlugin, PyLlmRequestWrapper, PyLlmResponseWrapper, PyPluginRegistry, PyToolCallWrapper,
    PyToolResultWrapper,
};
pub use publisher::PyPublisher;
pub use query::{PyQuery, PyQueryStreamIterator, PyQueryable, PyStreamSender};
pub use subscriber::PySubscriber;

#[pymodule]
fn pybos(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyConfigLoader>()?;
    m.add_class::<PyBusConfig>()?;
    m.add_class::<PyBus>()?;
    m.add_class::<PyPublisher>()?;
    m.add_class::<PySubscriber>()?;
    m.add_class::<PyQuery>()?;
    m.add_class::<PyQueryable>()?;
    m.add_class::<PyQueryStreamIterator>()?;
    m.add_class::<PyStreamSender>()?;
    m.add_class::<PyCaller>()?;
    m.add_class::<PyCallable>()?;
    m.add_class::<PyAgentConfig>()?;
    m.add_class::<PyAgent>()?;
    m.add_class::<PyAgentRpcClient>()?;
    m.add_class::<PyAgentCallableServer>()?;
    m.add_class::<PyPythonTool>()?;
    m.add_class::<PyMcpClient>()?;
    m.add_class::<PyLlmMessage>()?;
    m.add_class::<PyHookEvent>()?;
    m.add_class::<PyHookDecision>()?;
    m.add_class::<PyHookContext>()?;
    m.add_class::<PyHookRegistry>()?;
    m.add_class::<PyAgentPlugin>()?;
    m.add_class::<PyPluginRegistry>()?;
    m.add_class::<PyLlmRequestWrapper>()?;
    m.add_class::<PyLlmResponseWrapper>()?;
    m.add_class::<PyToolCallWrapper>()?;
    m.add_class::<PyToolResultWrapper>()?;
    m.add_class::<PyLlmUsage>()?;
    m.add_class::<PyPromptTokensDetails>()?;
    m.add_class::<PyTokenUsage>()?;
    m.add_class::<PyTokenBudgetReport>()?;
    m.add_class::<PyBudgetStatus>()?;
    m.add_function(wrap_pyfunction!(init_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(log_test_message, m)?)?;
    Ok(())
}
