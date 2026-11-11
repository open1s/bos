//! Offline Skill-activation benchmark.
//!
//! The harness answers the effectiveness question without a model or network:
//! each case says which Skill should be selected and which directive terms a
//! correct answer needs. A deterministic baseline answers with no Skill
//! instructions; the activated path runs the real plan/activate/compose
//! contract. The difference is the measured value of activation, and the byte
//! count is its context cost.

use super::index::SkillIndex;
use super::runtime::{retrieval_metrics, RetrievalMetrics, SkillLimits, SkillRuntime};
use super::{SkillContent, SkillPermission, TrustPolicy};

/// The score below which the simulated planner decides no Skill is needed.
pub const NO_SKILL_THRESHOLD: i64 = 12;

/// One benchmark task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkCase {
    /// What the user asked for.
    pub task: String,
    /// The Skill a correct planner selects; None means "no Skill".
    pub expected: Option<String>,
    /// Directive terms the assembled context must contain to count as success.
    pub required_terms: Vec<String>,
}

impl BenchmarkCase {
    /// A task that needs a Skill.
    pub fn skill(task: impl Into<String>, expected: impl Into<String>, terms: &[&str]) -> Self {
        Self {
            task: task.into(),
            expected: Some(expected.into()),
            required_terms: terms.iter().map(|term| term.to_string()).collect(),
        }
    }

    /// A task that needs no Skill.
    pub fn no_skill(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            expected: None,
            required_terms: Vec::new(),
        }
    }
}

/// One case's two outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseOutcome {
    /// Did the simulated answer contain every required directive?
    pub success: bool,
    /// The Skill the planner selected, if any.
    pub selected: Option<String>,
    /// Whether retrieval put the expected Skill in the top 3.
    pub retrieval_hit: bool,
    /// Instruction bytes injected for the case.
    pub instruction_bytes: usize,
}

/// A repeatable benchmark result.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkReport {
    /// Activated-path outcomes, case order.
    pub with_skills: Vec<CaseOutcome>,
    /// Baseline outcomes, case order.
    pub without_skills: Vec<CaseOutcome>,
    /// Mean retrieval metrics over cases with an expected Skill.
    pub retrieval: RetrievalMetrics,
    /// Selected-correctly / selected.
    pub activation_precision: f64,
    /// No-Skill cases decided correctly / all No-Skill cases.
    pub no_skill_accuracy: f64,
    /// Total instruction bytes injected by the activated path.
    pub instruction_bytes: usize,
}

impl BenchmarkReport {
    /// Activated-path success rate.
    pub fn with_success_rate(&self) -> f64 {
        rate(&self.with_skills)
    }

    /// Baseline success rate.
    pub fn without_success_rate(&self) -> f64 {
        rate(&self.without_skills)
    }

    /// The measured improvement of activation.
    pub fn improvement(&self) -> f64 {
        self.with_success_rate() - self.without_success_rate()
    }
}

fn rate(outcomes: &[CaseOutcome]) -> f64 {
    if outcomes.is_empty() {
        0.0
    } else {
        outcomes.iter().filter(|outcome| outcome.success).count() as f64 / outcomes.len() as f64
    }
}

fn host_policy() -> TrustPolicy {
    TrustPolicy {
        allowed_sources: [
            super::SkillSource::User,
            super::SkillSource::Builtin,
            super::SkillSource::ThirdParty,
            super::SkillSource::Federated,
        ]
        .into_iter()
        .collect(),
        require_publisher: true,
    }
}

/// Run the benchmark over a Skill set and a case list.
pub fn run_benchmark(
    skills: &[SkillContent],
    cases: &[BenchmarkCase],
    limits: SkillLimits,
) -> Result<BenchmarkReport, String> {
    let index = SkillIndex::from_metadata(skills.iter().map(|skill| skill.metadata.clone()));
    let mut with_skills = Vec::with_capacity(cases.len());
    let mut without_skills = Vec::with_capacity(cases.len());
    let mut precision_sum = 0.0;
    let mut recall_sum = 0.0;
    let mut rr_sum = 0.0;
    let mut measured = 0usize;
    let mut selected_total = 0usize;
    let mut selected_correct = 0usize;
    let mut no_skill_total = 0usize;
    let mut no_skill_correct = 0usize;
    let mut instruction_bytes = 0usize;

    for case in cases {
        let matches = index.search(&case.task, 3);
        let top = matches.first();
        let selected = top
            .filter(|matched| matched.score >= NO_SKILL_THRESHOLD)
            .map(|matched| matched.metadata.name.clone());
        let expected = case.expected.as_deref();
        let retrieval_hit = expected.is_some_and(|expected| {
            matches
                .iter()
                .any(|matched| matched.metadata.name == expected)
        });

        if let Some(expected) = expected {
            measured += 1;
            let ranked: Vec<String> = matches
                .iter()
                .map(|matched| matched.metadata.name.clone())
                .collect();
            let relevant = std::collections::BTreeSet::from([expected.to_string()]);
            let metrics = retrieval_metrics(&ranked, &relevant, 3);
            precision_sum += metrics.precision_at_k;
            recall_sum += metrics.recall_at_k;
            rr_sum += metrics.reciprocal_rank;
        } else {
            no_skill_total += 1;
            if selected.is_none() {
                no_skill_correct += 1;
            }
        }
        if selected.is_some() {
            selected_total += 1;
            if selected.as_deref() == expected {
                selected_correct += 1;
            }
        }

        // Baseline: no instructions, so only a task with no required
        // directive can succeed.
        without_skills.push(CaseOutcome {
            success: expected.is_none() && case.required_terms.is_empty(),
            selected: None,
            retrieval_hit,
            instruction_bytes: 0,
        });

        // Activated path: only a selected, expected Skill runs the real
        // plan/activate/compose contract.
        let mut bytes = 0usize;
        let success = match (selected.as_deref(), expected) {
            (None, None) => case.required_terms.is_empty(),
            (Some(selected), Some(expected)) if selected == expected => {
                let mut runtime = SkillRuntime::new(
                    std::sync::Arc::new(index.clone()),
                    host_policy(),
                    limits.clone(),
                );
                let plan = runtime
                    .plan("benchmark", &[selected.to_string()])
                    .map_err(|error| error.to_string())?;
                for name in &plan.order {
                    let Some(content) = skills.iter().find(|skill| &skill.metadata.name == name)
                    else {
                        return Err(format!("planned skill '{name}' is missing"));
                    };
                    runtime
                        .activate_with_body("benchmark", content, SkillPermission::Model)
                        .map_err(|error| error.to_string())?;
                }
                let composed = runtime
                    .compose("benchmark")
                    .map_err(|error| error.to_string())?;
                bytes = composed.bytes;
                instruction_bytes += bytes;
                case.required_terms
                    .iter()
                    .all(|term| composed.text.contains(term))
            }
            _ => false,
        };
        with_skills.push(CaseOutcome {
            success,
            selected,
            retrieval_hit,
            instruction_bytes: bytes,
        });
    }

    let measured = measured.max(1) as f64;
    Ok(BenchmarkReport {
        with_skills,
        without_skills,
        retrieval: RetrievalMetrics {
            precision_at_k: precision_sum / measured,
            recall_at_k: recall_sum / measured,
            reciprocal_rank: rr_sum / measured,
        },
        activation_precision: if selected_total == 0 {
            0.0
        } else {
            selected_correct as f64 / selected_total as f64
        },
        no_skill_accuracy: if no_skill_total == 0 {
            1.0
        } else {
            no_skill_correct as f64 / no_skill_total as f64
        },
        instruction_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillCategory, SkillMetadata, SkillVersion};
    use std::path::PathBuf;

    fn skill(name: &str, body: &str, tags: &[&str]) -> SkillContent {
        let mut metadata = SkillMetadata::new(
            name.to_string(),
            format!("{name} skill"),
            PathBuf::from(format!("/bench/{name}/SKILL.md")),
        );
        metadata.tags = tags.iter().map(|tag| tag.to_string()).collect();
        metadata.category = SkillCategory::Code;
        metadata.version = SkillVersion::new(1, 0, 0);
        SkillContent {
            metadata,
            instructions: body.to_string(),
            skill_dir: PathBuf::from(format!("/bench/{name}")),
        }
    }

    #[test]
    fn activation_improves_deterministic_tasks_at_a_measured_context_cost() {
        let skills = vec![
            skill(
                "git-commit",
                "Run the tests, then write a clear commit message.",
                &["commit", "git"],
            ),
            skill(
                "deploy",
                "Deploy with a health check and a rollback plan.",
                &["deploy", "ship"],
            ),
        ];
        let cases = vec![
            BenchmarkCase::skill(
                "write a commit message",
                "git-commit",
                &["clear commit message"],
            ),
            BenchmarkCase::skill("ship a build", "deploy", &["health check"]),
            BenchmarkCase::no_skill("tell me a joke"),
        ];
        let report =
            run_benchmark(&skills, &cases, SkillLimits::default()).expect("benchmark runs");

        assert_eq!(report.with_success_rate(), 1.0);
        assert_eq!(report.without_success_rate(), 1.0 / 3.0);
        assert!((report.improvement() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(report.activation_precision, 1.0);
        assert_eq!(report.no_skill_accuracy, 1.0);
        assert!(report.instruction_bytes > 0);
        assert!(report.retrieval.recall_at_k >= 0.99);
        assert!(report.retrieval.reciprocal_rank >= 0.99);
    }
}
