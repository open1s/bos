use super::{BackoffStrategy, ConditionType, Step, StepType, Workflow};
use std::time::Duration;

/// Builder for creating workflows with fluent API
pub struct WorkflowBuilder {
    name: String,
    description: String,
    steps: Vec<Step>,
    default_timeout: Duration,
    default_retries: u32,
    default_backoff: BackoffStrategy,
    default_input: Option<serde_json::Value>,
}

impl WorkflowBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            steps: Vec::new(),
            default_timeout: Duration::from_secs(60),
            default_retries: 3,
            default_backoff: BackoffStrategy::Exponential {
                base: Duration::from_millis(100),
                max: Duration::from_secs(10),
            },
            default_input: None,
        }
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    pub fn default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub fn default_retries(mut self, retries: u32) -> Self {
        self.default_retries = retries;
        self
    }

    pub fn with_default_input(mut self, input: serde_json::Value) -> Self {
        self.default_input = Some(input);
        self
    }

    pub fn add_step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    pub fn sequential_step(
        mut self,
        name: String,
        agent_id: Option<String>,
        _input: serde_json::Value,
    ) -> Self {
        self.steps.push(Step {
            name,
            step_type: StepType::Sequential,
            agent_id,
            timeout: self.default_timeout,
            max_retries: self.default_retries,
            backoff: self.default_backoff.clone(),
        });
        self
    }

    pub fn parallel_group(
        mut self,
        _group_name: String,
        steps: Vec<(String, Option<String>, serde_json::Value)>,
    ) -> Self {
        for (name, agent_id, _input) in steps {
            self.steps.push(Step {
                name,
                step_type: StepType::Parallel,
                agent_id,
                timeout: self.default_timeout,
                max_retries: self.default_retries,
                backoff: self.default_backoff.clone(),
            });
        }
        self
    }

    pub fn branch(
        mut self,
        name: String,
        condition: ConditionType,
        _true_branch: String,
        _false_branch: String,
    ) -> Self {
        self.steps.push(Step {
            name,
            step_type: StepType::Conditional { condition },
            agent_id: None,
            timeout: self.default_timeout,
            max_retries: 0,
            backoff: BackoffStrategy::Fixed {
                interval: Duration::ZERO,
            },
        });
        self
    }

    pub fn build(self) -> Workflow {
        Workflow {
            name: self.name,
            description: self.description,
            steps: self.steps,
            default_timeout: self.default_timeout,
            default_retries: self.default_retries,
        }
    }
}

pub struct StepBuilder {
    name: String,
    agent_id: Option<String>,
    step_type: StepType,
    timeout: Duration,
    max_retries: u32,
    backoff: BackoffStrategy,
}

impl StepBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            agent_id: None,
            step_type: StepType::Sequential,
            timeout: Duration::from_secs(60),
            max_retries: 3,
            backoff: BackoffStrategy::Exponential {
                base: Duration::from_millis(100),
                max: Duration::from_secs(10),
            },
        }
    }

    pub fn name(name: impl Into<String>) -> Self {
        Self::new(name)
    }

    pub fn sequential(mut self) -> Self {
        self.step_type = StepType::Sequential;
        self
    }

    pub fn parallel(mut self) -> Self {
        self.step_type = StepType::Parallel;
        self
    }

    pub fn conditional(mut self, condition: ConditionType) -> Self {
        self.step_type = StepType::Conditional { condition };
        self
    }

    pub fn local(mut self) -> Self {
        self.agent_id = None;
        self
    }

    pub fn remote(mut self, agent_id: String) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = duration;
        self
    }

    pub fn retries(mut self, count: u32) -> Self {
        self.max_retries = count;
        self
    }

    pub fn backoff(mut self, strategy: BackoffStrategy) -> Self {
        self.backoff = strategy;
        self
    }

    pub fn build(self) -> Step {
        Step {
            name: self.name,
            step_type: self.step_type,
            agent_id: self.agent_id,
            timeout: self.timeout,
            max_retries: self.max_retries,
            backoff: self.backoff,
        }
    }
}
