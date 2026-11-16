use pyo3::prelude::*;

#[pymodule]
pub fn py_logging(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(init_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(log_test_message, m)?)?;
    Ok(())
}

#[pyfunction]
pub fn init_tracing() {
    logging::auto_init_tracing();
}

#[pyfunction]
pub fn log_test_message(message: &str) {
    logging::log_test_message(message);
}
