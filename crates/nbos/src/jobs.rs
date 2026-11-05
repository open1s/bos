//! Background jobs: the live roster a shell draws and a tool reads.

use std::sync::Arc;

use crate::utils::json_to_py;
use agent::jobs::{JobOutput, JobStatus, JobSummary};
use pyo3::prelude::*;

fn summary_json(summary: &JobSummary) -> serde_json::Value {
    serde_json::json!({
        "id": summary.id,
        "session": summary.session,
        "tool": summary.tool,
        "command": summary.command,
        "status": summary.status.as_str(),
        "pid": summary.pid,
        "duration_ms": summary.duration_ms,
        "output_len": summary.output_len,
        "output_dropped": summary.output_dropped,
        "running": summary.is_running(),
        "settled": status_settled(summary.status),
        "exit_code": summary.status.exit_code(),
    })
}

fn status_settled(status: JobStatus) -> bool {
    status.is_settled()
}

fn output_json(output: &JobOutput) -> serde_json::Value {
    serde_json::json!({
        "text": output.text,
        "bytes": output.bytes,
        "dropped": output.dropped,
        "truncated": output.truncated(),
    })
}

/// The live roster of background jobs.
///
/// Share one instance between the bash tool that spawns jobs, the job tools
/// that read them, and the host that draws the list.
#[pyclass(name = "JobRegistry", skip_from_py_object)]
#[derive(Clone)]
pub struct PyJobRegistry {
    pub(crate) inner: Arc<agent::jobs::JobRegistry>,
}

#[pymethods]
impl PyJobRegistry {
    #[new]
    #[pyo3(signature = (job_cap = None, output_cap = None))]
    /// Create a roster with bounded job and output capacities.
    fn new(job_cap: Option<usize>, output_cap: Option<usize>) -> Self {
        let inner = match (job_cap, output_cap) {
            (Some(jobs), Some(bytes)) => agent::jobs::JobRegistry::with_capacity(jobs, bytes),
            (Some(jobs), None) => {
                agent::jobs::JobRegistry::with_capacity(jobs, agent::jobs::DEFAULT_OUTPUT_CAP)
            }
            (None, Some(bytes)) => {
                agent::jobs::JobRegistry::with_capacity(agent::jobs::DEFAULT_JOB_CAP, bytes)
            }
            (None, None) => agent::jobs::JobRegistry::new(),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Every job started by session, oldest first.
    fn list(&self, py: Python<'_>, session: String) -> PyResult<Py<PyAny>> {
        let jobs: Vec<serde_json::Value> = self
            .inner
            .list_for_session(&session)
            .iter()
            .map(summary_json)
            .collect();
        json_to_py(py, &serde_json::Value::Array(jobs))
    }

    /// One job's metadata, or None when the roster no longer holds it.
    fn get(&self, py: Python<'_>, id: String) -> PyResult<Py<PyAny>> {
        let value = self
            .inner
            .job(&id)
            .map(|job| summary_json(&job.summary(std::time::Instant::now())))
            .unwrap_or(serde_json::Value::Null);
        json_to_py(py, &value)
    }

    /// The captured output of one job, or None when it is not on the roster.
    #[pyo3(signature = (id, max_bytes = None))]
    fn output(&self, py: Python<'_>, id: String, max_bytes: Option<usize>) -> PyResult<Py<PyAny>> {
        let value = self
            .inner
            .output_tail(&id, max_bytes)
            .map(|output| output_json(&output))
            .unwrap_or(serde_json::Value::Null);
        json_to_py(py, &value)
    }

    /// Stop a running job; False when it already settled or is unknown.
    fn kill(&self, id: String) -> bool {
        self.inner.kill(&id)
    }

    /// Wait up to timeout_ms for a job to settle, then return its metadata.
    #[pyo3(signature = (id, timeout_ms = 0))]
    fn wait(&self, py: Python<'_>, id: String, timeout_ms: u64) -> PyResult<Py<PyAny>> {
        let registry = self.inner.clone();
        let summary =
            py.detach(move || registry.wait_for(&id, std::time::Duration::from_millis(timeout_ms)));
        let value = summary
            .as_ref()
            .map(summary_json)
            .unwrap_or(serde_json::Value::Null);
        json_to_py(py, &value)
    }

    /// Number of jobs from session that are still running.
    fn running_count(&self, session: String) -> u32 {
        self.inner.running_count(&session).min(u32::MAX as usize) as u32
    }

    /// Number of settled jobs from session.
    fn settled_count(&self, session: String) -> u32 {
        self.inner.settled_count(&session).min(u32::MAX as usize) as u32
    }

    /// Drop every settled job from session; returns how many were dropped.
    fn clear_settled(&self, session: String) -> u32 {
        self.inner.clear_settled(&session).min(u32::MAX as usize) as u32
    }

    /// A counter that changes whenever the roster changes.
    fn version(&self) -> u64 {
        self.inner.version()
    }
}
