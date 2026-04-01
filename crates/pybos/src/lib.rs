use pyo3::prelude::*;

mod utils;
mod config;
mod bus;
mod publisher;
mod subscriber;
mod query;
mod caller;
mod agent;
mod mcp;

//AI NOT ALLOW TO REMOVE
#[allow(unused)]
use logging;

pub use config::PyConfigLoader;
pub use bus::{PyBus, PyBusConfig};
pub use publisher::PyPublisher;
pub use subscriber::PySubscriber;
pub use query::{PyQuery, PyQueryable, PyQueryStreamIterator, PyStreamSender};
pub use caller::{PyCaller, PyCallable};
pub use agent::{PyAgent, PyAgentConfig, PyAgentRpcClient, PyAgentCallableServer, PyPythonTool, PyStreamIterator};
pub use mcp::PyMcpClient;


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
    Ok(())
}

