//! Tools that read and stop background jobs.
//!
//! Both tools are gated by the same policy principal as the shell that started
//! the job: the session must match, and the recorded working directory is the
//! target a policy re-checks. A job started in another session is not merely
//! hidden from the caller, it is refused - the registry is shared process-wide,
//! so ownership has to be checked rather than assumed.

use std::sync::Arc;

use crate::jobs::{JobRegistry, JobStatus};
use crate::policy::{Action, PolicySet, Target};
use crate::tools::bash::BashPolicy;
use react::tool::{Tool, ToolError};
use serde_json::Value;

/// How long job_output may wait for a job to settle.
///
/// The cap keeps a turn from parking on a command that never ends; the caller
/// gets the partial output and a running status instead.
pub const MAX_WAIT_MS: u64 = 30_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Output,
    Kill,
}

/// One of the two job tools, sharing the registry and principal of the shell.
pub struct JobTool {
    name: String,
    kind: Kind,
    policy: Option<BashPolicy>,
    jobs: Option<Arc<JobRegistry>>,
    session: String,
}

impl JobTool {
    /// The output reader, which takes an id and an optional wait.
    pub fn reader(name: &str) -> Self {
        Self::new(name, Kind::Output)
    }

    /// The stop control, which takes an id.
    pub fn stopper(name: &str) -> Self {
        Self::new(name, Kind::Kill)
    }

    fn new(name: &str, kind: Kind) -> Self {
        Self {
            name: name.to_string(),
            kind,
            policy: None,
            jobs: None,
            session: String::new(),
        }
    }

    /// Gate the tool through the turn policy. The session is the principal a
    /// job must belong to.
    pub fn with_policy(
        mut self,
        policy: Arc<PolicySet>,
        session: impl Into<String>,
        preset: impl Into<String>,
    ) -> Self {
        self.session = session.into();
        self.policy = Some(BashPolicy::new(policy, self.session.clone(), preset));
        self
    }

    /// Point the tool at the live roster and the session that owns its jobs.
    pub fn with_jobs(mut self, jobs: Arc<JobRegistry>, session: impl Into<String>) -> Self {
        self.session = session.into();
        self.jobs = Some(jobs);
        self
    }

    fn registry(&self) -> Result<&Arc<JobRegistry>, ToolError> {
        self.jobs.as_ref().ok_or_else(|| {
            ToolError::Failed("background jobs are not configured for this tool".to_string())
        })
    }

    /// One job the caller is allowed to address.
    fn owned(
        &self,
        id: &str,
    ) -> Result<(crate::jobs::BackgroundJob, &Arc<JobRegistry>), ToolError> {
        let registry = self.registry()?;
        let job = registry
            .job(id)
            .ok_or_else(|| ToolError::Failed(format!("no such background job: {id}")))?;
        if job.session != self.session {
            // The same refusal as an unknown id: a session must not be able to
            // enumerate another session's jobs through error text.
            return Err(ToolError::Failed(format!("no such background job: {id}")));
        }
        Ok((job, registry))
    }

    fn id(input: &Value) -> Result<&str, ToolError> {
        input
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::Failed("Missing required field: id".to_string()))
    }

    fn gate(&self, action: Action, job: &crate::jobs::BackgroundJob) -> Result<(), ToolError> {
        if let Some(policy) = &self.policy {
            policy.check(&self.name, action, Target::path(job.cwd.clone()))?;
        }
        Ok(())
    }

    fn read_output(&self, input: &Value) -> Result<Value, ToolError> {
        let id = Self::id(input)?;
        let (job, registry) = self.owned(id)?;
        self.gate(Action::Read, &job)?;
        let wait_ms = input
            .get("wait_ms")
            .and_then(|value| value.as_u64())
            .unwrap_or(0)
            .min(MAX_WAIT_MS);
        let job = registry
            .wait_for(id, std::time::Duration::from_millis(wait_ms))
            .ok_or_else(|| ToolError::Failed(format!("no such background job: {id}")))?;
        let output = registry.output_for(id).unwrap_or(crate::jobs::JobOutput {
            text: String::new(),
            bytes: 0,
            dropped: 0,
        });
        Ok(serde_json::json!({
            "id": job.id,
            "status": job.status.as_str(),
            "exit_code": job.status.exit_code(),
            "running": job.is_running(),
            "duration_ms": job.duration_ms,
            "output": output.text,
            "output_bytes": output.bytes,
            "output_truncated": output.dropped > 0,
        }))
    }

    fn stop(&self, input: &Value) -> Result<Value, ToolError> {
        let id = Self::id(input)?;
        let (job, registry) = self.owned(id)?;
        self.gate(Action::Spawn, &job)?;
        let killed = registry.kill(id);
        let status = registry
            .job(id)
            .map(|job| job.status)
            .unwrap_or(JobStatus::Killed);
        Ok(serde_json::json!({
            "id": id,
            "killed": killed,
            "running": status.is_running(),
            "status": status.as_str(),
        }))
    }
}

impl Tool for JobTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        match self.kind {
            Kind::Output => "Read a background job's captured output and status. Pass the job id \
                 returned by bash with run_in_background. wait_ms (up to 30000) waits \
                 for the job to settle before answering."
                .to_string(),
            Kind::Kill => "Stop a running background job. Pass the job id returned by bash with \
                 run_in_background. The whole process group is signalled; an already \
                 settled job reports killed: false."
                .to_string(),
        }
    }

    fn category(&self) -> String {
        "system".to_string()
    }

    fn json_schema(&self) -> Value {
        match self.kind {
            Kind::Output => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The job id returned when it was started."
                    },
                    "wait_ms": {
                        "type": "integer",
                        "description": "Wait this many milliseconds (max 30000) for the job to settle.",
                        "minimum": 0
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            Kind::Kill => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The job id returned when it was started."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }

    fn to_openai_definition(&self) -> react::tool::ToolDefinition {
        let id = react::tool::ToolParameterProperty {
            param_type: "string".to_string(),
            description: Some("The job id returned when it was started.".to_string()),
            enum_values: None,
        };
        let mut properties = std::collections::HashMap::new();
        properties.insert("id".to_string(), id);
        if self.kind == Kind::Output {
            properties.insert(
                "wait_ms".to_string(),
                react::tool::ToolParameterProperty {
                    param_type: "integer".to_string(),
                    description: Some(
                        "Wait this many milliseconds (max 30000) for the job to settle."
                            .to_string(),
                    ),
                    enum_values: None,
                },
            );
        }
        react::tool::ToolDefinition::new(self.name(), self.description()).with_parameters(
            react::tool::ToolParameters {
                param_type: "object".to_string(),
                properties: Some(properties),
                required: Some(vec!["id".to_string()]),
            },
        )
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        match self.kind {
            Kind::Output => self.read_output(input),
            Kind::Kill => self.stop(input),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{PolicyData, PolicySet};

    #[test]
    fn an_unknown_id_is_refused() {
        let tool =
            JobTool::reader("job_output").with_jobs(Arc::new(JobRegistry::new()), "session-a");
        match tool.run(&serde_json::json!({ "id": "nope" })) {
            Err(ToolError::Failed(message)) => assert!(message.contains("no such")),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn another_session_cannot_read_or_kill_a_job() {
        let registry = Arc::new(JobRegistry::new());
        let id = registry.begin("session-a", "bash", "sleep 5");
        let read = JobTool::reader("job_output").with_jobs(registry.clone(), "session-b");
        assert!(
            read.run(&serde_json::json!({ "id": id })).is_err(),
            "a different session must not read it"
        );
        let kill = JobTool::stopper("job_kill").with_jobs(registry.clone(), "session-b");
        assert!(kill.run(&serde_json::json!({ "id": id })).is_err());
        assert_eq!(
            registry.job(&id).expect("job").status,
            JobStatus::Running,
            "the refused kill changed nothing"
        );
    }

    #[test]
    fn the_policy_gate_runs_before_the_roster_is_touched() {
        use crate::policy::PolicySource;
        let root = tempfile::tempdir().expect("workspace");
        let outside = root.path().parent().expect("parent").join("elsewhere");
        let registry = Arc::new(JobRegistry::new());
        let id = registry.begin_in("session-a", "bash", "sleep 5", &outside);
        // The baseline permits controlling a job this session owns; this
        // workspace module narrows that back out, and the refusal must happen
        // before the kill.
        let deny_kill = PolicySource::new(
            "workspace",
            r#"package policy.workspace

# rule: no-job-kill
verdict := {"allow": false, "rule": "no-job-kill", "reason": "jobs may not be stopped here"} if {
    input.action == "spawn"
    input.tool == "job_kill"
} else := {"allow": true, "rule": "workspace-allow"}
"#,
        );
        let policy = Arc::new(PolicySet::compile(
            [crate::policy::baseline_source(), deny_kill],
            PolicyData::new(root.path()),
        ));
        let kill = JobTool::stopper("job_kill")
            .with_policy(policy, "session-a", "preset")
            .with_jobs(registry.clone(), "session-a");
        match kill.run(&serde_json::json!({ "id": id })) {
            Err(ToolError::Failed(message)) => {
                assert!(message.contains("no-job-kill"), "{message}")
            }
            other => panic!("expected a spawn refusal, got {other:?}"),
        }
        assert_eq!(
            registry.job(&id).expect("job").status,
            JobStatus::Running,
            "a denied kill leaves the job alone"
        );
    }

    #[test]
    fn the_output_tool_reports_the_captured_tail() {
        let registry = Arc::new(JobRegistry::new());
        let id = registry.begin("session-a", "bash", "echo hi");
        registry.append_output(&id, b"hi\n");
        registry.finish(&id, JobStatus::Succeeded { code: 0 });
        let tool = JobTool::reader("job_output").with_jobs(registry, "session-a");
        let value = tool
            .run(&serde_json::json!({ "id": id, "wait_ms": 10 }))
            .expect("the output runs");
        assert_eq!(value["status"], serde_json::json!("succeeded"));
        assert_eq!(value["exit_code"], serde_json::json!(0));
        assert_eq!(value["output"], serde_json::json!("hi\n"));
        assert_eq!(value["running"], serde_json::json!(false));
    }

    /// The user-facing promise: a wait that reports a finished job reports the
    /// whole capture. The command exits while a child it started still holds
    /// the pipes and is about to write, so observing the exit status is not the
    /// same fact as having every byte.
    #[test]
    fn a_finished_job_output_is_never_partial() {
        use crate::tools::bash::BashTool;
        let registry = Arc::new(JobRegistry::new());
        let bash = BashTool::new("bash").with_jobs(registry.clone(), "session-a");
        let value = bash
            .run(&serde_json::json!({
                "command": "printf first; sleep 0.002; ( sleep 0.02; printf last; printf err 1>&2 ) & exit 3",
                "run_in_background": true
            }))
            .expect("the spawn is accepted");
        let id = value["job_id"].as_str().expect("a job id").to_string();
        let reader = JobTool::reader("job_output").with_jobs(registry, "session-a");
        let read = reader
            .run(&serde_json::json!({ "id": id, "wait_ms": 5000 }))
            .expect("the output runs");
        assert_eq!(read["status"], serde_json::json!("failed"));
        assert_eq!(read["exit_code"], serde_json::json!(3));
        let output = read["output"].as_str().expect("captured output");
        assert!(
            output.contains("first") && output.contains("last") && output.contains("err"),
            "job_output reported a finished job with a partial capture: {output:?}"
        );
    }

    /// `job_output` asks the policy for a Read before it touches the roster;
    /// a denial names the rule and never returns the captured bytes.
    #[test]
    fn a_read_denial_refuses_before_the_output_is_returned() {
        use crate::policy::PolicySource;
        let root = tempfile::tempdir().expect("workspace");
        let registry = Arc::new(JobRegistry::new());
        let id = registry.begin_in("session-a", "bash", "echo secret-output", root.path());
        registry.append_output(&id, b"secret-output\n");
        let deny_read = PolicySource::new(
            "workspace",
            r#"package policy.workspace

# rule: no-job-output
verdict := {"allow": false, "rule": "no-job-output", "reason": "job output is not readable here"} if {
    input.action == "read"
    input.tool == "job_output"
} else := {"allow": true, "rule": "workspace-allow"}
"#,
        );
        let policy = Arc::new(PolicySet::compile(
            [crate::policy::baseline_source(), deny_read],
            PolicyData::new(root.path()),
        ));
        let tool = JobTool::reader("job_output")
            .with_policy(policy, "session-a", "preset")
            .with_jobs(registry, "session-a");
        match tool.run(&serde_json::json!({ "id": id })) {
            Err(ToolError::Failed(message)) => {
                assert!(message.contains("no-job-output"), "{message}");
                assert!(
                    !message.contains("secret-output"),
                    "the refusal leaked the captured bytes: {message}"
                );
            }
            other => panic!("expected a read refusal, got {other:?}"),
        }
    }
}
