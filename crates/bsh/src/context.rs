//! The per-turn runtime-context snapshot, in the reference's shape.
//!
//! The shipped DeepSeek Harness renders its context sections into ONE user
//! message headed by
//! `Current runtime context. This snapshot supersedes earlier runtime-context
//! snapshots.` (`dsh-system-prompt__lib__index.js`, `renderContextSnapshot`).
//! BSH did not carry that message at all: its sandbox and permission facts
//! reached the model only through tool results and refusals, so the model had to
//! discover the ground it stood on by bumping into it.
//!
//! This module builds the snapshot from the SAME [`Effective`] the turn's build
//! uses - not from a second read of the settings - and the runner injects it as
//! the first user message of every turn. It is rebuilt on every turn, never
//! cached: a switch flipped between turns is named on the next turn, and the
//! turn already streaming keeps the facts it was built with.
//!
//! The sandbox sentence uses the three reason strings the bash result and the
//! audit page already print ([`crate::settings`], [`agent::tools::bash`]),
//! so the model, the transcript and the audit read one truth.

use std::path::Path;

use agent::tools::bash::SANDBOX_RELEASED_REASON;
use agent::Agent;

use crate::settings::Effective;

/// The reference's snapshot heading, byte-for-byte.
pub(crate) const RUNTIME_CONTEXT_PREAMBLE: &str =
    "Current runtime context. This snapshot supersedes earlier runtime-context snapshots.";

/// The reference's child context, with BSH's own permission fact.
///
/// The reference says approval-requiring operations are rejected automatically;
/// BSH has no approval mechanism at all, so that clause is replaced by the fact
/// that a child cannot add a tool it was not built with. The opening, the
/// no-widening rule and the reporting rule are the reference's.
pub(crate) const SUBAGENT_CONTEXT: &str = "You are a delegated subagent: your permission scope was fixed when you were started and cannot be widened from inside this session, and a tool this child was not given cannot be added here. When the task needs access beyond that scope, do not retry a denied operation; state the limitation in your reply so the delegating agent can handle it.";

/// The resolved facts one turn's snapshot states.
///
/// Stored (rather than the rendered text) so a child turn built later can
/// render its own snapshot from the same resolution its build used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeFacts {
    /// The workspace root the turn runs in, or empty for no workspace.
    pub(crate) cwd: String,
    /// Whether the shell is confined after resolution.
    pub(crate) confined: bool,
    /// Why it is not confined, when it is not: one of the three sandbox reason
    /// strings. Never a fourth wording.
    pub(crate) sandbox_reason: Option<&'static str>,
    /// The effective permission tier.
    pub(crate) permission: String,
    /// The preset id in force, or empty.
    pub(crate) preset: String,
    /// Whether the preset master switch is off.
    pub(crate) presets_disabled: bool,
}

impl RuntimeFacts {
    /// Read the facts from the one resolution the build uses.
    ///
    /// No field is recomputed here: the sandbox reason is the same precedence
    /// [`crate::settings::resolve::sandbox_off_reason`] applies to a bash
    /// result, with the permission's own release as the remaining arm.
    pub(crate) fn from_effective(effective: &Effective) -> Self {
        let sandbox_reason = if effective.sandbox.heel {
            None
        } else {
            crate::settings::resolve::sandbox_off_reason(&effective.sandbox)
                .or(Some(SANDBOX_RELEASED_REASON))
        };
        Self {
            cwd: effective.root.clone(),
            confined: effective.sandbox.heel,
            sandbox_reason,
            permission: effective.permission.effective.clone(),
            preset: effective.preset_id.clone(),
            presets_disabled: effective.presets_disabled,
        }
    }
}

/// The two user messages a turn carries in front of its own input.
///
/// Both are rebuilt per turn. `instructions` is `None` when the workspace has
/// no AGENTS.md, so nothing empty is ever sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnContext {
    /// The runtime snapshot. Empty only in tests that opt out of injecting.
    pub(crate) snapshot: String,
    /// The framed workspace instructions, when the workspace has any.
    pub(crate) instructions: Option<String>,
}

impl TurnContext {
    /// Render one turn's context from already-resolved facts.
    pub(crate) fn from_facts(facts: &RuntimeFacts, child: bool) -> Self {
        Self {
            snapshot: render_snapshot(facts, child),
            instructions: crate::instructions::render(Path::new(&facts.cwd)),
        }
    }

    /// Append the context ahead of the turn's input.
    ///
    /// The agent's session is re-seeded from the stored transcript on every
    /// turn, so these messages live for one request and are refreshed by the
    /// next turn's build instead of accumulating in the durable transcript.
    pub(crate) fn inject(&self, agent: &Agent) {
        let mut session = agent.session();
        if !self.snapshot.is_empty() {
            session.add_user_content(react::llm::Content::Text(self.snapshot.clone()));
        }
        if let Some(instructions) = &self.instructions {
            session.add_user_content(react::llm::Content::Text(instructions.clone()));
        }
    }

    /// The messages [`Self::inject`] would append, in order.
    #[cfg(test)]
    pub(crate) fn injected_messages(&self) -> Vec<&str> {
        let mut messages = Vec::new();
        if !self.snapshot.is_empty() {
            messages.push(self.snapshot.as_str());
        }
        if let Some(instructions) = &self.instructions {
            messages.push(instructions);
        }
        messages
    }
}

/// Whether a user row is one of the harness's per-turn injections.
///
/// The child transcript is read back from the agent's own session, where these
/// rows exist; the reader's view should show the child's work, not the frame
/// the harness put around it.
pub(crate) fn is_injected_user_text(text: &str) -> bool {
    text.starts_with(RUNTIME_CONTEXT_PREAMBLE)
        || (text.starts_with("<system-reminder>")
            && text.contains("The following workspace instructions"))
        // The engine's closing request when a turn reaches its step guard. It is
        // a harness instruction, not something the child or its caller said, so
        // the read-only view must not draw it as a user row.
        || text.starts_with(react::engine::STEP_GUARD_NOTE)
        // The engine's one retry when a step came back empty: also harness
        // wording, never a row the caller wrote.
        || text.starts_with(react::engine::EMPTY_ANSWER_NOTE)
}

/// The most top-level entries the layout names.
const LAYOUT_ENTRIES: usize = 40;

/// How many directories the layout opens one level.
const LAYOUT_EXPANDED_DIRS: usize = 3;

/// The most children a directory may have and still be opened.
const LAYOUT_DIR_ENTRIES: usize = 12;

/// The layout's byte budget, so a wide directory cannot bloat the snapshot.
const LAYOUT_BYTES: usize = 900;

/// One line naming the workspace's shape.
///
/// The reader asks for a small thing and the model starts by listing the tree:
/// one round trip, then the real work. The listing is cheap here - two read_dir
/// levels, not a walk - and it lets step one read the file it needs. Opening a
/// source directory matters as much as naming it: "src/" alone still forces a
/// list_dir before src/main.rs can be read. Nothing hidden, no .git, bounded so
/// a huge directory cannot become the prompt.
fn render_layout(root: &Path) -> Option<String> {
    let entries = child_names(root);
    if entries.is_empty() {
        return None;
    }
    let total = entries.len();
    let mut shown: Vec<String> = Vec::new();
    let mut bytes = 0;
    let mut expanded = 0;
    for (name, is_dir) in entries {
        if shown.len() >= LAYOUT_ENTRIES || bytes + name.len() > LAYOUT_BYTES {
            break;
        }
        let mut text = if is_dir { format!("{name}/") } else { name };
        if is_dir && expanded < LAYOUT_EXPANDED_DIRS {
            let children = child_names(&root.join(text.trim_end_matches('/')));
            if !children.is_empty() && children.len() <= LAYOUT_DIR_ENTRIES {
                expanded += 1;
                let names: Vec<String> = children
                    .into_iter()
                    .filter(|(child, _)| !child.starts_with('.'))
                    .map(|(child, dir)| if dir { format!("{child}/") } else { child })
                    .collect();
                if !names.is_empty() {
                    text = format!("{text} ({})", names.join(", "));
                }
            }
        }
        bytes += text.len() + 2;
        shown.push(text);
    }
    let more = if total > shown.len() {
        format!(" (+{} more)", total - shown.len())
    } else {
        String::new()
    };
    Some(format!(
        "Workspace layout: {}{}. Read the files you need from this without listing the workspace first, and batch every independent read into one message.",
        shown.join(", "),
        more
    ))
}

/// The visible names of one directory, sorted, as (name, is directory).
///
/// The repository directory is dropped: it is not a workspace fact, and every
/// model already knows what it holds.
fn child_names(dir: &Path) -> Vec<(String, bool)> {
    let mut names: Vec<(String, bool)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_name() != ".git")
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().to_string(),
                        entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Render the snapshot: heading, working directory and layout, sandbox,
/// permission, preset, then the child context when this is a child turn.
fn render_snapshot(facts: &RuntimeFacts, child: bool) -> String {
    let mut sections: Vec<String> = Vec::new();
    let cwd = facts.cwd.trim();
    sections.push(if cwd.is_empty() {
        "Current working directory: none (no workspace is open).".to_string()
    } else {
        format!("Current working directory: {cwd:?}.")
    });
    // The shape of the workspace, so the first step can read the file it needs
    // instead of spending a whole round trip on list_dir. A turn is one LLM
    // call per step and a step costs seconds, so a free listing is not free.
    if !cwd.is_empty() {
        if let Some(layout) = render_layout(Path::new(cwd)) {
            sections.push(layout);
        }
    }
    sections.push(match (facts.confined, facts.sandbox_reason) {
        (true, _) => "Sandbox: confined to the workspace.".to_string(),
        (false, Some(reason)) => format!("Sandbox: none ({reason})."),
        // A released shell always carries one of the three reasons; this arm
        // exists so a future arm cannot render a bare "none".
        (false, None) => format!("Sandbox: none ({SANDBOX_RELEASED_REASON})."),
    });
    sections.push(format!("Permission: {}.", facts.permission));
    sections.push(if facts.presets_disabled {
        "Agent preset: none (presets are off).".to_string()
    } else if facts.preset.trim().is_empty() {
        "Agent preset: none (no preset selected).".to_string()
    } else {
        format!("Agent preset: {}.", facts.preset)
    });
    if child {
        sections.push(SUBAGENT_CONTEXT.to_string());
    }
    format!("{RUNTIME_CONTEXT_PREAMBLE}\n\n{}", sections.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::global::{Preset, SandboxChoice};
    use crate::settings::resolve::SessionChoice;
    use crate::settings::Settings;
    use crate::vault::Vault;
    use agent::tools::bash::{PRESET_SANDBOX_OFF_REASON, SANDBOX_SETTINGS_OFF_REASON};

    const READ_ONLY: &str = "read-only";
    const FULL_ACCESS: &str = "danger-full-access";

    /// The engine's closing request is a harness instruction, so the child's
    /// read-only transcript must not draw it as something the caller said.
    #[test]
    fn the_step_guard_note_is_not_a_user_row() {
        assert!(is_injected_user_text(react::engine::STEP_GUARD_NOTE));
        assert!(is_injected_user_text(react::engine::EMPTY_ANSWER_NOTE));
        assert!(
            !is_injected_user_text("Please fix the failing test."),
            "a real request still draws as a user row"
        );
    }

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bsh-context-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).expect("a project root");
        dir
    }

    fn rooted(root: &std::path::Path) -> Settings {
        let mut settings = Settings::default();
        let id = settings.note_workspace("scratch", &root.to_string_lossy());
        settings.global.general.active_workspace = id;
        settings
    }

    fn build(settings: &Settings) -> crate::caps::BuiltAgent {
        crate::caps::build_agent(
            settings,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        )
    }

    fn facts(settings: &Settings) -> RuntimeFacts {
        RuntimeFacts::from_effective(&build(settings).effective)
    }

    fn turn_context(settings: &Settings, child: bool) -> TurnContext {
        TurnContext::from_facts(&facts(settings), child)
    }

    /// The snapshot opens with the reference's heading and states the four
    /// live facts in its own sentences.
    #[test]
    fn the_snapshot_carries_the_live_facts() {
        let root = scratch("facts");
        let snapshot = render_snapshot(&facts(&rooted(&root)), false);
        assert!(snapshot.starts_with(RUNTIME_CONTEXT_PREAMBLE), "{snapshot}");
        assert!(
            snapshot.contains(&format!("Current working directory: {root:?}.")),
            "{snapshot}"
        );
        assert!(
            snapshot.contains("Sandbox: confined to the workspace."),
            "{snapshot}"
        );
        assert!(
            snapshot.contains("Permission: workspace-write."),
            "{snapshot}"
        );
        assert!(snapshot.contains("Agent preset: standard."), "{snapshot}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The first user message names the workspace top level, so step one can
    /// read the file it needs instead of spending a round trip on list_dir.
    #[test]
    fn the_snapshot_names_the_workspace_layout() {
        let root = scratch("layout");
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::write(root.join("Cargo.toml"), "[package]").expect("manifest");
        std::fs::write(root.join("src").join("main.rs"), "fn main() {}").expect("file");
        let snapshot = render_snapshot(&facts(&rooted(&root)), false);
        assert!(
            snapshot.contains("Workspace layout: Cargo.toml, src/ (main.rs)."),
            "{snapshot}"
        );
        assert!(
            !snapshot.contains(".git,"),
            "the repository directory is not a workspace fact: {snapshot}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A wide directory is capped, and the cap is named rather than silent.
    #[test]
    fn the_layout_is_bounded() {
        let root = scratch("layout-wide");
        for index in 0..100 {
            std::fs::write(root.join(format!("file-{index:03}.txt")), "x").expect("file");
        }
        let snapshot = render_snapshot(&facts(&rooted(&root)), false);
        assert!(
            snapshot.contains("(+"),
            "the remainder is named: {snapshot}"
        );
        assert!(
            snapshot.len() < 2_000,
            "the layout cannot bloat the snapshot: {} bytes",
            snapshot.len()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A read-only session states the tier actually in force.
    #[test]
    fn a_read_only_session_shows_the_read_only_tier() {
        let root = scratch("read-only");
        let mut settings = rooted(&root);
        settings.global.default_preset = String::new();
        settings.workspace.agent.permission = READ_ONLY.to_string();
        let snapshot = render_snapshot(&facts(&settings), false);
        assert!(snapshot.contains("Permission: read-only."), "{snapshot}");
        assert!(
            snapshot.contains("Agent preset: none (no preset selected)."),
            "the preset line is still honest: {snapshot}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A child turn carries the delegation context in the same snapshot.
    #[test]
    fn a_child_turn_carries_the_subagent_context() {
        let root = scratch("child");
        let facts = facts(&rooted(&root));
        let child = render_snapshot(&facts, true);
        assert!(child.contains(SUBAGENT_CONTEXT), "{child}");
        assert!(child.contains("Permission: workspace-write."), "{child}");
        let parent = render_snapshot(&facts, false);
        assert!(!parent.contains(SUBAGENT_CONTEXT), "{parent}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The three release arms are distinguishable, and each names its own
    /// switch in the tools' and audit's vocabulary.
    #[test]
    fn the_three_sandbox_releases_are_named_with_the_shared_vocabulary() {
        let root = scratch("releases");

        // 1. the settings master switch, over a preset that asked for heel.
        let mut settings = rooted(&root);
        settings.global.presets.insert(
            "heeled".to_string(),
            Preset {
                sandbox: Some(SandboxChoice::Heel),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "heeled".to_string();
        assert!(render_snapshot(&facts(&settings), false).contains("Sandbox: confined"));
        settings.global.general.sandbox_enabled = Some(false);
        assert_eq!(
            facts(&settings).sandbox_reason,
            Some(SANDBOX_SETTINGS_OFF_REASON)
        );
        assert!(render_snapshot(&facts(&settings), false)
            .contains(&format!("Sandbox: none ({SANDBOX_SETTINGS_OFF_REASON}).")));

        // 2. a preset's own widening at full access.
        let mut settings = rooted(&root);
        settings.workspace.agent.permission = FULL_ACCESS.to_string();
        settings.global.presets.insert(
            "widened".to_string(),
            Preset {
                permission: Some(FULL_ACCESS.to_string()),
                sandbox: Some(SandboxChoice::Off),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "widened".to_string();
        assert_eq!(
            facts(&settings).sandbox_reason,
            Some(PRESET_SANDBOX_OFF_REASON)
        );
        assert!(render_snapshot(&facts(&settings), false)
            .contains(&format!("Sandbox: none ({PRESET_SANDBOX_OFF_REASON}).")));

        // 3. the permission's own release, with no policy arm involved.
        let mut settings = rooted(&root);
        settings.global.general.presets_enabled = Some(false);
        settings.workspace.agent.permission = FULL_ACCESS.to_string();
        assert_eq!(
            facts(&settings).sandbox_reason,
            Some(SANDBOX_RELEASED_REASON)
        );
        assert!(render_snapshot(&facts(&settings), false)
            .contains(&format!("Sandbox: none ({SANDBOX_RELEASED_REASON}).")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A snapshot is built for the turn that is starting: flipping a switch
    /// does not rewrite the running turn's facts, and the next build names the
    /// switch that released the shell.
    #[test]
    fn a_switch_changes_the_next_turn_and_not_the_running_one() {
        let root = scratch("next-turn");
        let mut settings = rooted(&root);
        // The built turn owns its Effective; it is not re-read when the
        // settings change underneath it.
        let running_build = build(&settings);
        let running = TurnContext::from_facts(
            &RuntimeFacts::from_effective(&running_build.effective),
            false,
        );
        assert!(running
            .snapshot
            .contains("Sandbox: confined to the workspace."));
        settings.global.general.sandbox_enabled = Some(false);
        let still_running = TurnContext::from_facts(
            &RuntimeFacts::from_effective(&running_build.effective),
            false,
        );
        assert_eq!(
            still_running.snapshot, running.snapshot,
            "the already-built turn keeps the facts it was sent with"
        );
        let next = turn_context(&settings, false);
        assert!(
            next.snapshot
                .contains(&format!("Sandbox: none ({SANDBOX_SETTINGS_OFF_REASON}).")),
            "and the next turn names the settings switch: {}",
            next.snapshot
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With no instructions the context is ONE user message, never an empty
    /// second one.
    #[test]
    fn a_workspace_without_instructions_injects_only_the_snapshot() {
        let root = scratch("inject");
        let context = TurnContext::from_facts(&facts(&rooted(&root)), false);
        assert_eq!(context.instructions, None);
        assert_eq!(context.injected_messages().len(), 1);
        assert!(context.injected_messages()[0].starts_with(RUNTIME_CONTEXT_PREAMBLE));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With an instruction file the two messages are snapshot first, then the
    /// framed instructions, and both reach the agent session in that order.
    #[test]
    fn the_snapshot_precedes_the_instructions_in_the_session() {
        let root = scratch("order");
        std::fs::write(root.join("AGENTS.md"), "workspace rule").expect("the file");
        let context = TurnContext::from_facts(&facts(&rooted(&root)), false);
        let messages = context.injected_messages();
        assert_eq!(messages.len(), 2);
        assert!(messages[0].starts_with(RUNTIME_CONTEXT_PREAMBLE));
        assert!(messages[1].contains("Instructions from: AGENTS.md"));

        let provider = std::sync::Arc::new(agent::agent::agentic::LlmProvider::new());
        let agent = Agent::new(agent::AgentConfig::default(), provider);
        context.inject(&agent);
        let session = agent.session();
        let user_rows: Vec<&str> = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                react::llm::LlmMessage::User { content } => content.as_text(),
                _ => None,
            })
            .collect();
        assert_eq!(user_rows.len(), 2, "{user_rows:?}");
        assert!(user_rows[0].starts_with(RUNTIME_CONTEXT_PREAMBLE));
        assert!(user_rows[1].contains("Instructions from: AGENTS.md"));
        assert!(
            user_rows.iter().all(|row| !row.is_empty()),
            "no empty injected message"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
