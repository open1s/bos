//! The deterministic AUDIT view model.
//!
//! The page answers "what did this conversation actually do, and what was
//! refused?" from data that already exists: the persisted [crate::model::ToolEvent]
//! rows on each ChatMessage, merged with the policy denial ring. No new
//! storage, no model call, no I/O. Every figure here is counted from those two
//! sources, so the same session always produces the same page.
//!
//! Two limits are stated on the page and repeated here: a shell command's
//! output is recorded but the files it creates are not individually audited,
//! and the only network the audit can see is tool/MCP egress - a shell's own
//! sockets are not intercepted.
//!
//! Rows are windowed like the transcript: Session::messages holds the loaded
//! window, and a tool call's FULL text is never held here - only a bounded,
//! redacted preview. The full record is read from the session on demand, when a
//! reader opens a call.
//!
//! Compaction never removes transcript rows (it moves the request boundary), so
//! building this view walks every loaded message regardless of
//! [crate::model::Session::compacted]: a fold cannot make an audited call
//! disappear.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::{ChatMessage, Role, Session};
use agent::policy::PolicyAuditEntry;

/// How many characters a preview may carry.
pub(crate) const PREVIEW_LIMIT: usize = 220;

/// How much of a value is scanned for secret-looking keys before the preview is
/// cut. Bounds the redaction's work as well as the preview's size.
pub(crate) const PREVIEW_SCAN: usize = 4_096;

/// How many characters a target may carry.
pub(crate) const TARGET_LIMIT: usize = 96;

/// What a tool call was allowed to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditCategory {
    /// A file read (read_file, list_dir).
    FileRead,
    /// A file write (write_file, edit_file).
    FileWrite,
    /// A shell command (bash).
    Shell,
    /// Observable tool/MCP egress (web fetch and search).
    Network,
    /// A tool served by a connected MCP namespace.
    Mcp,
    /// A delegated child run.
    Subagent,
    /// Anything the table does not name.
    Other,
}

impl AuditCategory {
    /// The locale key this category renders under.
    pub(crate) fn key(self) -> &'static str {
        match self {
            AuditCategory::FileRead => "audit.category.fileRead",
            AuditCategory::FileWrite => "audit.category.fileWrite",
            AuditCategory::Shell => "audit.category.shell",
            AuditCategory::Network => "audit.category.network",
            AuditCategory::Mcp => "audit.category.mcp",
            AuditCategory::Subagent => "audit.category.subagent",
            AuditCategory::Other => "audit.category.other",
        }
    }

    /// Classify a tool by its name alone.
    ///
    /// This is the pure half of the rule: the names are the ones this shell
    /// registers. An MCP tool is namespaced {server}_{tool}, so it can only be
    /// recognised once the live namespaces are known - see
    /// [AuditCategory::classify_in].
    pub(crate) fn classify(tool: &str) -> Self {
        let name = tool.trim().to_ascii_lowercase();
        match name.as_str() {
            "read_file" | "list_dir" | "read" | "ls" | "glob" | "grep" | "search_files"
            | "view" => AuditCategory::FileRead,
            "write_file" | "edit_file" | "write" | "edit" | "patch" | "create_file"
            | "delete_file" | "move_file" | "rename_file" => AuditCategory::FileWrite,
            "bash" | "shell" | "run_command" | "exec" => AuditCategory::Shell,
            "subagent" => AuditCategory::Subagent,
            _ if name.starts_with("mcp_") || name == "mcp" => AuditCategory::Mcp,
            _ if name.starts_with("web_")
                || name.starts_with("http_")
                || matches!(
                    name.as_str(),
                    "fetch" | "http" | "url" | "egress" | "browse" | "browser" | "request"
                )
                || name.contains("fetch")
                || name.contains("web_search") =>
            {
                AuditCategory::Network
            }
            _ => AuditCategory::Other,
        }
    }

    /// Classify a tool, treating any connected MCP namespace as its own family.
    ///
    /// An MCP tool registers as {namespace}_{name}, and the namespaces are
    /// known while the pool is connected. Without that knowledge the same name
    /// would read as Other, so the caller passes the live list.
    pub(crate) fn classify_in(tool: &str, namespaces: &[String]) -> Self {
        let name = tool.trim().to_ascii_lowercase();
        for namespace in namespaces {
            let namespace = namespace.trim().to_ascii_lowercase();
            if namespace.is_empty() {
                continue;
            }
            if name == namespace
                || name
                    .strip_prefix(&namespace)
                    .is_some_and(|rest| rest.starts_with('_'))
            {
                return AuditCategory::Mcp;
            }
        }
        Self::classify(tool)
    }
}

/// How a call ended, as the stored record can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditOutcome {
    /// The call returned without the engine's error marker.
    Ok,
    /// The engine recorded an Error: prefix - a refused or failed tool call.
    Failed,
    /// The call started and has no recorded output yet.
    Pending,
}

impl AuditOutcome {
    /// The locale key this outcome renders under.
    pub(crate) fn key(self) -> &'static str {
        match self {
            AuditOutcome::Ok => "audit.outcome.ok",
            AuditOutcome::Failed => "audit.outcome.failed",
            AuditOutcome::Pending => "audit.outcome.pending",
        }
    }
}

/// One audited tool call.
///
/// The preview is bounded and redacted. The full record is NOT stored here: the
/// reader opens the call and [full_text] reads it from the session, so a
/// megabyte of output never becomes a megabyte of view model.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditCall {
    /// The tool name the model called.
    pub(crate) tool: String,
    /// What family the call belongs to.
    pub(crate) category: AuditCategory,
    /// The path, host or command the call named, redacted and bounded.
    pub(crate) target: String,
    /// How the call ended.
    pub(crate) outcome: AuditOutcome,
    /// The recorded wall-clock duration, when one was measured.
    pub(crate) ms: Option<u64>,
    /// A bounded, redacted one-line preview of the output (or the arguments).
    pub(crate) preview: String,
    /// Index of the message that carries the call, for the full record.
    pub(crate) message: usize,
    /// Index of the call among that message's tools.
    pub(crate) tool_index: usize,
}

/// One refused operation, from the policy denial ring.
///
/// A refusal is a decision, not a call: the tool never ran, which is why the
/// row says so instead of styling it as a failed invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AuditRefusal {
    /// When the policy decided, epoch milliseconds.
    pub(crate) at_ms: u64,
    /// The tool that asked.
    pub(crate) tool: String,
    /// The path or host it asked about.
    pub(crate) target: String,
    /// The effective permission in force at the decision.
    pub(crate) permission: String,
    /// The action the policy was asked about (read, write, egress, spawn,
    /// capability).
    pub(crate) action: String,
    /// The rule that refused it, when the policy named one.
    pub(crate) rule: Option<String>,
    /// The policy's reason, when it gave one.
    pub(crate) reason: Option<String>,
}

/// One turn and everything it did.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditTurn {
    /// 1-based turn number, the same ordinal the rail uses.
    pub(crate) index: usize,
    /// The transcript message that opened the turn, for the Chat jump.
    pub(crate) message: usize,
    /// When the turn opened, epoch milliseconds, when recorded.
    pub(crate) started_at: Option<u64>,
    /// The observed span in milliseconds; zero when no clock placed it.
    pub(crate) duration_ms: u64,
    /// The turn's calls, oldest first.
    pub(crate) calls: Vec<AuditCall>,
    /// The refusals that happened while this turn ran.
    pub(crate) refusals: Vec<AuditRefusal>,
}

/// The deterministic summary the page and its export print.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct AuditTotals {
    /// Number of turns.
    pub(crate) turns: usize,
    /// Number of tool calls.
    pub(crate) calls: usize,
    /// File-read tool calls.
    pub(crate) files_read: usize,
    /// File-write tool calls.
    pub(crate) files_written: usize,
    /// Shell commands.
    pub(crate) commands: usize,
    /// Refused operations.
    pub(crate) refusals: usize,
    /// Calls that returned without the engine's error marker.
    pub(crate) succeeded: usize,
    /// Calls the engine recorded as failed.
    pub(crate) failed: usize,
    /// Calls that have started and have no recorded output yet.
    pub(crate) pending: usize,
    /// The sum of every turn's observed span.
    pub(crate) duration_ms: u64,
}

/// Allowed policy decisions, counted per action.
///
/// The policy ring counts allows instead of storing them: a turn makes
/// thousands, and only the denials are individually interesting. The page says
/// so rather than pretending it lists them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct AuditAllows {
    /// Every allowed decision.
    pub(crate) total: u64,
    /// Allowed path reads.
    pub(crate) reads: u64,
    /// Allowed path writes.
    pub(crate) writes: u64,
    /// Allowed egress checks.
    pub(crate) egress: u64,
    /// Allowed process spawns.
    pub(crate) spawns: u64,
    /// Allowed capability (registration-time) questions.
    pub(crate) capabilities: u64,
}

/// The policy that was applied, and the results it produced.
///
/// This is the second half of the audit: a refusal row says WHICH rule refused
/// WHICH action against WHICH target under WHICH permission, and this block says
/// which evaluator produced those decisions, how many were allowed and denied,
/// and whether the set is in its failure mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct AuditPolicy {
    /// The effective permission the policy applies right now.
    pub(crate) permission: String,
    /// "bytecode" or "interpreter".
    pub(crate) evaluator: String,
    /// Why the set is in its failure mode, when it is.
    pub(crate) error: Option<String>,
    /// The preset whose policy data was applied (empty takes the default).
    pub(crate) preset: String,
    /// Paths the preset restricts reads to (0 = unrestricted).
    pub(crate) read_scope: usize,
    /// Paths the preset restricts writes to (0 = unrestricted).
    pub(crate) write_scope: usize,
    /// Paths the preset denies outright.
    pub(crate) deny_roots: usize,
    /// Entries in the workspace egress allowlist.
    pub(crate) egress_allowlist: usize,
    /// Whether egress is restricted to the allowlist.
    pub(crate) egress_restricted: bool,
    /// Allowed decisions, counted per action.
    pub(crate) allows: AuditAllows,
    /// Every denial ever counted, including ones the bounded ring dropped.
    pub(crate) denials_total: u64,
}

impl AuditPolicy {
    /// The total decisions the policy answered: allowed plus denied.
    pub(crate) fn checks(&self) -> u64 {
        self.allows.total.saturating_add(self.denials_total)
    }
}

/// The whole page: totals computed once, and one row per turn.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct AuditView {
    /// Session-wide figures.
    pub(crate) totals: AuditTotals,
    /// The policy that answered these decisions, and its counted results.
    pub(crate) policy: AuditPolicy,
    /// Turns in transcript order.
    pub(crate) turns: Vec<AuditTurn>,
}

impl AuditView {
    /// Build the view from a session's loaded window and the denial ring.
    ///
    /// The denials are scoped to the session here: the ring is process-wide,
    /// and an audit of THIS conversation must not borrow a sibling
    /// conversation's refusal.
    pub(crate) fn build(
        session: &Session,
        denials: &[PolicyAuditEntry],
        mcp_namespaces: &[String],
    ) -> Self {
        let mut turns: Vec<AuditTurn> = Vec::new();
        let mut message_turn: Vec<Option<usize>> = vec![None; session.messages.len()];
        for (index, message) in session.messages.iter().enumerate() {
            if message.role == Role::User || turns.is_empty() {
                turns.push(AuditTurn {
                    index: turns.len() + 1,
                    message: index,
                    started_at: None,
                    duration_ms: 0,
                    calls: Vec::new(),
                    refusals: Vec::new(),
                });
            }
            let turn = turns.len() - 1;
            message_turn[index] = Some(turn);
            for (tool_index, tool) in message.tools.iter().enumerate() {
                turns[turn].calls.push(AuditCall {
                    tool: tool.name.clone(),
                    category: AuditCategory::classify_in(&tool.name, mcp_namespaces),
                    target: target_of(&tool.args),
                    outcome: outcome_of(tool.output.as_deref()),
                    ms: tool.ms,
                    preview: preview(
                        tool.output
                            .as_deref()
                            .filter(|output| !output.trim().is_empty())
                            .unwrap_or(&tool.args),
                    ),
                    message: index,
                    tool_index,
                });
            }
        }
        // The turn's own clock: the earliest recorded start among its messages,
        // and the latest end. A call with no measured duration contributes its
        // start only, so a streaming turn still has a position.
        let mut starts: Vec<Option<u64>> = vec![None; turns.len()];
        let mut ends: Vec<Option<u64>> = vec![None; turns.len()];
        for (index, message) in session.messages.iter().enumerate() {
            let Some(turn) = message_turn[index] else {
                continue;
            };
            let (start, end) = span_of(message);
            if let Some(start) = start {
                starts[turn] = Some(starts[turn].map_or(start, |current| current.min(start)));
            }
            if let Some(end) = end {
                ends[turn] = Some(ends[turn].map_or(end, |current| current.max(end)));
            }
        }
        for (index, turn) in turns.iter_mut().enumerate() {
            // The user message's own submission instant is the canonical start
            // when the record carries one; every later timestamp is activity
            // inside it, not a new opening.
            let user_start =
                session
                    .messages
                    .get(turn.message)
                    .and_then(|message| match message.role {
                        Role::User => message.timing().and_then(|timing| timing.start),
                        Role::Assistant => None,
                    });
            turn.started_at = user_start.or(starts[index]);
            turn.duration_ms = match (starts[index], ends[index]) {
                (Some(start), Some(end)) => end.saturating_sub(start),
                _ => 0,
            };
        }
        // Merge each refusal into the turn it happened in. The ring is ordered,
        // so a last-turn-at-or-before rule is deterministic; a record with no
        // clocks at all can only be placed at the end, which the page says by
        // keeping the ring's own order.
        let mut scoped: Vec<&PolicyAuditEntry> = denials
            .iter()
            .filter(|entry| entry.session == session.id)
            .collect();
        scoped.sort_by_key(|entry| entry.at_ms);
        for entry in scoped {
            let target = turns
                .iter()
                .rposition(|turn| turn.started_at.is_some_and(|start| start <= entry.at_ms));
            let fallback = if turns.iter().any(|turn| turn.started_at.is_some()) {
                0
            } else {
                turns.len().saturating_sub(1)
            };
            let Some(turn) = turns.get_mut(target.unwrap_or(fallback)) else {
                continue;
            };
            turn.refusals.push(AuditRefusal {
                at_ms: entry.at_ms,
                tool: entry.tool.clone(),
                target: preview_bounded(&entry.target, TARGET_LIMIT),
                permission: entry.permission.clone(),
                action: entry.action.as_str().to_string(),
                rule: entry.rule.clone(),
                reason: entry.reason.clone(),
            });
        }
        let mut view = AuditView {
            totals: AuditTotals {
                turns: turns.len(),
                ..AuditTotals::default()
            },
            policy: AuditPolicy::default(),
            turns,
        };
        view.totals = view.count();
        view
    }

    /// Attach the policy results the page and its export print.
    ///
    /// The view model stays framework-free: the caller resolves the policy
    /// snapshot (evaluator, effective permission, counted allows, total denials)
    /// and this method records it beside the refusals those decisions produced.
    pub(crate) fn with_policy(mut self, policy: AuditPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Re-count every total from the built turns.
    fn count(&self) -> AuditTotals {
        let mut totals = AuditTotals {
            turns: self.turns.len(),
            ..AuditTotals::default()
        };
        for turn in &self.turns {
            totals.calls += turn.calls.len();
            totals.refusals += turn.refusals.len();
            totals.duration_ms += turn.duration_ms;
            for call in &turn.calls {
                match call.category {
                    AuditCategory::FileRead => totals.files_read += 1,
                    AuditCategory::FileWrite => totals.files_written += 1,
                    AuditCategory::Shell => totals.commands += 1,
                    _ => {}
                }
                match call.outcome {
                    AuditOutcome::Ok => totals.succeeded += 1,
                    AuditOutcome::Failed => totals.failed += 1,
                    AuditOutcome::Pending => totals.pending += 1,
                }
            }
        }
        totals
    }

    /// The refusal rules and how often each was used, alphabetical.
    ///
    /// Deterministic order is the point: the summary line cannot reshuffle
    /// between frames because the ring happened to iterate differently.
    pub(crate) fn rule_counts(&self) -> Vec<(Option<String>, usize)> {
        let mut counts: BTreeMap<Option<String>, usize> = BTreeMap::new();
        for turn in &self.turns {
            for refusal in &turn.refusals {
                *counts.entry(refusal.rule.clone()).or_default() += 1;
            }
        }
        counts.into_iter().collect()
    }

    /// The measure a turn contributes under the toolbar's mode.
    pub(crate) fn weight(&self, turn: &AuditTurn, mode: usize) -> f32 {
        match mode {
            0 => turn.duration_ms as f32,
            2 => turn.calls.len() as f32,
            _ => 1.0,
        }
    }

    /// The largest weight any turn contributes under this mode.
    pub(crate) fn max_weight(&self, mode: usize) -> f32 {
        self.turns
            .iter()
            .map(|turn| self.weight(turn, mode))
            .fold(0.0f32, f32::max)
    }

    /// A turn's share of the strip, 0..=1.
    pub(crate) fn share(&self, turn: &AuditTurn, mode: usize) -> f32 {
        let max = self.max_weight(mode);
        if max <= 0.0 {
            0.0
        } else {
            (self.weight(turn, mode) / max).clamp(0.0, 1.0)
        }
    }

    /// The Markdown export: one table row per call, one section per turn.
    pub(crate) fn to_markdown(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::tr_args;
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", lang.tr("audit.export.title")));
        let summary = [
            ("audit.summary.turns", self.totals.turns.to_string()),
            ("audit.summary.calls", self.totals.calls.to_string()),
            ("audit.summary.reads", self.totals.files_read.to_string()),
            (
                "audit.summary.writes",
                self.totals.files_written.to_string(),
            ),
            ("audit.summary.commands", self.totals.commands.to_string()),
            ("audit.summary.refusals", self.totals.refusals.to_string()),
            ("audit.summary.failed", self.totals.failed.to_string()),
            (
                "audit.summary.duration",
                duration_text(self.totals.duration_ms),
            ),
        ];
        for (key, value) in summary {
            out.push_str(&format!("- {}\n", tr_args(lang, key, &[("count", value)])));
        }
        let unnamed = lang.tr("audit.ruleUnnamed");
        let rules = self
            .rule_counts()
            .iter()
            .map(|(rule, count)| {
                format!("{} x{}", rule.as_deref().unwrap_or(unnamed.as_str()), count)
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !rules.is_empty() {
            out.push_str(&format!(
                "- {}: {}\n",
                lang.tr("audit.summary.rules"),
                rules
            ));
        }
        // The policy that produced those decisions, and the results it counted.
        if self.policy.checks() > 0
            || !self.policy.permission.is_empty()
            || self.policy.error.is_some()
        {
            out.push_str(&format!(
                "- {}: {}\n",
                lang.tr("audit.policy.title"),
                tr_args(
                    lang,
                    "audit.policy.line",
                    &[
                        ("evaluator", self.policy.evaluator.clone()),
                        ("permission", self.policy.permission.clone()),
                        ("checks", self.policy.checks().to_string()),
                        ("allowed", self.policy.allows.total.to_string()),
                        ("denied", self.policy.denials_total.to_string()),
                    ],
                )
            ));
            out.push_str(&format!(
                "- {}: {}\n",
                lang.tr("audit.policy.allows"),
                tr_args(
                    lang,
                    "audit.policy.allowsLine",
                    &[
                        ("reads", self.policy.allows.reads.to_string()),
                        ("writes", self.policy.allows.writes.to_string()),
                        ("egress", self.policy.allows.egress.to_string()),
                        ("spawns", self.policy.allows.spawns.to_string()),
                        ("capabilities", self.policy.allows.capabilities.to_string()),
                    ],
                )
            ));
            // The policy that was APPLIED: preset and the scopes it installed.
            out.push_str(&format!(
                "- {}: {}\n",
                lang.tr("audit.policy.applied"),
                tr_args(
                    lang,
                    "audit.policy.appliedLine",
                    &[
                        (
                            "preset",
                            if self.policy.preset.is_empty() {
                                lang.tr("audit.policy.noPreset")
                            } else {
                                self.policy.preset.clone()
                            },
                        ),
                        ("read", self.policy.read_scope.to_string()),
                        ("write", self.policy.write_scope.to_string()),
                        ("deny", self.policy.deny_roots.to_string()),
                        ("egress", self.policy.egress_allowlist.to_string()),
                        (
                            "egressMode",
                            lang.tr(if self.policy.egress_restricted {
                                "audit.policy.egressRestricted"
                            } else {
                                "audit.policy.egressOpen"
                            }),
                        ),
                    ],
                )
            ));
            // The EXECUTION results this session's calls produced.
            out.push_str(&format!(
                "- {}: {}\n",
                lang.tr("audit.policy.exec"),
                tr_args(
                    lang,
                    "audit.policy.execLine",
                    &[
                        ("ok", self.totals.succeeded.to_string()),
                        ("failed", self.totals.failed.to_string()),
                        ("pending", self.totals.pending.to_string()),
                        ("denied", self.totals.refusals.to_string()),
                    ],
                )
            ));
            if let Some(error) = &self.policy.error {
                out.push_str(&format!(
                    "- {}\n",
                    tr_args(lang, "audit.policy.error", &[("error", error.clone())])
                ));
            }
        }
        out.push_str(&format!(
            "\n{}\n\n{}\n\n",
            lang.tr("audit.limits.shell"),
            lang.tr("audit.limits.network")
        ));
        for turn in &self.turns {
            let started = turn
                .started_at
                .map(|at| format!(" \u{2014} {}", crate::state::stamp(at / 1_000)))
                .unwrap_or_default();
            out.push_str(&format!(
                "## {}{} \u{2014} {}\n\n",
                lang.tr("chat.railTurn")
                    .replace("{turn}", &turn.index.to_string()),
                started,
                duration_text(turn.duration_ms)
            ));
            if turn.calls.is_empty() {
                out.push_str(&format!("{}\n\n", lang.tr("audit.noCalls")));
            } else {
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n| --- | --- | --- | --- | --- | --- |\n",
                    lang.tr("audit.table.tool"),
                    lang.tr("audit.table.category"),
                    lang.tr("audit.table.target"),
                    lang.tr("audit.table.outcome"),
                    lang.tr("audit.table.ms"),
                    lang.tr("audit.table.preview"),
                ));
                for call in &turn.calls {
                    out.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} |\n",
                        cell(&call.tool),
                        cell(&lang.tr(call.category.key())),
                        cell(&call.target),
                        cell(&lang.tr(call.outcome.key())),
                        cell(&call.ms.map(|ms| ms.to_string()).unwrap_or_default()),
                        cell(&call.preview),
                    ));
                }
                out.push('\n');
            }
            if !turn.refusals.is_empty() {
                out.push_str(&format!("### {}\n\n", lang.tr("audit.table.refusals")));
                for refusal in &turn.refusals {
                    out.push_str(&format!(
                        "- {} \u{2014} {} {} {} \u{2014} {} ({})\n",
                        refusal
                            .rule
                            .clone()
                            .unwrap_or_else(|| lang.tr("audit.ruleUnnamed")),
                        lang.tr(action_key(&refusal.action)),
                        refusal.tool,
                        refusal.target,
                        lang.tr("audit.refusedNote"),
                        refusal.permission,
                    ));
                    if let Some(reason) = &refusal.reason {
                        out.push_str(&format!("  - {}\n", reason.replace('\n', " ")));
                    }
                }
                out.push('\n');
            }
        }
        out
    }

    /// The JSON export: the view itself, one object per call.
    pub(crate) fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Escape one Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// Does a call answer the search field?
pub(crate) fn matches_call(call: &AuditCall, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    [&call.tool, &call.target, &call.preview]
        .iter()
        .any(|field| field.to_lowercase().contains(&needle))
        || call.ms.is_some_and(|ms| ms.to_string().contains(&needle))
}

/// Does a refusal answer the search field?
pub(crate) fn matches_refusal(refusal: &AuditRefusal, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    [
        refusal.tool.as_str(),
        refusal.target.as_str(),
        refusal.permission.as_str(),
        refusal.rule.as_deref().unwrap_or(""),
        refusal.reason.as_deref().unwrap_or(""),
    ]
    .iter()
    .any(|field| field.to_lowercase().contains(&needle))
}

/// Does a turn answer the search field (through its own label or any child)?
pub(crate) fn matches_turn(turn: &AuditTurn, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    format!("turn {}", turn.index).contains(&needle)
        || turn.calls.iter().any(|call| matches_call(call, query))
        || turn
            .refusals
            .iter()
            .any(|refusal| matches_refusal(refusal, query))
}

/// The full stored record for a call, read from the session on demand.
///
/// This is deliberately not part of the view model: a caller reaches it only
/// after a reader opens the call, so the frame that draws the audit never
/// clones an output it is not showing.
pub(crate) fn full_text(session: &Session, call: &AuditCall) -> String {
    let Some(message) = session.messages.get(call.message) else {
        return String::new();
    };
    let Some(tool) = message.tools.get(call.tool_index) else {
        return String::new();
    };
    let output = tool.output.as_deref().unwrap_or("").trim();
    if output.is_empty() {
        tool.args.clone()
    } else {
        format!("{}\n\n{}", tool.args, output)
    }
}

/// The turn's start and end, from its recorded timings and call clocks.
fn span_of(message: &ChatMessage) -> (Option<u64>, Option<u64>) {
    let mut start = message.timing().and_then(|timing| timing.start);
    let mut end = message
        .timing()
        .and_then(|timing| timing.start.zip(timing.ms).map(|(at, ms)| at + ms));
    for tool in &message.tools {
        let Some(at) = tool.at else {
            continue;
        };
        start = Some(start.map_or(at, |current| current.min(at)));
        let finish = at + tool.ms.unwrap_or(0);
        end = Some(end.map_or(finish, |current| current.max(finish)));
    }
    (start, end)
}

/// How a stored output reads: the engine prefixes a tool failure with "Error: ",
/// and an absent output is a call still running.
fn outcome_of(output: Option<&str>) -> AuditOutcome {
    match output {
        None => AuditOutcome::Pending,
        Some(output) if output.starts_with("Error:") || output.starts_with("Error\u{ff1a}") => {
            AuditOutcome::Failed
        }
        Some(_) => AuditOutcome::Ok,
    }
}

/// The key fields a target may come from, in priority order.
const TARGET_KEYS: [&str; 10] = [
    "command",
    "file_path",
    "path",
    "url",
    "query",
    "pattern",
    "instruction",
    "prompt",
    "name",
    "target",
];

/// The path, host or command a call named.
fn target_of(args: &str) -> String {
    let value: Option<serde_json::Value> = serde_json::from_str(args).ok();
    if let Some(value) = value {
        for key in TARGET_KEYS {
            if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
                return preview_bounded(text, TARGET_LIMIT);
            }
        }
        if let Some(text) = first_string(&value) {
            return preview_bounded(text, TARGET_LIMIT);
        }
    }
    preview_bounded(args, TARGET_LIMIT)
}

/// The first string inside a JSON value, in the serializer's own order.
fn first_string(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Array(items) => items.iter().find_map(first_string),
        serde_json::Value::Object(map) => map.values().find_map(first_string),
        _ => None,
    }
}

/// A redacted, flattened, bounded preview.
pub(crate) fn preview(text: &str) -> String {
    preview_bounded(text, PREVIEW_LIMIT)
}

/// A redacted, flattened preview bounded to max characters.
///
/// Redaction runs BEFORE the cut, so a secret cannot survive as a fragment. The
/// scan is bounded first, so the work follows the preview rather than the
/// payload.
pub(crate) fn preview_bounded(text: &str, max: usize) -> String {
    let scan: String = text.chars().take(PREVIEW_SCAN).collect();
    let redacted = redact_secrets(&scan);
    let flat = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{}...", cut.trim_end())
}

/// The keys whose values are redacted from a preview.
const SECRET_KEYS: [&str; 12] = [
    "authorization",
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "token",
    "secret",
    "password",
    "passphrase",
    "credential",
    "bearer",
    "env",
];

/// A word character for boundary purposes: a key only matches as a whole
/// segment, so "environment" is not "env" and "mytoken" is not "token".
fn word_char(ch: char) -> bool {
    ch.is_alphanumeric()
}

/// Start boundary: the key may follow a path separator, a quote, or "_", but
/// not a letter or digit.
fn starts_segment(chars: &[char], at: usize) -> bool {
    at == 0 || !word_char(chars[at - 1])
}

/// End boundary: the key must end before punctuation, whitespace or a quote.
fn ends_segment(chars: &[char], at: usize) -> bool {
    at >= chars.len() || !word_char(chars[at])
}

/// Redact secret-looking values: KEY=value, key: value, JSON string fields,
/// and Bearer tokens.
///
/// The rule is deliberately about SHAPE, not about knowing a credential: an
/// audit preview is read long after the fact, and a value that looks like a
/// credential must not be reproduced even if this process never saw the real
/// one.
pub(crate) fn redact_secrets(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|ch| ch.to_ascii_lowercase()).collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if let Some((length, by_bearer)) = key_at(&lower, index) {
            push(&mut out, &chars[index..index + length]);
            let mut cursor = index + length;
            if by_bearer {
                let spaces = skip_space(&chars, cursor);
                push(&mut out, &chars[cursor..spaces]);
                if spaces < chars.len() {
                    let end = redacted_token_end(&chars, spaces);
                    if end > spaces {
                        out.push_str("***");
                    }
                    cursor = end;
                } else {
                    cursor = spaces;
                }
                index = cursor;
                continue;
            }
            // A JSON key carries a closing quote before its colon.
            if cursor < chars.len() && matches!(chars[cursor], '"' | '\'') {
                out.push(chars[cursor]);
                cursor += 1;
            }
            let spaces = skip_space(&chars, cursor);
            push(&mut out, &chars[cursor..spaces]);
            cursor = spaces;
            if cursor < chars.len() && matches!(chars[cursor], ':' | '=') {
                out.push(chars[cursor]);
                cursor += 1;
                let spaces = skip_space(&chars, cursor);
                push(&mut out, &chars[cursor..spaces]);
                cursor = spaces;
                if cursor < chars.len() && matches!(chars[cursor], '"' | '\'') {
                    let quote = chars[cursor];
                    out.push(quote);
                    cursor += 1;
                    let value_start = cursor;
                    while cursor < chars.len() && chars[cursor] != quote && chars[cursor] != '\n' {
                        cursor += 1;
                    }
                    if cursor > value_start {
                        out.push_str("***");
                    }
                    if cursor < chars.len() && chars[cursor] == quote {
                        out.push(quote);
                        cursor += 1;
                    }
                } else {
                    let value_end = value_end(&chars, cursor);
                    if value_end > cursor {
                        out.push_str("***");
                    }
                    cursor = value_end;
                }
            }
            index = cursor;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// Append a slice of characters to the output.
fn push(out: &mut String, chars: &[char]) {
    for ch in chars {
        out.push(*ch);
    }
}

/// The index of the first non-whitespace character at or after from.
fn skip_space(chars: &[char], from: usize) -> usize {
    let mut at = from;
    while at < chars.len() && chars[at].is_whitespace() {
        at += 1;
    }
    at
}

/// Where an unquoted value ends: a delimiter, or the line.
fn value_end(chars: &[char], from: usize) -> usize {
    let mut at = from;
    while at < chars.len() && !matches!(chars[at], ',' | ';' | '}' | ']' | '\n') {
        at += 1;
    }
    at
}

/// Where a space-delimited bearer token ends.
fn redacted_token_end(chars: &[char], from: usize) -> usize {
    let mut at = from;
    while at < chars.len()
        && !chars[at].is_whitespace()
        && !matches!(chars[at], '"' | '\'' | ',' | ';' | '}' | ']')
    {
        at += 1;
    }
    at
}

/// The key at this character, longest first, and whether it is the bearer
/// scheme rather than a key/value field.
fn key_at(lower: &[char], at: usize) -> Option<(usize, bool)> {
    if !starts_segment(lower, at) {
        return None;
    }
    let rest = &lower[at..];
    let mut best: Option<(usize, bool)> = None;
    for key in SECRET_KEYS {
        let key_chars: Vec<char> = key.chars().collect();
        if rest.len() >= key_chars.len()
            && rest[..key_chars.len()] == key_chars[..]
            && ends_segment(lower, at + key_chars.len())
            && best.is_none_or(|(length, _)| key_chars.len() > length)
        {
            best = Some((key_chars.len(), key == "bearer"));
        }
    }
    best
}

/// The locale key an action the policy was asked about renders under.
pub(crate) fn action_key(action: &str) -> &'static str {
    match action {
        "read" => "audit.action.read",
        "write" => "audit.action.write",
        "egress" => "audit.action.egress",
        "spawn" => "audit.action.spawn",
        "capability" => "audit.action.capability",
        _ => "audit.action.other",
    }
}

/// A duration the export prints.
pub(crate) fn duration_text(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else {
        let seconds = ms as f64 / 1_000.0;
        if ms < 10_000 {
            format!("{seconds:.2} s")
        } else {
            format!("{seconds:.1} s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        full_text, outcome_of, preview, preview_bounded, redact_secrets, AuditAllows,
        AuditCategory, AuditOutcome, AuditPolicy, AuditView, PREVIEW_LIMIT, TARGET_LIMIT,
    };
    use crate::model::{ChatMessage, Session, ToolEvent};
    use agent::policy::{Action, PolicyAuditEntry, Target};

    fn tool(name: &str, args: &str, output: Option<&str>, ms: u64, at: u64) -> ToolEvent {
        ToolEvent {
            name: name.to_string(),
            args: args.to_string(),
            output: output.map(str::to_string),
            ms: Some(ms),
            at: Some(at),
        }
    }

    fn session_with(turns: usize) -> Session {
        let mut session = Session::new("w", "audit");
        session.id = "audit".to_string();
        for turn in 0..turns {
            let mut user = ChatMessage::user(format!("turn {turn}"));
            user.timing_mut().start = Some(1_000 + turn as u64 * 10_000);
            session.messages.push(user);
            let mut assistant = ChatMessage::assistant();
            assistant.text = "done".to_string();
            assistant.timing_mut().start = Some(1_000 + turn as u64 * 10_000);
            assistant.timing_mut().ms = Some(2_000);
            session.messages.push(assistant);
        }
        session
    }

    /// Every family the table names is classified from the tool's own name, and
    /// a connected MCP namespace wins over the generic rules.
    #[test]
    fn every_category_has_a_name_rule() {
        assert_eq!(
            AuditCategory::classify("read_file"),
            AuditCategory::FileRead
        );
        assert_eq!(AuditCategory::classify("list_dir"), AuditCategory::FileRead);
        assert_eq!(
            AuditCategory::classify("write_file"),
            AuditCategory::FileWrite
        );
        assert_eq!(
            AuditCategory::classify("edit_file"),
            AuditCategory::FileWrite
        );
        assert_eq!(AuditCategory::classify("bash"), AuditCategory::Shell);
        assert_eq!(AuditCategory::classify("web_fetch"), AuditCategory::Network);
        assert_eq!(
            AuditCategory::classify("web_search"),
            AuditCategory::Network
        );
        assert_eq!(AuditCategory::classify("subagent"), AuditCategory::Subagent);
        assert_eq!(AuditCategory::classify("update_plan"), AuditCategory::Other);
        let namespaces = vec!["github".to_string(), "files".to_string()];
        assert_eq!(
            AuditCategory::classify_in("github_search", &namespaces),
            AuditCategory::Mcp
        );
        assert_eq!(
            AuditCategory::classify_in("files_read_file", &namespaces),
            AuditCategory::Mcp,
            "the namespace wins over the file-family name"
        );
        assert_eq!(
            AuditCategory::classify_in("read_file", &namespaces),
            AuditCategory::FileRead,
            "a bare name is not a namespace"
        );
    }

    /// A credential-shaped value cannot survive into a preview, however it is
    /// written. The benign parts of the line are kept, so the preview still
    /// explains what happened.
    #[test]
    fn a_secret_never_survives_redaction() {
        let cases = [
            ("API_KEY=sk-live-123", "API_KEY=***"),
            ("api_key: sk-live-123", "api_key: ***"),
            (
                r#"{"api_key":"sk-live-123","ok":true}"#,
                r#""api_key":"***""#,
            ),
            ("Authorization: Bearer abc.def", "Authorization: ***"),
            (
                "curl -H 'Authorization: Bearer abc.def'",
                "curl -H 'Authorization: ***",
            ),
            ("token=abc123", "token=***"),
            ("DB_PASSWORD=hunter2", "DB_PASSWORD=***"),
            ("env: FOO=bar", "env: ***"),
            ("the token is fine", "the token is fine"),
            ("environment=prod", "environment=prod"),
        ];
        for (input, expected) in cases {
            let redacted = redact_secrets(input);
            assert!(
                !redacted.contains("sk-live-123")
                    && !redacted.contains("abc.def")
                    && !redacted.contains("abc123")
                    && !redacted.contains("hunter2"),
                "a secret survived {input:?}: {redacted:?}"
            );
            assert!(
                redacted.contains(expected),
                "the benign shape is kept for {input:?}: {redacted:?}"
            );
        }
    }

    /// A preview is bounded by characters, not bytes, and says it was cut.
    #[test]
    fn a_huge_preview_is_bounded_at_a_character_boundary() {
        let huge = "\u{1f600}".repeat(PREVIEW_LIMIT * 4);
        let previewed = preview(&huge);
        assert!(
            previewed.chars().count() <= PREVIEW_LIMIT + 3,
            "bounded: {}",
            previewed.chars().count()
        );
        assert!(previewed.ends_with("..."));
        let block = "0123456789".repeat(10_000);
        assert_eq!(preview_bounded(&block, 10), "0123456789...");
    }

    /// The totals are exactly the events: one count per turn, per call and per
    /// family, with failed calls called out.
    #[test]
    fn the_totals_equal_the_events() {
        let mut session = session_with(2);
        session.messages[1].tools.push(tool(
            "read_file",
            r#"{"path":"notes/a.md"}"#,
            Some("body"),
            100,
            1_200,
        ));
        session.messages[1].tools.push(tool(
            "bash",
            r#"{"command":"ls"}"#,
            Some("Error: refused"),
            20,
            1_400,
        ));
        session.messages[3].tools.push(tool(
            "write_file",
            r#"{"path":"out.txt"}"#,
            Some("written"),
            30,
            11_200,
        ));
        session.messages[3].tools.push(tool(
            "subagent",
            r#"{"prompt":"go"}"#,
            Some("done"),
            40,
            11_300,
        ));
        let view = AuditView::build(&session, &[], &[]);
        assert_eq!(view.totals.turns, 2);
        assert_eq!(view.totals.calls, 4);
        assert_eq!(view.totals.files_read, 1);
        assert_eq!(view.totals.files_written, 1);
        assert_eq!(view.totals.commands, 1);
        assert_eq!(view.totals.failed, 1);
        assert_eq!(view.totals.succeeded, 3);
        assert_eq!(view.totals.pending, 0);
        assert_eq!(view.totals.refusals, 0);
        assert_eq!(view.totals.duration_ms, 4_000, "two 2 s turns");
        assert_eq!(view.turns[0].calls.len(), 2);
        assert_eq!(view.turns[1].calls.len(), 2);
        assert_eq!(view.turns[0].calls[0].target, "notes/a.md");
        assert_eq!(view.turns[0].calls[1].target, "ls");
        assert_eq!(view.turns[0].calls[1].outcome, AuditOutcome::Failed);
        assert_eq!(view.turns[0].calls[0].outcome, AuditOutcome::Ok);
    }

    /// A refusal lands in the turn whose span contains it, and a sibling
    /// session's refusal is not borrowed.
    #[test]
    fn a_refusal_lands_in_the_turn_it_happened_in() {
        let session = session_with(2);
        let refusal = |at: u64, target: &str, rule: &str, session: &str| PolicyAuditEntry {
            at_ms: at,
            session: session.to_string(),
            preset: "default".to_string(),
            tool: "write_file".to_string(),
            permission: "read-only".to_string(),
            action: Action::Write,
            target: target.to_string(),
            rule: Some(rule.to_string()),
            reason: Some("outside the workspace".to_string()),
        };
        let denials = vec![
            refusal(1_500, "/etc/passwd", "deny-outside", "audit"),
            refusal(11_500, "/tmp/x", "deny-temp", "audit"),
            refusal(11_600, "/tmp/y", "deny-other-session", "someone-else"),
        ];
        let view = AuditView::build(&session, &denials, &[]);
        assert_eq!(view.totals.refusals, 2, "scoped to this session");
        assert_eq!(
            view.turns[0].refusals[0].action, "write",
            "the refused action is recorded with the decision"
        );
        assert_eq!(
            view.turns[0].refusals[0].permission, "read-only",
            "and the permission in force at that decision"
        );
        assert_eq!(view.turns[0].refusals.len(), 1);
        assert_eq!(
            view.turns[0].refusals[0].rule.as_deref(),
            Some("deny-outside")
        );
        assert_eq!(view.turns[1].refusals.len(), 1);
        assert_eq!(view.turns[1].refusals[0].rule.as_deref(), Some("deny-temp"));
        assert_eq!(
            view.rule_counts(),
            vec![
                (Some("deny-outside".to_string()), 1),
                (Some("deny-temp".to_string()), 1)
            ]
        );
    }

    /// The policy that answered the questions is part of the audit: its
    /// evaluator, effective permission, counted allows and total denials are
    /// carried beside the refusal rows, not inferred from the tool list.
    #[test]
    fn the_policy_results_are_carried_and_exported() {
        let view = AuditView::build(&session_with(1), &[], &[]).with_policy(AuditPolicy {
            permission: "workspace-write".to_string(),
            evaluator: "bytecode".to_string(),
            error: Some("baseline failed to compile".to_string()),
            allows: AuditAllows {
                total: 7,
                reads: 3,
                writes: 2,
                egress: 1,
                spawns: 1,
                capabilities: 0,
            },
            denials_total: 2,
            preset: "review".to_string(),
            read_scope: 2,
            write_scope: 1,
            deny_roots: 3,
            egress_allowlist: 4,
            egress_restricted: true,
        });
        assert_eq!(view.policy.checks(), 9);
        assert_eq!(view.policy.allows.total, 7);
        assert_eq!(view.policy.preset, "review");
        let json: serde_json::Value = serde_json::from_str(&view.to_json()).expect("valid JSON");
        assert_eq!(json["policy"]["permission"], "workspace-write");
        assert_eq!(json["policy"]["evaluator"], "bytecode");
        assert_eq!(json["policy"]["allows"]["reads"], 3);
        assert_eq!(json["policy"]["denials_total"], 2);
        assert_eq!(json["policy"]["read_scope"], 2);
        assert_eq!(json["policy"]["egress_restricted"], true);
        let markdown = view.to_markdown(crate::i18n::Lang::En);
        assert!(markdown.contains("workspace-write"));
        assert!(markdown.contains("bytecode"));
        assert!(markdown.contains("baseline failed to compile"));
        assert!(markdown.contains("9 checks"));
        assert!(markdown.contains("7 allowed"));
        assert!(markdown.contains("review"), "the applied preset is named");
        assert!(markdown.contains("read scope 2"), "the applied scopes");
        assert!(markdown.contains("0 ok"), "the exec results are counted");
        assert!(markdown.contains("2 denied"));
    }

    /// Compaction moves the request boundary; it does not remove rows, so the
    /// audit still lists every call above and below the fold.
    #[test]
    fn a_compaction_does_not_drop_rows() {
        let mut session = session_with(2);
        session.messages[1]
            .tools
            .push(tool("bash", r#"{"command":"one"}"#, Some("1"), 5, 1_100));
        session.messages[3]
            .tools
            .push(tool("bash", r#"{"command":"two"}"#, Some("2"), 5, 11_100));
        session.compacted = 2;
        let view = AuditView::build(&session, &[], &[]);
        assert_eq!(view.totals.calls, 2, "the folded turn is still audited");
        assert_eq!(view.turns[0].calls[0].target, "one");
    }

    /// The full record is read only through the indices the view carries, and
    /// it is the whole stored arguments-plus-output text.
    #[test]
    fn the_full_text_is_the_stored_record() {
        let mut session = session_with(1);
        let huge = "x".repeat(200_000);
        session.messages[1].tools.push(tool(
            "bash",
            r#"{"command":"cat big"}"#,
            Some(&huge),
            9,
            1_100,
        ));
        let view = AuditView::build(&session, &[], &[]);
        let call = &view.turns[0].calls[0];
        assert!(call.preview.chars().count() <= PREVIEW_LIMIT + 3);
        let full = full_text(&session, call);
        assert!(full.contains(&huge), "the full record is complete");
        assert!(full.starts_with(r#"{"command":"cat big"}"#));
        assert!(call.target.chars().count() <= TARGET_LIMIT + 3);
    }

    /// An absent output is a running call, and the engine's error marker is the
    /// only failure signal the stored record carries.
    #[test]
    fn outcomes_read_the_record_without_guessing() {
        assert_eq!(outcome_of(None), AuditOutcome::Pending);
        assert_eq!(outcome_of(Some("ok")), AuditOutcome::Ok);
        assert_eq!(outcome_of(Some("Error: nope")), AuditOutcome::Failed);
    }

    /// The JSON export is valid and carries one object per call; the Markdown
    /// export carries one table row per call and the refusal rule.
    #[test]
    fn the_exports_carry_one_row_per_call() {
        let mut session = session_with(2);
        session.messages[1]
            .tools
            .push(tool("read_file", r#"{"path":"a"}"#, Some("A"), 1, 1_100));
        session.messages[1]
            .tools
            .push(tool("bash", r#"{"command":"ls"}"#, Some("B"), 2, 1_200));
        let denials = [PolicyAuditEntry {
            at_ms: 1_500,
            session: "audit".to_string(),
            preset: "default".to_string(),
            tool: "write_file".to_string(),
            permission: "read-only".to_string(),
            action: Action::Write,
            target: "/etc/x".to_string(),
            rule: Some("deny-outside".to_string()),
            reason: None,
        }];
        let view = AuditView::build(&session, &denials, &[]);
        let json: serde_json::Value = serde_json::from_str(&view.to_json()).expect("valid JSON");
        let calls: usize = json["turns"]
            .as_array()
            .expect("turns")
            .iter()
            .map(|turn| turn["calls"].as_array().map_or(0, Vec::len))
            .sum();
        assert_eq!(calls, view.totals.calls, "one object per call");
        let markdown = view.to_markdown(crate::i18n::Lang::En);
        for call in view.turns.iter().flat_map(|turn| &turn.calls) {
            assert!(
                markdown.contains(&call.tool),
                "the Markdown lists {}",
                call.tool
            );
        }
        assert!(markdown.contains("deny-outside"), "the rule is named");
        assert!(
            markdown.contains("did NOT happen"),
            "the refusal explains itself"
        );
    }

    /// The toolbar's measure is live: duration, equal turn shares, call count.
    #[test]
    fn the_measure_follows_the_toolbar_mode() {
        let mut session = session_with(2);
        session.messages[1].tools.push(tool(
            "bash",
            r#"{"command":"slow"}"#,
            Some("ok"),
            9_000,
            1_100,
        ));
        for _ in 0..3 {
            session.messages[3].tools.push(tool(
                "bash",
                r#"{"command":"quick"}"#,
                Some("ok"),
                10,
                11_100,
            ));
        }
        let view = AuditView::build(&session, &[], &[]);
        let (first, second) = (&view.turns[0], &view.turns[1]);
        assert!(view.weight(first, 0) > view.weight(second, 0), "duration");
        assert_eq!(view.weight(first, 1), view.weight(second, 1), "turns");
        assert!(view.weight(second, 2) > view.weight(first, 2), "calls");
        assert_eq!(view.share(first, 1), 1.0);
    }

    /// A session with no rows is an empty view, not a panic.
    #[test]
    fn an_empty_session_has_an_empty_view() {
        let session = Session::new("w", "empty");
        let view = AuditView::build(&session, &[], &[]);
        assert_eq!(view.totals, Default::default());
        assert!(view.turns.is_empty());
        assert!(view.rule_counts().is_empty());
    }

    /// A secret in the ARGUMENTS is redacted from both the target and the
    /// preview, even when no output exists yet.
    #[test]
    fn a_secret_in_the_arguments_never_reaches_the_preview() {
        let mut session = session_with(1);
        session.messages[1].tools.push(ToolEvent {
            name: "bash".to_string(),
            args: r#"{"command":"curl -H 'Authorization: Bearer sk-abc-123' https://x"}"#
                .to_string(),
            output: None,
            ms: None,
            at: None,
        });
        let view = AuditView::build(&session, &[], &[]);
        let call = &view.turns[0].calls[0];
        assert_eq!(call.outcome, AuditOutcome::Pending);
        assert!(!call.preview.contains("sk-abc-123"), "{}", call.preview);
        assert!(!call.target.contains("sk-abc-123"), "{}", call.target);
    }

    /// The search field filters turns through their calls, refusals and label,
    /// and an absent needle keeps everything.
    #[test]
    fn the_search_finds_calls_refusals_and_turn_labels() {
        let mut session = session_with(1);
        session.messages[1].tools.push(tool(
            "read_file",
            r#"{"path":"notes/secret.md"}"#,
            Some("body"),
            4,
            1_100,
        ));
        let denial = PolicyAuditEntry {
            at_ms: 1_500,
            session: "audit".to_string(),
            preset: String::new(),
            tool: "write_file".to_string(),
            permission: "read-only".to_string(),
            action: Action::Write,
            target: "/etc/passwd".to_string(),
            rule: Some("deny-outside".to_string()),
            reason: None,
        };
        let view = AuditView::build(&session, &[denial], &[]);
        let turn = &view.turns[0];
        assert!(super::matches_turn(turn, ""));
        assert!(super::matches_turn(turn, "notes"));
        assert!(super::matches_turn(turn, "passwd"));
        assert!(super::matches_turn(turn, "turn 1"));
        assert!(!super::matches_turn(turn, "absent"));
        assert!(super::matches_refusal(&turn.refusals[0], "deny-outside"));
        assert!(!super::matches_call(&turn.calls[0], "deny-outside"));
    }

    /// An old record with no clock still classifies and lists, and its refusals
    /// are placed at the end of the ring's order rather than dropped.
    #[test]
    fn a_record_without_clocks_still_lists_every_call() {
        let mut session = Session::new("w", "old");
        session.id = "old".to_string();
        session.messages.push(ChatMessage::user("hello"));
        let mut assistant = ChatMessage::assistant();
        assistant.tools.push(ToolEvent {
            name: "bash".to_string(),
            args: r#"{"command":"ls"}"#.to_string(),
            output: Some("a".to_string()),
            ms: None,
            at: None,
        });
        session.messages.push(assistant);
        let denial = PolicyAuditEntry {
            at_ms: 50,
            session: "old".to_string(),
            preset: String::new(),
            tool: "read_file".to_string(),
            permission: "read-only".to_string(),
            action: Action::Read,
            target: Target::path("/x").display(),
            rule: None,
            reason: None,
        };
        let view = AuditView::build(&session, &[denial], &[]);
        assert_eq!(view.totals.calls, 1);
        assert_eq!(view.totals.refusals, 1, "a clockless log keeps it");
        assert_eq!(view.turns[0].refusals.len(), 1);
        assert_eq!(view.turns[0].started_at, None);
    }
}
