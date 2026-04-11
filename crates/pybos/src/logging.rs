use pyo3::prelude::*;

#[pymodule]
pub fn py_logging(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(init_tracing, m)?)?;
    Ok(())
}

#[pyfunction]
pub fn init_tracing() {
    logging::auto_init_tracing();
}
