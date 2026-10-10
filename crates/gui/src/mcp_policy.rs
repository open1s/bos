//! Per-tool policy for MCP servers: which tool an agent may see at all, and
//! which one has to be approved even where nothing else is.
//!
//! The agent crate carries the seam — `agent::mcp::policy::{McpPolicy, McpRule,
//! McpEffect}` — but **nothing consults it**, so a connected server's tools were
//! attached whole: enabling a server granted every tool it advertises, and the
//! only gate was the workspace-wide `require_approval`. This module is the
//! missing half. It is a durable, editable policy consulted **where the tools
//! are attached**, so a denied tool never reaches the registry (the model cannot
//! see it, let alone call it) and an `ask` tool is gated in a workspace that
//! gates nothing else.
//!
//! Precedence is stated once and tested: **deny beats ask beats allow**, either
//! half of a rule may be `*`, a pair with several matching rules keeps the
//! strictest, and a pair with none takes the policy default. The workspace-wide
//! gate is a floor and not a ceiling: a per-tool `allow` never removes it,
//! because a policy surface must not be able to weaken a setting the user made
//! somewhere else.
//!
//! `decide` is the one place the answer is computed, so the enforcement path and
//! the pane cannot disagree — the defect this project keeps finding is a surface
//! that reports a rule nobody applies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::tools::AsyncTool;
use agent::Agent;
use serde::{Deserialize, Serialize};

/// Version of the on-disk document.
pub(crate) const FILE_VERSION: u32 = 1;
/// Bounds on the document, refused with the number named rather than truncated.
pub(crate) const MAX_RULES: usize = 200;
/// Longest server or tool name a rule may carry.
pub(crate) const MAX_NAME_CHARS: usize = 64;
/// What a pair with no matching rule gets.
pub(crate) const DEFAULT_EFFECT: Effect = Effect::Allow;

/// What the policy says about one `(server, tool)` pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Effect {
    /// Attached, with no gate of its own.
    Allow,
    /// Attached, but every call waits for approval.
    Ask,
    /// Never attached: the tool does not exist as far as the agent knows.
    Deny,
}

impl Default for Effect {
    fn default() -> Self {
        DEFAULT_EFFECT
    }
}

impl Effect {
    /// Parse the wire name.
    pub(crate) fn parse(text: &str) -> Option<Effect> {
        match text.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Effect::Allow),
            "ask" => Some(Effect::Ask),
            "deny" => Some(Effect::Deny),
            _ => None,
        }
    }

    /// Precedence: a larger rank wins when several rules match one pair.
    fn rank(self) -> u8 {
        match self {
            Effect::Allow => 1,
            Effect::Ask => 2,
            Effect::Deny => 3,
        }
    }
}

/// One rule: an effect for a `(server, tool)` pair, either half of which may be
/// `*`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rule {
    /// Server name (the MCP namespace), or `*`.
    pub server: String,
    /// Tool name, or `*`.
    pub tool: String,
    /// What to do with a match.
    pub effect: Effect,
}

impl Rule {
    /// Build a rule, trimming both halves.
    pub(crate) fn new(server: &str, tool: &str, effect: Effect) -> Self {
        Self {
            server: server.trim().to_string(),
            tool: tool.trim().to_string(),
            effect,
        }
    }

    /// The pair this rule is stored under, for display: `srv/*`, `*`.
    pub(crate) fn key(&self) -> String {
        format!("{}/{}", self.server, self.tool)
    }

    /// A rule with an empty or over-long half is dropped on load.
    pub(crate) fn is_valid(&self) -> bool {
        self.server.len() <= MAX_NAME_CHARS
            && self.tool.len() <= MAX_NAME_CHARS
            && !self.server.trim().is_empty()
            && !self.tool.trim().is_empty()
    }

    /// Whether this rule speaks about `(server, tool)`.
    pub(crate) fn matches(&self, server: &str, tool: &str) -> bool {
        matches_pattern(&self.server, server) && matches_pattern(&self.tool, tool)
    }

    fn from_value(value: &serde_json::Value) -> Option<Rule> {
        let object = value.as_object()?;
        let server = object.get("server")?.as_str()?.trim();
        let tool = object.get("tool")?.as_str()?.trim();
        let effect = Effect::parse(object.get("effect")?.as_str()?)?;
        let rule = Rule::new(server, tool, effect);
        rule.is_valid().then_some(rule)
    }
}

/// `*` matches anything; otherwise the names must be equal.
pub(crate) fn matches_pattern(pattern: &str, name: &str) -> bool {
    pattern == "*" || pattern == name
}

/// The answer for one pair, plus **why** — the rule that decided it, or
/// `"default"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Decision {
    /// What happens to the tool.
    pub effect: Effect,
    /// `"srv/*"` for the rule that decided it, `"default"` when none matched.
    pub by: String,
}

impl Decision {
    /// Whether the tool is kept off the agent.
    pub(crate) fn is_denied(&self) -> bool {
        self.effect == Effect::Deny
    }

    /// Whether this pair needs an approval gate of its own.
    pub(crate) fn asks(&self) -> bool {
        self.effect == Effect::Ask
    }
}

/// Whether a tool has to be gated, given the workspace gate and the policy.
///
/// The workspace gate is a **floor**: turning it on gates everything, and a
/// per-tool `allow` cannot turn it back off.
pub(crate) fn gate_for(workspace_gate: bool, decision: &Decision) -> bool {
    workspace_gate || decision.asks()
}

/// The document as written.
#[derive(Clone, Debug, Serialize)]
struct File {
    version: u32,
    default: Effect,
    rules: Vec<Rule>,
}

/// What is read back: deliberately loose, so one bad rule costs one rule.
#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    rules: Vec<serde_json::Value>,
}

/// A tool the policy kept off the agent, and the rule that did it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Denied {
    /// The tool's name, as the server advertises it.
    pub tool: String,
    /// The decision that removed it.
    pub decision: Decision,
}

/// What attaching one server's tools did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Attachment {
    /// Tools now on the agent, in the order they were registered.
    pub attached: Vec<String>,
    /// Tools the policy removed, with the rule responsible.
    pub denied: Vec<Denied>,
}

/// One tool as the pane lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ToolReport {
    /// The tool's name.
    pub name: String,
    /// What the policy says about it.
    pub effect: Effect,
    /// Which rule decided that (`"srv/*"` or `"default"`).
    pub by: String,
}

/// One server as the pane lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ServerReport {
    /// Server name (namespace).
    pub name: String,
    /// Every tool it advertises, with its decision.
    pub tools: Vec<ToolReport>,
    /// How many of them are attached.
    pub attached: usize,
    /// How many the policy removed.
    pub denied: usize,
}

/// The whole policy, plus what it does to the servers currently connected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Report {
    /// Caller's clock, for the pane.
    pub now_ms: u64,
    /// What an unruled pair gets.
    pub default: Effect,
    /// Every rule, in document order.
    pub rules: Vec<Rule>,
    /// Version the loaded file declared (what this build writes when they agree).
    pub file_version: u32,
    /// Version this build writes.
    pub expected_version: u32,
    /// Bounds in force.
    pub max_rules: usize,
    /// Longest name a rule may carry.
    pub max_name_chars: usize,
    /// One entry per connected server.
    pub servers: Vec<ServerReport>,
    /// Totals across those servers.
    pub attached: usize,
    /// Tools removed by the policy.
    pub denied: usize,
    /// Tools that will be gated because of a rule.
    pub asked: usize,
}

/// The durable policy.
pub(crate) struct Store {
    path: PathBuf,
    /// Version the file declared. A document from a newer build is loaded and
    /// reported rather than rewritten behind the user's back.
    version: u32,
    default: Effect,
    rules: Vec<Rule>,
}

impl Store {
    /// `~/.bos/gui/mcp-policy.json`, beside the other GUI stores.
    pub(crate) fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".bos")
            .join("gui")
            .join("mcp-policy.json")
    }

    /// Open the default store, tolerating anything a file can be.
    pub(crate) fn open() -> Self {
        Self::load(&Self::default_path())
    }

    /// Read a store. A missing or unreadable file is the default policy, a
    /// corrupt one is too, and a hand-edited rule that no longer validates is
    /// dropped rather than taken on faith.
    pub(crate) fn load(path: &Path) -> Self {
        let empty = Self {
            path: path.to_path_buf(),
            version: FILE_VERSION,
            default: DEFAULT_EFFECT,
            rules: Vec::new(),
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return empty;
        };
        let Ok(raw) = serde_json::from_str::<RawFile>(&text) else {
            return empty;
        };
        let default = raw
            .default
            .as_deref()
            .and_then(Effect::parse)
            .unwrap_or(DEFAULT_EFFECT);
        let mut rules: Vec<Rule> = Vec::new();
        for value in &raw.rules {
            let Some(rule) = Rule::from_value(value) else {
                continue;
            };
            // Two rules for one pair are one pair: the strictest survives, which
            // is the same rule that decides a conflict at evaluation time.
            match rules
                .iter_mut()
                .find(|existing| existing.server == rule.server && existing.tool == rule.tool)
            {
                Some(existing) => {
                    if rule.effect.rank() > existing.effect.rank() {
                        *existing = rule;
                    }
                }
                None => rules.push(rule),
            }
            if rules.len() >= MAX_RULES {
                break;
            }
        }
        Self {
            path: path.to_path_buf(),
            // A document with no version predates the field; treat it as this
            // build's format rather than as a file from the future.
            version: if raw.version == 0 {
                FILE_VERSION
            } else {
                raw.version
            },
            default,
            rules,
        }
    }

    /// Write the policy, atomically.
    pub(crate) fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = File {
            version: FILE_VERSION,
            default: self.default,
            rules: self.rules.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Set the fallback effect.
    pub(crate) fn set_default(&mut self, effect: Effect) {
        self.default = effect;
    }

    /// What happens to `(server, tool)` and which rule decided it.
    ///
    /// A pair covered by several rules takes the strictest, and among rules of
    /// equal strictness the first in document order — so the reason shown in the
    /// pane is stable.
    pub(crate) fn decide(&self, server: &str, tool: &str) -> Decision {
        let mut best: Option<&Rule> = None;
        for rule in self.rules.iter().filter(|r| r.matches(server, tool)) {
            let better = match best {
                Some(current) => rule.effect.rank() > current.effect.rank(),
                None => true,
            };
            if better {
                best = Some(rule);
            }
        }
        match best {
            Some(rule) => Decision {
                effect: rule.effect,
                by: rule.key(),
            },
            None => Decision {
                effect: self.default,
                by: "default".to_string(),
            },
        }
    }

    /// Set the effect for a pair, replacing any rule already stored for it.
    pub(crate) fn set(&mut self, server: &str, tool: &str, effect: Effect) -> Result<(), String> {
        let rule = Rule::new(server, tool, effect);
        if !rule.is_valid() {
            return Err(format!(
                "a rule needs a server and a tool, each at most {MAX_NAME_CHARS} characters"
            ));
        }
        if let Some(existing) = self
            .rules
            .iter_mut()
            .find(|r| r.server == rule.server && r.tool == rule.tool)
        {
            existing.effect = effect;
            return Ok(());
        }
        if self.rules.len() >= MAX_RULES {
            return Err(format!("at the cap of {MAX_RULES} rules; clear one first"));
        }
        self.rules.push(rule);
        Ok(())
    }

    /// Drop the rule stored for a pair. `false` when there was none.
    pub(crate) fn clear(&mut self, server: &str, tool: &str) -> bool {
        let server = server.trim();
        let tool = tool.trim();
        let before = self.rules.len();
        self.rules
            .retain(|r| !(r.server == server && r.tool == tool));
        self.rules.len() != before
    }

    /// Evaluate the policy against what the connected servers advertise.
    pub(crate) fn report(&self, servers: &[(String, Vec<String>)], now_ms: u64) -> Report {
        let mut rows = Vec::new();
        let mut attached = 0;
        let mut denied = 0;
        let mut asked = 0;
        for (name, tools) in servers {
            let mut listed = Vec::new();
            let mut kept = 0;
            let mut removed = 0;
            for tool in tools {
                let decision = self.decide(name, tool);
                match decision.effect {
                    Effect::Deny => removed += 1,
                    Effect::Ask => {
                        kept += 1;
                        asked += 1;
                    }
                    Effect::Allow => kept += 1,
                }
                listed.push(ToolReport {
                    name: tool.clone(),
                    effect: decision.effect,
                    by: decision.by,
                });
            }
            attached += kept;
            denied += removed;
            rows.push(ServerReport {
                name: name.clone(),
                tools: listed,
                attached: kept,
                denied: removed,
            });
        }
        Report {
            now_ms,
            file_version: self.version,
            expected_version: FILE_VERSION,
            default: self.default,
            rules: self.rules.clone(),
            max_rules: MAX_RULES,
            max_name_chars: MAX_NAME_CHARS,
            servers: rows,
            attached,
            denied,
            asked,
        }
    }
}

/// Attach one server's tools to `agent`, applying the policy.
///
/// `names` is what the server advertised, `make` builds the tool at that index
/// (the caller owns the transport and the approval broker), and `gated` says
/// whether the tool must wait for approval. A denied tool is **never built**, so
/// there is no wrapper to forget and no name for the model to call. The factory
/// is injected so the decision can be tested against a real [`Agent`] without a
/// network or an MCP server.
pub(crate) fn attach_tools_with_policy<F>(
    agent: &mut Agent,
    server: &str,
    names: &[String],
    policy: &Store,
    workspace_gate: bool,
    mut make: F,
) -> Result<Attachment, String>
where
    F: FnMut(usize, bool) -> Arc<dyn AsyncTool>,
{
    let mut attachment = Attachment::default();
    for (index, name) in names.iter().enumerate() {
        let decision = policy.decide(server, name);
        if decision.is_denied() {
            attachment.denied.push(Denied {
                tool: name.clone(),
                decision,
            });
            continue;
        }
        let gated = gate_for(workspace_gate, &decision);
        let tool = make(index, gated);
        agent
            .add_mcp_tool(server, name, tool)
            .map_err(|e| e.to_string())?;
        attachment.attached.push(name.clone());
    }
    Ok(attachment)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp path per test name, removed first so a run cannot read history.
    fn store(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("bos-mcp-policy-{name}.json"));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn policy(rules: &[(&str, &str, Effect)], default: Effect) -> Store {
        let path = store("in-memory");
        let mut store = Store::load(&path);
        store.set_default(default);
        for (server, tool, effect) in rules {
            store.set(server, tool, *effect).expect("valid rule");
        }
        store
    }

    fn echo(name: &str) -> Arc<dyn AsyncTool> {
        Arc::new(agent::tools::AsyncFunctionTool::new(
            name,
            "echo",
            serde_json::json!({"type": "object"}),
            |input: &serde_json::Value| {
                let value = input.clone();
                Box::pin(async move { Ok(value) })
            },
        ))
    }

    #[test]
    fn an_unruled_pair_takes_the_default() {
        let store = policy(&[], Effect::Allow);
        let decision = store.decide("srv", "read");
        assert_eq!(decision.effect, Effect::Allow);
        assert_eq!(decision.by, "default");
        let mut store = policy(&[], Effect::Deny);
        store.set_default(Effect::Deny);
        assert!(store.decide("srv", "anything").is_denied());
        assert_eq!(store.default, Effect::Deny);
    }

    #[test]
    fn a_rule_outranks_the_default() {
        let store = policy(&[("srv", "read", Effect::Deny)], Effect::Allow);
        let decision = store.decide("srv", "read");
        assert!(decision.is_denied());
        assert_eq!(decision.by, "srv/read");
        assert_eq!(store.decide("srv", "write").effect, Effect::Allow);
        assert_eq!(store.decide("other", "read").effect, Effect::Allow);
    }

    #[test]
    fn either_half_of_a_rule_may_be_a_wildcard() {
        let store = policy(
            &[("srv", "*", Effect::Ask), ("*", "read", Effect::Allow)],
            Effect::Allow,
        );
        // `srv/*` asks: the server half is what matched.
        let decision = store.decide("srv", "write");
        assert!(decision.asks(), "{decision:?}");
        assert_eq!(decision.by, "srv/*");
        // `*/read` allows a tool from a server with no rule of its own.
        assert_eq!(store.decide("other", "read").effect, Effect::Allow);
        assert_eq!(store.decide("other", "read").by, "*/read");
        // Neither wildcard covers this one, so the default answers.
        assert_eq!(store.decide("other", "write").by, "default");
        // And a catch-all really does catch everything: `*/*` is measured
        // against `srv/*`, and deny is the stricter of the two.
        let store = policy(
            &[("srv", "*", Effect::Ask), ("*", "*", Effect::Deny)],
            Effect::Allow,
        );
        assert!(store.decide("srv", "read").is_denied());
        assert_eq!(store.decide("srv", "read").by, "*/*");
    }

    #[test]
    fn deny_beats_ask_beats_allow_for_one_pair() {
        let store = policy(
            &[
                ("srv", "read", Effect::Allow),
                ("srv", "*", Effect::Ask),
                ("*", "read", Effect::Deny),
            ],
            Effect::Allow,
        );
        let decision = store.decide("srv", "read");
        assert!(decision.is_denied(), "{decision:?}");
        // The strictest rule wins regardless of the order they were added in.
        let store = policy(
            &[
                ("*", "read", Effect::Deny),
                ("srv", "*", Effect::Ask),
                ("srv", "read", Effect::Allow),
            ],
            Effect::Allow,
        );
        assert!(store.decide("srv", "read").is_denied());
    }

    #[test]
    fn the_reason_names_the_first_rule_at_the_winning_strength() {
        let store = policy(
            &[("srv", "read", Effect::Deny), ("*", "read", Effect::Deny)],
            Effect::Allow,
        );
        assert_eq!(store.decide("srv", "read").by, "srv/read");
    }

    #[test]
    fn setting_a_pair_again_replaces_its_rule() {
        let mut store = policy(&[("srv", "read", Effect::Allow)], Effect::Allow);
        store.set("srv", "read", Effect::Deny).expect("update");
        assert_eq!(store.rules.len(), 1, "one pair is one rule");
        assert!(store.decide("srv", "read").is_denied());
        store.set("srv", "read", Effect::Ask).expect("update again");
        assert_eq!(store.rules.len(), 1);
        assert!(store.decide("srv", "read").asks());
    }

    #[test]
    fn a_rule_needs_both_halves_and_they_are_bounded() {
        let mut store = policy(&[], Effect::Allow);
        assert!(store.set("", "read", Effect::Deny).is_err());
        assert!(store.set("srv", "   ", Effect::Deny).is_err());
        assert!(store
            .set("srv", &"t".repeat(MAX_NAME_CHARS + 1), Effect::Deny)
            .is_err());
        assert!(store
            .set(&"s".repeat(MAX_NAME_CHARS), "read", Effect::Deny)
            .is_ok());
        // Setting a pair trims it, so `" srv "` is the same rule as `"srv"`.
        let mut store = policy(&[("srv", "read", Effect::Allow)], Effect::Allow);
        store.set(" srv ", " read ", Effect::Deny).expect("trimmed");
        assert_eq!(store.rules.len(), 1);
        assert!(store.decide("srv", "read").is_denied());
    }

    #[test]
    fn the_store_refuses_a_rule_past_its_cap() {
        let path = store("cap");
        let mut store = Store::load(&path);
        for index in 0..MAX_RULES {
            store
                .set("srv", &format!("tool-{index}"), Effect::Ask)
                .expect("under the cap");
        }
        assert_eq!(store.rules.len(), MAX_RULES);
        let err = store
            .set("srv", "one-more", Effect::Deny)
            .expect_err("refused");
        assert!(err.contains(&MAX_RULES.to_string()), "unexpected: {err}");
        // An existing pair is still editable at the cap: it adds nothing.
        store
            .set("srv", "tool-0", Effect::Deny)
            .expect("still editable");
    }

    #[test]
    fn clearing_a_pair_says_whether_there_was_one() {
        let mut store = policy(&[("srv", "read", Effect::Deny)], Effect::Allow);
        assert!(store.clear("srv", "read"));
        assert!(!store.clear("srv", "read"), "the second time there is none");
        assert!(store.decide("srv", "read").effect == Effect::Allow);
    }

    #[test]
    fn a_stored_policy_survives_a_restart() {
        let path = store("restart");
        let mut written = Store::load(&path);
        written.set_default(Effect::Ask);
        written.set("srv", "*", Effect::Deny).expect("rule");
        written.set("*", "read", Effect::Allow).expect("rule");
        written.save().expect("saved");

        let read = Store::load(&path);
        assert_eq!(read.default, Effect::Ask);
        assert_eq!(read.rules.len(), 2);
        assert!(read.decide("srv", "anything").is_denied());
        assert_eq!(read.decide("other", "read").effect, Effect::Allow);
        assert_eq!(read.decide("other", "read").by, "*/read");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_or_corrupt_file_is_the_default_policy() {
        let path = store("missing");
        let store = Store::load(&path);
        assert_eq!(store.rules.len(), 0);
        assert_eq!(store.default, Effect::Allow);

        std::fs::write(&path, "{not json").unwrap();
        let store = Store::load(&path);
        assert_eq!(store.rules.len(), 0);
        assert_eq!(store.default, Effect::Allow);

        // A valid document with a nonsense default falls back rather than
        // refusing to open: an unreadable policy must not become "deny nothing"
        // by accident, and it must not brick the pane either.
        std::fs::write(&path, r#"{"version":1,"default":"maybe","rules":[]}"#).unwrap();
        assert_eq!(Store::load(&path).default, Effect::Allow);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_drops_rules_that_no_longer_validate() {
        let path = store("hand-edited");
        std::fs::write(
            &path,
            r#"{"version":1,"default":"allow","rules":[
                {"server":"srv","tool":"read","effect":"deny"},
                {"server":"","tool":"read","effect":"deny"},
                {"server":"srv","tool":"read","effect":"perhaps"},
                {"server":"srv"},
                {"tool":"read","effect":"deny"},
                {"server":"srv","tool":"write","effect":"ask"}
            ]}"#,
        )
        .unwrap();
        let store = Store::load(&path);
        assert_eq!(store.rules.len(), 2, "the four invalid rules are gone");
        assert!(store.decide("srv", "read").is_denied());
        assert!(store.decide("srv", "write").asks());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_keeps_the_strictest_of_two_rules_for_one_pair() {
        let path = store("duplicates");
        std::fs::write(
            &path,
            r#"{"version":1,"default":"allow","rules":[
                {"server":"srv","tool":"read","effect":"allow"},
                {"server":"srv","tool":"read","effect":"deny"}
            ]}"#,
        )
        .unwrap();
        let store = Store::load(&path);
        assert_eq!(store.rules.len(), 1);
        assert!(store.decide("srv", "read").is_denied());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_report_counts_what_the_policy_does_to_the_connected_tools() {
        let store = policy(
            &[("srv", "write", Effect::Deny), ("srv", "read", Effect::Ask)],
            Effect::Allow,
        );
        let servers = vec![(
            "srv".to_string(),
            vec!["read".to_string(), "write".to_string(), "list".to_string()],
        )];
        let report = store.report(&servers, 42);
        assert_eq!(report.now_ms, 42);
        assert_eq!(report.servers.len(), 1);
        let server = &report.servers[0];
        assert_eq!(server.attached, 2);
        assert_eq!(server.denied, 1);
        assert_eq!(report.attached, 2);
        assert_eq!(report.denied, 1);
        assert_eq!(report.asked, 1);
        assert_eq!(server.tools[0].name, "read");
        assert_eq!(server.tools[0].effect, Effect::Ask);
        assert_eq!(server.tools[0].by, "srv/read");
        assert_eq!(server.tools[1].effect, Effect::Deny);
        assert_eq!(server.tools[2].effect, Effect::Allow);
        assert_eq!(server.tools[2].by, "default");
        assert_eq!(report.max_rules, MAX_RULES);
        assert_eq!(report.file_version, FILE_VERSION);
        assert_eq!(report.expected_version, FILE_VERSION);
        assert_eq!(report.rules.len(), 2);
        // A server that is down has no tools to list, but the policy is intact.
        let report = store.report(&[], 1);
        assert!(report.servers.is_empty());
        assert_eq!(report.rules.len(), 2);
    }

    #[test]
    fn the_workspace_gate_is_a_floor_and_ask_adds_one() {
        let allow = Decision {
            effect: Effect::Allow,
            by: "default".to_string(),
        };
        let ask = Decision {
            effect: Effect::Ask,
            by: "srv/read".to_string(),
        };
        assert!(!gate_for(false, &allow), "nothing asks for a gate");
        assert!(gate_for(true, &allow), "the workspace gate still applies");
        assert!(gate_for(false, &ask), "an ask rule gates on its own");
        assert!(gate_for(true, &ask));
    }

    #[test]
    fn a_denied_tool_never_reaches_the_agent_and_the_factory_is_not_called() {
        let store = policy(&[("srv", "write", Effect::Deny)], Effect::Allow);
        let mut agent = Agent::from_config(agent::AgentConfig::default());
        let names = vec!["read".to_string(), "write".to_string()];
        let mut built: Vec<(usize, bool)> = Vec::new();
        let attachment =
            attach_tools_with_policy(&mut agent, "srv", &names, &store, false, |index, gated| {
                built.push((index, gated));
                echo(&format!("srv_{}", names[index]))
            })
            .expect("attached");
        assert_eq!(attachment.attached, vec!["read".to_string()]);
        assert_eq!(attachment.denied.len(), 1);
        assert_eq!(attachment.denied[0].tool, "write");
        assert_eq!(attachment.denied[0].decision.by, "srv/write");
        assert_eq!(built, vec![(0, false)], "the denied tool is never built");
        let registered = agent.registry().expect("registry").async_tool_names();
        assert!(registered.contains(&"srv_read".to_string()));
        assert!(
            !registered.contains(&"srv_write".to_string()),
            "a denied tool must not exist: {registered:?}"
        );
    }

    #[test]
    fn an_ask_rule_gates_a_tool_even_with_no_workspace_gate() {
        let store = policy(&[("srv", "read", Effect::Ask)], Effect::Allow);
        let mut agent = Agent::from_config(agent::AgentConfig::default());
        let names = vec!["read".to_string(), "list".to_string()];
        let mut gates = Vec::new();
        attach_tools_with_policy(&mut agent, "srv", &names, &store, false, |index, gated| {
            gates.push((names[index].clone(), gated));
            echo(&format!("srv_{}", names[index]))
        })
        .expect("attached");
        assert_eq!(
            gates,
            vec![("read".to_string(), true), ("list".to_string(), false)],
            "only the asked tool takes a gate"
        );
    }

    #[test]
    fn a_catch_all_deny_leaves_the_agent_without_the_server_at_all() {
        let store = policy(&[("*", "*", Effect::Deny)], Effect::Allow);
        let mut agent = Agent::from_config(agent::AgentConfig::default());
        let names = vec!["a".to_string(), "b".to_string()];
        let attachment =
            attach_tools_with_policy(&mut agent, "srv", &names, &store, false, |index, _| {
                echo(&format!("srv_{}", names[index]))
            })
            .expect("nothing to attach is not an error");
        assert!(attachment.attached.is_empty());
        assert_eq!(attachment.denied.len(), 2);
        assert!(agent
            .registry()
            .expect("registry")
            .async_tool_names()
            .is_empty());
    }

    #[test]
    fn the_effects_and_decisions_serialise_the_way_the_frontend_reads_them() {
        let json = serde_json::to_value(Effect::Ask).expect("wire");
        assert_eq!(json, serde_json::json!("ask"));
        assert_eq!(Effect::parse(" DENY "), Some(Effect::Deny));
        assert_eq!(Effect::parse("maybe"), None);
        let decision = Decision {
            effect: Effect::Deny,
            by: "srv/read".to_string(),
        };
        let json = serde_json::to_value(&decision).expect("wire");
        assert_eq!(json["effect"], serde_json::json!("deny"));
        assert_eq!(json["by"], serde_json::json!("srv/read"));
    }

    #[test]
    fn a_saved_policy_names_its_version() {
        let path = store("versioned");
        let mut store = Store::load(&path);
        store.set("srv", "read", Effect::Deny).expect("rule");
        store.save().expect("saved");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(
            text.contains(&format!("\"version\": {FILE_VERSION}")),
            "{text}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A decision is a value, not a view: mutating the store afterwards must not
    /// change what a caller already holds.
    #[test]
    fn a_decision_does_not_change_when_the_policy_does() {
        let mut store = policy(&[("srv", "read", Effect::Deny)], Effect::Allow);
        let before = store.decide("srv", "read");
        store.set("srv", "read", Effect::Allow).expect("changed");
        assert!(before.is_denied(), "the holder keeps its answer");
        assert!(!store.decide("srv", "read").is_denied());
    }
}
