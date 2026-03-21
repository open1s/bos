use agent::{
    WorkflowBuilder, StepBuilder, Scheduler, BackoffStrategy, StepType, ConditionType,
};
use anyhow::Result;
use brainos_common::{setup_bus, setup_logging};
use clap::Parser;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(about = "Demonstrates workflow scheduler capabilities")]
struct Args {
    #[arg(long)]
    workflow: Option<String>,

    #[arg(long)]
    parallel: bool,

    #[arg(long)]
    timeout_ms: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging()?;

    let args = Args::parse();
    let session = setup_bus(None).await?;

    println!("╔══════════════════════════════════════╗");
    println!("║  Scheduler Demo - Phase 4 Plan 02    ║");
    println!("╚══════════════════════════════════════╝\n");

    let scheduler = Scheduler::new();

    let workflow = match args.workflow.as_deref() {
        Some("sequential") => build_sequential_workflow(),
        Some("parallel") if args.parallel => build_parallel_workflow(),
        Some("conditional") => build_conditional_workflow(),
        Some("retry") => build_retry_workflow(),
        Some("timeout") => build_timeout_workflow(args.timeout_ms),
        _ => build_sequential_workflow(),
    };

    println!("Executing workflow: {} ({})\n", workflow.name, workflow.description);
    println!("Steps: {}\n", workflow.steps.len());

    println!("{}", "=".repeat(50));
    let result = scheduler.execute_workflow(&workflow).await;
    println!("{}", "=".repeat(50));
    println!();

        match &result.status {
            agent::WorkflowStatus::Completed => {
                println!("✓ Workflow completed successfully");
                println!("  Duration: {:?}", result.duration);
                println!("  Steps executed: {}", result.step_results.len());
            }
            agent::WorkflowStatus::Failed { failed_step } => {
                println!("✗ Workflow failed at step: {}", failed_step);
                println!("  Duration: {:?}", result.duration);
                for step_result in &result.step_results {
                    if let agent::StepStatus::Failed { error } = &step_result.status {
                        println!("  Error: {}", error);
                    }
                }
            }
            agent::WorkflowStatus::PartiallyCompleted => {
                println!("⚠ Workflow partially completed");
                println!("  Duration: {:?}", result.duration);
                println!("  Steps executed: {} / {}", result.step_results.len(), result.step_results.len());
            }
        }

    println!("\nStep Results:");
    for step_result in &result.step_results {
        let status_icon = match step_result.status {
            agent::StepStatus::Completed => "✓",
            agent::StepStatus::Failed { .. } => "✗",
            agent::StepStatus::TimedOut => "⏱",
            agent::StepStatus::Skipped => "⊘",
            agent::StepStatus::Retrying => "↻",
        };

        println!("  {} {} - {:?} ({:?})",
            status_icon,
            step_result.step_name,
            step_result.status,
            step_result.duration
        );

        if let Some(output) = &step_result.output {
            println!("    Output: {}", output);
        }

        if step_result.retry_count > 0 {
            println!("    Retries: {}", step_result.retry_count);
        }
    }

    Ok(())
}

fn build_sequential_workflow() -> agent::Workflow {
    WorkflowBuilder::new("sequential-workflow")
        .description("Sequential workflow demonstrating step chaining")
        .default_timeout(Duration::from_secs(5))
        .default_retries(3)
        .add_step(StepBuilder::new("step1")
            .sequential()
            .timeout(Duration::from_secs(1))
            .build()
        )
        .add_step(StepBuilder::new("step2")
            .sequential()
            .timeout(Duration::from_secs(1))
            .build()
        )
        .add_step(StepBuilder::new("step3")
            .sequential()
            .timeout(Duration::from_secs(1))
            .build()
        )
        .build()
}

fn build_parallel_workflow() -> agent::Workflow {
    WorkflowBuilder::new("parallel-workflow")
        .description("Parallel workflow demonstrating concurrent execution")
        .default_timeout(Duration::from_secs(5))
        .default_retries(1)
        .add_step(StepBuilder::new("parallel-step")
            .parallel()
            .timeout(Duration::from_secs(2))
            .build()
        )
        .build()
}

fn build_conditional_workflow() -> agent::Workflow {
    WorkflowBuilder::new("conditional-workflow")
        .description("Conditional workflow demonstrating branching")
        .default_timeout(Duration::from_secs(3))
        .default_retries(1)
        .add_step(StepBuilder::new("conditional-step")
            .conditional(ConditionType::JsonPath {
                path: "value".to_string(),
                expected: serde_json::json!(10),
            })
            .timeout(Duration::from_secs(1))
            .build()
        )
        .build()
}

fn build_retry_workflow() -> agent::Workflow {
    WorkflowBuilder::new("retry-workflow")
        .description("Retry workflow demonstrating exponential backoff")
        .default_timeout(Duration::from_secs(10))
        .default_retries(5)
        .add_step(StepBuilder::new("retry-step")
            .sequential()
            .timeout(Duration::from_millis(100))
            .backoff(BackoffStrategy::Exponential {
                base: Duration::from_millis(50),
                max: Duration::from_secs(1),
            })
            .build()
        )
        .build()
}

fn build_timeout_workflow(timeout_ms: Option<u64>) -> agent::Workflow {
    WorkflowBuilder::new("timeout-workflow")
        .description("Timeout workflow demonstrating duration enforcement")
        .default_timeout(Duration::from_millis(timeout_ms.unwrap_or(50)))
        .default_retries(1)
        .add_step(StepBuilder::new("long-step")
            .sequential()
            .timeout(Duration::from_millis(timeout_ms.unwrap_or(50)))
            .build()
        )
        .build()
}
