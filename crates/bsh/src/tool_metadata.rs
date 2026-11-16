//! Declared capability metadata for the built-in tools the host composes.
//!
//! The tool types live in `agent` and are generic: a `FunctionTool` does not
//! know whether its body writes a file. The host is the only place that knows
//! how it is wiring each built-in, so the declaration lives here and travels
//! with the tool through [agent::tools::DeclaredTool].
//!
//! Every entry is a claim about governance, not about behaviour: risk, side
//! effects, and permission scopes. A capability catalogue reads it; a policy
//! engine may use it for pre-validation. It never grants anything.

use agent::tools::ToolMetadata;

/// The declaration for one built-in tool, or None when the name is not a
/// built-in this host composes.
pub(crate) fn builtin_metadata(name: &str) -> Option<ToolMetadata> {
    let metadata = match name {
        "bash" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("high")
            .with_side_effects(["process", "write", "network"])
            .with_permissions(["proc.spawn", "fs.write:workspace", "net.egress"])
            .with_timeout_ms(600_000),
        "glob" | "grep" | "read_file" | "read_files" | "list_dir" => read_only("fs.read:workspace"),
        "write_file" | "edit_file" | "apply_patch" | "sed" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["write"])
            .with_permissions(["fs.write:workspace"]),
        "update_plan" => local("plan.write", "low"),
        "ask_user" => local("interaction.ask", "low"),
        "surface_catalogue" => read_only("surface.read"),
        "present_surface" | "patch_surface" | "close_surface" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["write"])
            .with_permissions(["surface.write"]),
        "schedule_list" => read_only("schedule.read"),
        "schedule_create" | "schedule_edit" | "schedule_delete" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["write"])
            .with_permissions(["schedule.write"]),
        "task_create" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["write", "spend"])
            .with_permissions(["task.write", "agent.spawn"]),
        "task_join" => read_only("task.read"),
        "task_cancel" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["write"])
            .with_permissions(["task.write"]),
        "task_status" | "task_list" => read_only("task.read"),
        "task_graph" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["write", "spend", "spawn"])
            .with_permissions(["task.write", "agent.spawn"]),
        "task_retry" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["write", "spend"])
            .with_permissions(["task.write", "agent.spawn"]),
        "get_goal" => local("goal.read", "low"),
        "update_goal" => local("goal.write", "medium"),
        "web_search" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["network"])
            .with_permissions(["net.egress"]),
        "mcp_gateway" => ToolMetadata::new()
            .with_risk("medium")
            .with_side_effects(["network"])
            .with_permissions(["net.egress"]),
        "subagent" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("high")
            .with_side_effects(["spend"])
            .with_permissions(["agent.spawn"]),
        "subagent_result" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("low")
            .with_side_effects(["read"])
            .with_permissions(["agent.read:runs"]),
        // A follow-up is another child turn, so it costs a model run: the same
        // spend and the same spawn capability the first delegation declares.
        "send_message" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("high")
            .with_side_effects(["spend"])
            .with_permissions(["agent.spawn"]),
        // Stopping a child signals a task this session owns: no spend, but a
        // process-level effect the audit must record.
        "interrupt_agent" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("low")
            .with_side_effects(["process"])
            .with_permissions(["agent.write:runs"]),
        "job_list" | "job_output" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("low")
            .with_side_effects(["read"])
            .with_permissions(["proc.read:jobs"]),
        "job_kill" => ToolMetadata::new()
            .with_version("1.0.0")
            .with_risk("medium")
            .with_side_effects(["process"])
            .with_permissions(["proc.signal:jobs"]),
        "federation_status" | "federation_peers" | "federation_capabilities" => {
            read_only("net.federation")
        }
        "federation_delegate" | "federation_cancel" | "federation_query" | "federation_cleanup" => {
            ToolMetadata::new()
                .with_version("1.0.0")
                .with_risk("medium")
                .with_side_effects(["network", "write"])
                .with_permissions(["net.federation"])
                .with_timeout_ms(30_000)
        }
        _ => return None,
    };
    Some(metadata)
}

/// The declaration for a built-in, panicking when the name has none.
pub(crate) fn declared(name: &str) -> ToolMetadata {
    builtin_metadata(name)
        .unwrap_or_else(|| panic!("built-in tool {name} has no declared metadata"))
}

fn read_only(scope: &str) -> ToolMetadata {
    ToolMetadata::new()
        .with_version("1.0.0")
        .with_risk("low")
        .with_side_effects(["read"])
        .with_permissions([scope])
}

fn local(scope: &str, risk: &str) -> ToolMetadata {
    ToolMetadata::new()
        .with_version("1.0.0")
        .with_risk(risk)
        .with_side_effects(["write"])
        .with_permissions([scope])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_builtin_names_a_version_and_a_risk() {
        for name in [
            "bash",
            "glob",
            "grep",
            "read_file",
            "read_files",
            "write_file",
            "edit_file",
            "apply_patch",
            "sed",
            "list_dir",
            "update_plan",
            "get_goal",
            "update_goal",
            "job_output",
            "job_kill",
            "job_list",
            "send_message",
            "interrupt_agent",
            "schedule_create",
            "schedule_list",
        ] {
            let metadata = builtin_metadata(name).unwrap_or_else(|| panic!("{name} is declared"));
            assert_eq!(metadata.version.as_deref(), Some("1.0.0"), "{name}");
            assert!(metadata.risk.is_some(), "{name}");
            assert!(!metadata.side_effects.is_empty(), "{name}");
        }
        assert!(builtin_metadata("mcp_github_create_issue").is_none());
    }
}
