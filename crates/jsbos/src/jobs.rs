//! Background jobs: the live roster a shell draws and a tool reads.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use agent::jobs::{JobOutput, JobStatus, JobSummary};

fn status_name(status: JobStatus) -> &'static str {
  status.as_str()
}

fn summary_json(summary: &JobSummary) -> serde_json::Value {
  serde_json::json!({
      "id": summary.id,
      "session": summary.session,
      "tool": summary.tool,
      "command": summary.command,
      "status": status_name(summary.status),
      "pid": summary.pid,
      "duration_ms": summary.duration_ms,
      "output_len": summary.output_len,
      "output_dropped": summary.output_dropped,
      "running": summary.is_running(),
      "settled": summary.status.is_settled(),
      "exit_code": summary.status.exit_code(),
  })
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
#[napi]
pub struct JobRegistry {
  pub(crate) inner: Arc<agent::jobs::JobRegistry>,
}

#[napi]
impl JobRegistry {
  #[napi(constructor)]
  /// Create a roster with bounded job and output capacities.
  pub fn new(job_cap: Option<u32>, output_cap: Option<u32>) -> Self {
    let inner = match (job_cap, output_cap) {
      (Some(jobs), Some(bytes)) => {
        agent::jobs::JobRegistry::with_capacity(jobs as usize, bytes as usize)
      }
      (Some(jobs), None) => {
        agent::jobs::JobRegistry::with_capacity(jobs as usize, agent::jobs::DEFAULT_OUTPUT_CAP)
      }
      (None, Some(bytes)) => {
        agent::jobs::JobRegistry::with_capacity(agent::jobs::DEFAULT_JOB_CAP, bytes as usize)
      }
      (None, None) => agent::jobs::JobRegistry::new(),
    };
    Self {
      inner: Arc::new(inner),
    }
  }

  #[napi]
  /// Every job started by `session`, oldest first.
  pub fn list(&self, session: String) -> Vec<serde_json::Value> {
    self
      .inner
      .list_for_session(&session)
      .iter()
      .map(summary_json)
      .collect()
  }

  #[napi]
  /// One job's metadata, or null when the roster no longer holds it.
  pub fn get(&self, id: String) -> Option<serde_json::Value> {
    self
      .inner
      .job(&id)
      .map(|job| summary_json(&job.summary(std::time::Instant::now())))
  }

  #[napi]
  /// The captured output of one job, or null when it is not on the roster.
  pub fn output(&self, id: String, max_bytes: Option<u32>) -> Option<serde_json::Value> {
    self
      .inner
      .output_tail(&id, max_bytes.map(|bytes| bytes as usize))
      .map(|output| output_json(&output))
  }

  #[napi]
  /// Stop a running job; false when it already settled or is unknown.
  pub fn kill(&self, id: String) -> bool {
    self.inner.kill(&id)
  }

  #[napi]
  /// Wait up to `timeout_ms` for a job to settle, then return its metadata.
  pub async fn wait(&self, id: String, timeout_ms: i64) -> Result<Option<serde_json::Value>> {
    let timeout = crate::millis_from_js("timeout_ms", timeout_ms)?;
    let registry = self.inner.clone();
    let summary = tokio::task::spawn_blocking(move || registry.wait_for(&id, timeout))
      .await
      .map_err(|error| {
        Error::new(
          napi::Status::GenericFailure,
          format!("job wait failed: {error}"),
        )
      })?;
    Ok(summary.as_ref().map(summary_json))
  }

  #[napi]
  /// Number of jobs from `session` that are still running.
  pub fn running_count(&self, session: String) -> u32 {
    self.inner.running_count(&session).min(u32::MAX as usize) as u32
  }

  #[napi]
  /// Number of settled jobs from `session`.
  pub fn settled_count(&self, session: String) -> u32 {
    self.inner.settled_count(&session).min(u32::MAX as usize) as u32
  }

  #[napi]
  /// Drop every settled job from `session`; returns how many were dropped.
  pub fn clear_settled(&self, session: String) -> u32 {
    self.inner.clear_settled(&session).min(u32::MAX as usize) as u32
  }

  #[napi]
  /// A counter that changes whenever the roster changes.
  pub fn version(&self) -> i64 {
    self.inner.version().min(i64::MAX as u64) as i64
  }
}
