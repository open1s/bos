//! Scoped "allow-always" approval rules for tool invocations.
//!
//! A [`RuleSet`](crate::security::rules::RuleSet) holds a list of [`ToolRule`](crate::security::rules::ToolRule)s. Each rule names a tool (exact
//! name or a `*` glob), may narrow the match with an argument predicate, is
//! scoped to a [`RuleScope`](crate::security::rules::RuleScope), and carries an [`RuleEffect`](crate::security::rules::RuleEffect). [`RuleSet::decide`](crate::security::rules::RuleSet::decide)
//! evaluates a tool call against the rules and returns a [`Decision`](crate::security::rules::Decision).
//!
//! Precedence is explicit and total:
//!
//! 1. An explicit [`RuleEffect::Deny`](crate::security::rules::RuleEffect::Deny) matches beat any [`RuleEffect::Allow`](crate::security::rules::RuleEffect::Allow).
//! 2. Among rules with the same effect, the most specific rule wins. Specificity
//!    is evaluated lexicographically: narrower scope first, then "has an
//!    argument predicate".
//! 3. When nothing matches, the answer is [`DecisionEffect::Ask`](crate::security::rules::DecisionEffect::Ask).
//!
//! The rule set also keeps a capacity-bounded, oldest-first audit ring that
//! records every decision, and can be saved to / loaded from a JSON file. A
//! malformed file never panics: unparseable entries are skipped and reported as
//! warnings while the remaining entries are kept.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// The scope a [`ToolRule`] applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum RuleScope {
    /// Applies to every workspace and session.
    Global,
    /// Applies only to the named workspace.
    Workspace(String),
    /// Applies only to the named session.
    Session(String),
}

impl RuleScope {
    fn rank(&self) -> u8 {
        match self {
            RuleScope::Global => 0,
            RuleScope::Workspace(_) => 1,
            RuleScope::Session(_) => 2,
        }
    }

    /// Whether a rule carrying `self` applies to a call made in `requested`.
    ///
    /// A [`RuleScope::Global`] rule applies everywhere. A workspace or session
    /// rule applies only when the requested scope is the same and names the same
    /// workspace or session.
    pub fn matches(&self, requested: &RuleScope) -> bool {
        match self {
            RuleScope::Global => true,
            RuleScope::Workspace(w) => {
                matches!(requested, RuleScope::Workspace(other) if other == w)
            }
            RuleScope::Session(s) => matches!(requested, RuleScope::Session(other) if other == s),
        }
    }
}

/// What a matching [`ToolRule`] does to a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
    /// Allow the call without asking the user.
    Allow,
    /// Deny the call outright.
    Deny,
}

/// The outcome of evaluating a tool call against a [`RuleSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionEffect {
    /// A matching allow rule permits the call.
    Allow,
    /// A matching deny rule forbids the call.
    Deny,
    /// Nothing matched, so the host should prompt the user.
    Ask,
}

impl DecisionEffect {
    /// Whether this decision permits the call.
    pub fn is_allowed(&self) -> bool {
        matches!(self, DecisionEffect::Allow)
    }

    /// Whether this decision forbids the call.
    pub fn is_denied(&self) -> bool {
        matches!(self, DecisionEffect::Deny)
    }

    /// Whether this decision leaves the call to the user.
    pub fn is_ask(&self) -> bool {
        matches!(self, DecisionEffect::Ask)
    }
}

/// How an argument predicate compares the value of one argument key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgMatch {
    /// The argument value must equal the predicate value.
    Exact(String),
    /// The argument value must start with the predicate value.
    Prefix(String),
    /// The argument value must contain the predicate value.
    Contains(String),
}

impl ArgMatch {
    fn matches(&self, value: &str) -> bool {
        match self {
            ArgMatch::Exact(want) => value == want,
            ArgMatch::Prefix(want) => value.starts_with(want.as_str()),
            ArgMatch::Contains(want) => value.contains(want.as_str()),
        }
    }
}

/// A predicate over one JSON argument key.
///
/// Predicates apply to string-valued arguments only: an argument that is
/// missing, or whose value is not a JSON string, never matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgPredicate {
    /// The argument key to inspect.
    pub key: String,
    /// The comparison applied to the argument value.
    pub predicate: ArgMatch,
}

impl ArgPredicate {
    /// Require `key` to equal `value`.
    pub fn exact(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            predicate: ArgMatch::Exact(value.into()),
        }
    }

    /// Require `key` to start with `value`.
    pub fn prefix(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            predicate: ArgMatch::Prefix(value.into()),
        }
    }

    /// Require `key` to contain `value`.
    pub fn contains(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            predicate: ArgMatch::Contains(value.into()),
        }
    }

    /// Whether `args` matches this predicate.
    pub fn matches(&self, args: &serde_json::Value) -> bool {
        let Some(value) = args.get(&self.key) else {
            return false;
        };
        let Some(text) = value.as_str() else {
            return false;
        };
        self.predicate.matches(text)
    }
}

/// One scoped approval rule for a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRule {
    /// Stable rule id; empty when the rule has not been added to a [`RuleSet`] yet.
    #[serde(default)]
    pub id: String,
    /// Tool name to match, either an exact name or a `*` glob.
    pub tool: String,
    /// Optional argument predicate that further narrows the match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<ArgPredicate>,
    /// The scope the rule applies to.
    pub scope: RuleScope,
    /// What happens when the rule matches.
    pub effect: RuleEffect,
}

impl ToolRule {
    /// Create an allow rule for `tool`, scoped globally.
    pub fn allow(tool: impl Into<String>) -> Self {
        Self::new(tool, RuleEffect::Allow)
    }

    /// Create a deny rule for `tool`, scoped globally.
    pub fn deny(tool: impl Into<String>) -> Self {
        Self::new(tool, RuleEffect::Deny)
    }

    /// Create a rule with the given tool pattern and effect.
    pub fn new(tool: impl Into<String>, effect: RuleEffect) -> Self {
        Self {
            id: String::new(),
            tool: tool.into(),
            arg: None,
            scope: RuleScope::Global,
            effect,
        }
    }

    /// Restrict the rule to `scope`.
    pub fn with_scope(mut self, scope: RuleScope) -> Self {
        self.scope = scope;
        self
    }

    /// Narrow the rule with an argument predicate.
    pub fn with_arg(mut self, arg: ArgPredicate) -> Self {
        self.arg = Some(arg);
        self
    }

    /// Whether this rule's tool pattern matches `tool`.
    pub fn tool_matches(&self, tool: &str) -> bool {
        glob_match(&self.tool, tool)
    }

    /// Whether this rule applies to a call of `tool` with `args` in `scope`.
    pub fn matches(&self, tool: &str, args: &serde_json::Value, scope: &RuleScope) -> bool {
        self.tool_matches(tool)
            && self.scope.matches(scope)
            && self
                .arg
                .as_ref()
                .map(|predicate| predicate.matches(args))
                .unwrap_or(true)
    }

    fn specificity(&self) -> (u8, u8) {
        (self.scope.rank(), u8::from(self.arg.is_some()))
    }
}

/// Glob match supporting `*` as "any run of characters" (including empty).
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == text;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    let Some((first, rest_parts)) = parts.split_first() else {
        return false;
    };
    let Some((last, middle)) = rest_parts.split_last() else {
        return text.starts_with(first);
    };

    let mut rest = match text.strip_prefix(first) {
        Some(rest) => rest,
        None => return false,
    };
    for part in middle {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(idx) => rest = &rest[idx + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The result of evaluating a tool call against a [`RuleSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// What the caller should do.
    pub effect: DecisionEffect,
    /// Id of the rule that produced the decision, when one matched.
    pub matched_rule: Option<String>,
    /// Human-readable explanation of the decision.
    pub reason: String,
}

/// One recorded decision in the audit ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// Tool that was evaluated.
    pub tool: String,
    /// Stable digest of the argument JSON.
    pub args_digest: String,
    /// Decision that was reached.
    pub effect: DecisionEffect,
    /// Id of the rule that decided it, if any.
    pub rule: Option<String>,
    /// Wall-clock time of the decision, in Unix epoch milliseconds.
    pub at_ms: u64,
}

/// Default size of the bounded audit ring.
pub const DEFAULT_AUDIT_CAPACITY: usize = 128;

/// A set of scoped approval rules plus a bounded decision audit ring.
pub struct RuleSet {
    rules: Vec<ToolRule>,
    next_id: u64,
    audit: Mutex<VecDeque<AuditEntry>>,
    audit_capacity: usize,
}

impl Default for RuleSet {
    fn default() -> Self {
        Self::new()
    }
}

impl RuleSet {
    /// Create an empty rule set with the default audit capacity.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            next_id: 0,
            audit: Mutex::new(VecDeque::new()),
            audit_capacity: DEFAULT_AUDIT_CAPACITY,
        }
    }

    /// Cap the audit ring at `capacity` entries (0 disables recording).
    pub fn with_audit_capacity(mut self, capacity: usize) -> Self {
        self.audit_capacity = capacity;
        if let Ok(mut ring) = self.audit.lock() {
            while ring.len() > capacity {
                ring.pop_front();
            }
        }
        self
    }

    /// Add `rule`, assigning an id when it does not already carry one.
    ///
    /// Returns the id the rule is stored under.
    pub fn add(&mut self, mut rule: ToolRule) -> String {
        if rule.id.trim().is_empty() {
            self.next_id += 1;
            rule.id = format!("rule-{}", self.next_id);
        }
        let id = rule.id.clone();
        self.rules.push(rule);
        id
    }

    /// The stored rules, in insertion order.
    pub fn list(&self) -> &[ToolRule] {
        &self.rules
    }

    /// Remove every rule with `id`; returns whether anything was removed.
    pub fn revoke(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|rule| rule.id != id);
        self.rules.len() != before
    }

    /// Evaluate a tool call and record it in the audit ring.
    ///
    /// See the module docs for the precedence rules.
    pub fn decide(&self, tool: &str, args: &serde_json::Value, scope: &RuleScope) -> Decision {
        let matching: Vec<&ToolRule> = self
            .rules
            .iter()
            .filter(|rule| rule.matches(tool, args, scope))
            .collect();

        let best = |effect: RuleEffect| {
            matching
                .iter()
                .filter(|rule| rule.effect == effect)
                .max_by_key(|rule| rule.specificity())
                .copied()
        };

        let (effect, matched) = if let Some(rule) = best(RuleEffect::Deny) {
            (DecisionEffect::Deny, Some(rule))
        } else if let Some(rule) = best(RuleEffect::Allow) {
            (DecisionEffect::Allow, Some(rule))
        } else {
            (DecisionEffect::Ask, None)
        };

        let reason = match matched {
            Some(rule) => format!(
                "{}: matched rule '{}' ({} {:?})",
                tool, rule.id, rule.tool, rule.scope
            ),
            None => format!("{}: no matching rule", tool),
        };

        let entry = AuditEntry {
            tool: tool.to_string(),
            args_digest: digest_args(args),
            effect,
            rule: matched.map(|rule| rule.id.clone()),
            at_ms: now_ms(),
        };
        self.record(entry);

        Decision {
            effect,
            matched_rule: matched.map(|rule| rule.id.clone()),
            reason,
        }
    }

    fn record(&self, entry: AuditEntry) {
        if self.audit_capacity == 0 {
            return;
        }
        if let Ok(mut ring) = self.audit.lock() {
            while ring.len() >= self.audit_capacity {
                ring.pop_front();
            }
            ring.push_back(entry);
        }
    }

    /// Snapshot of the audit ring, oldest entry first.
    pub fn audit(&self) -> Vec<AuditEntry> {
        self.audit
            .lock()
            .map(|ring| ring.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Current audit ring capacity.
    pub fn audit_capacity(&self) -> usize {
        self.audit_capacity
    }

    /// Number of rules in the set.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the set has no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Serialize the rules (not the audit ring) as a pretty JSON array.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.rules)
    }

    /// Write the rules to `path` as JSON.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = self
            .to_json_string()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        fs::write(path, json)
    }

    /// Read rules from the JSON file at `path`.
    ///
    /// Returns the parsed set plus one warning per entry that could not be
    /// parsed. Only I/O failures are returned as errors; a syntactically broken
    /// file yields an empty set and a warning.
    pub fn load(path: &Path) -> std::io::Result<(RuleSet, Vec<String>)> {
        let text = fs::read_to_string(path)?;
        Ok(Self::from_json_str(&text))
    }

    /// Parse rules from a JSON string, tolerating individual bad entries.
    ///
    /// Accepts either a top-level array of rules or an object with a `rules`
    /// array. Each entry is parsed independently; failures are skipped and
    /// described in the returned warnings.
    pub fn from_json_str(text: &str) -> (RuleSet, Vec<String>) {
        let mut set = RuleSet::new();
        let mut warnings = Vec::new();

        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(format!("top-level JSON parse error: {err}"));
                return (set, warnings);
            }
        };

        let entries = match value {
            serde_json::Value::Array(entries) => entries,
            serde_json::Value::Object(mut map) => match map.remove("rules") {
                Some(serde_json::Value::Array(entries)) => entries,
                Some(_) => {
                    warnings.push("'rules' is not an array".to_string());
                    return (set, warnings);
                }
                None => {
                    warnings.push("object has no 'rules' array".to_string());
                    return (set, warnings);
                }
            },
            _ => {
                warnings.push("expected a JSON array of rules".to_string());
                return (set, warnings);
            }
        };

        for (index, entry) in entries.into_iter().enumerate() {
            match serde_json::from_value::<ToolRule>(entry) {
                Ok(rule) => {
                    set.add(rule);
                }
                Err(err) => warnings.push(format!("rules[{index}]: {err}")),
            }
        }

        (set, warnings)
    }
}

/// Stable digest of a JSON argument value.
pub fn digest_args(args: &serde_json::Value) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    args.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn global() -> RuleScope {
        RuleScope::Global
    }

    #[test]
    fn exact_tool_match_allows() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("read_file"));

        let decision = set.decide("read_file", &json!({}), &global());
        assert_eq!(decision.effect, DecisionEffect::Allow);
        assert_eq!(decision.matched_rule.as_deref(), Some("rule-1"));
    }

    #[test]
    fn glob_tool_match_allows() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("fs_*"));

        assert!(set
            .decide("fs_read", &json!({}), &global())
            .effect
            .is_allowed());
        assert!(set
            .decide("fs_write", &json!({}), &global())
            .effect
            .is_allowed());
        assert!(set.decide("net_get", &json!({}), &global()).effect.is_ask());
        // A bare `*` matches everything.
        let mut any = RuleSet::new();
        any.add(ToolRule::allow("*"));
        assert!(any
            .decide("anything", &json!({}), &global())
            .effect
            .is_allowed());
    }

    #[test]
    fn arg_predicate_exact() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("bash").with_arg(ArgPredicate::exact("command", "ls")));

        assert!(set
            .decide("bash", &json!({"command": "ls"}), &global())
            .effect
            .is_allowed());
        assert!(set
            .decide("bash", &json!({"command": "ls -la"}), &global())
            .effect
            .is_ask());
    }

    #[test]
    fn arg_predicate_prefix() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("bash").with_arg(ArgPredicate::prefix("command", "git status")));

        assert!(set
            .decide("bash", &json!({"command": "git status --short"}), &global())
            .effect
            .is_allowed());
        assert!(set
            .decide("bash", &json!({"command": "git push"}), &global())
            .effect
            .is_ask());
    }

    #[test]
    fn arg_predicate_contains() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("bash").with_arg(ArgPredicate::contains("command", "test")));

        assert!(set
            .decide("bash", &json!({"command": "cargo test"}), &global())
            .effect
            .is_allowed());
        assert!(set
            .decide("bash", &json!({"command": "cargo build"}), &global())
            .effect
            .is_ask());
    }

    #[test]
    fn arg_predicate_ignores_missing_and_non_string_values() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("bash").with_arg(ArgPredicate::exact("command", "ls")));

        assert!(set.decide("bash", &json!({}), &global()).effect.is_ask());
        assert!(set
            .decide("bash", &json!({"command": 42}), &global())
            .effect
            .is_ask());
    }

    #[test]
    fn deny_beats_allow() {
        let mut set = RuleSet::new();
        // The specific allow is added first; deny still wins.
        set.add(ToolRule::allow("bash").with_arg(ArgPredicate::exact("command", "rm -rf /tmp/x")));
        set.add(ToolRule::deny("bash"));

        let decision = set.decide("bash", &json!({"command": "rm -rf /tmp/x"}), &global());
        assert_eq!(decision.effect, DecisionEffect::Deny);
        assert_eq!(decision.matched_rule.as_deref(), Some("rule-2"));
    }

    #[test]
    fn narrower_scope_beats_broader_for_the_same_effect() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("read_file"));
        set.add(ToolRule::allow("read_file").with_scope(RuleScope::Session("s".into())));

        // Both match in the session; the narrower session rule is reported.
        let in_session = set.decide("read_file", &json!({}), &RuleScope::Session("s".into()));
        assert_eq!(in_session.effect, DecisionEffect::Allow);
        assert_eq!(in_session.matched_rule.as_deref(), Some("rule-2"));

        // Globally only the broad rule matches.
        let elsewhere = set.decide("read_file", &json!({}), &global());
        assert_eq!(elsewhere.matched_rule.as_deref(), Some("rule-1"));
    }

    #[test]
    fn argument_predicate_beats_plain_rule_at_same_scope() {
        let mut set = RuleSet::new();
        // A broad deny and a predicate deny: the predicate rule is more specific.
        set.add(ToolRule::deny("bash"));
        set.add(ToolRule::deny("bash").with_arg(ArgPredicate::prefix("command", "rm ")));

        let decision = set.decide("bash", &json!({"command": "rm -rf /"}), &global());
        assert_eq!(decision.effect, DecisionEffect::Deny);
        assert_eq!(decision.matched_rule.as_deref(), Some("rule-2"));
    }

    #[test]
    fn workspace_rule_does_not_apply_to_a_session_scope() {
        let mut set = RuleSet::new();
        set.add(ToolRule::deny("write").with_scope(RuleScope::Workspace("w".into())));

        // A session elsewhere is untouched by the workspace deny.
        let in_session = set.decide("write", &json!({}), &RuleScope::Session("s".into()));
        assert_eq!(in_session.effect, DecisionEffect::Ask);

        let in_workspace = set.decide("write", &json!({}), &RuleScope::Workspace("w".into()));
        assert_eq!(in_workspace.effect, DecisionEffect::Deny);
    }

    #[test]
    fn unknown_tool_asks() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("read_file"));

        let decision = set.decide("write_file", &json!({}), &global());
        assert_eq!(decision.effect, DecisionEffect::Ask);
        assert!(decision.matched_rule.is_none());
        assert!(decision.reason.contains("no matching rule"));
    }

    #[test]
    fn revoke_removes_the_rule() {
        let mut set = RuleSet::new();
        let id = set.add(ToolRule::allow("read_file"));
        assert!(set
            .decide("read_file", &json!({}), &global())
            .effect
            .is_allowed());

        assert!(set.revoke(&id));
        assert!(set
            .decide("read_file", &json!({}), &global())
            .effect
            .is_ask());
        assert!(!set.revoke(&id));
        assert!(set.is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("bos-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rules.json");

        let mut set = RuleSet::new();
        set.add(ToolRule::allow("read_file"));
        set.add(
            ToolRule::deny("bash")
                .with_arg(ArgPredicate::prefix("command", "rm "))
                .with_scope(RuleScope::Workspace("proj".into())),
        );
        set.save(&path).unwrap();

        let (loaded, warnings) = RuleSet::load(&path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.list().len(), 2);
        assert!(loaded
            .decide("read_file", &json!({}), &global())
            .effect
            .is_allowed());
        assert!(loaded
            .decide(
                "bash",
                &json!({"command": "rm -rf /"}),
                &RuleScope::Workspace("proj".into())
            )
            .effect
            .is_denied());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn malformed_entries_are_skipped_with_warnings() {
        let text = r#"[
            {"tool": "read_file", "scope": {"kind": "Global"}, "effect": "allow"},
            {"tool": 5, "scope": {"kind": "Global"}, "effect": "allow"},
            {"tool": "bash", "scope": {"kind": "Global"}, "effect": "explode"},
            "not an object",
            {"tool": "write_file", "scope": {"kind": "Global"}, "effect": "deny"}
        ]"#;

        let (set, warnings) = RuleSet::from_json_str(text);
        assert_eq!(set.list().len(), 2, "kept the two valid rules");
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.contains("rules[")));
        assert!(set
            .decide("read_file", &json!({}), &global())
            .effect
            .is_allowed());
        assert!(set
            .decide("write_file", &json!({}), &global())
            .effect
            .is_denied());
    }

    #[test]
    fn completely_broken_file_yields_empty_set_and_warning() {
        let (set, warnings) = RuleSet::from_json_str("this is not json");
        assert!(set.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("top-level JSON parse error"));
    }

    #[test]
    fn object_wrapper_with_rules_array_is_accepted() {
        let text = r#"{"rules": [{"tool": "*", "scope": {"kind": "Global"}, "effect": "allow"}]}"#;
        let (set, warnings) = RuleSet::from_json_str(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(set.list().len(), 1);
    }

    #[test]
    fn audit_ring_is_bounded_and_oldest_first() {
        let mut set = RuleSet::new().with_audit_capacity(3);
        set.add(ToolRule::allow("read_file"));

        for _ in 0..5 {
            set.decide("read_file", &json!({}), &global());
        }
        set.decide("write_file", &json!({}), &global());

        let audit = set.audit();
        assert_eq!(audit.len(), 3, "capacity is enforced");
        // Oldest survivors are the last three decisions, oldest first.
        assert_eq!(audit[0].tool, "read_file");
        assert_eq!(audit[1].tool, "read_file");
        assert_eq!(audit[2].tool, "write_file");
        assert_eq!(audit[2].effect, DecisionEffect::Ask);
        assert!(audit[2].rule.is_none());
        assert!(audit[0].at_ms > 0);
        assert!(!audit[0].args_digest.is_empty());
    }

    #[test]
    fn audit_digest_is_stable_for_equal_args() {
        let mut set = RuleSet::new();
        set.add(ToolRule::allow("read_file"));
        let args = json!({"path": "a.txt"});
        set.decide("read_file", &args, &global());
        set.decide("read_file", &args, &global());

        let audit = set.audit();
        assert_eq!(audit[0].args_digest, audit[1].args_digest);
        assert_ne!(digest_args(&args), digest_args(&json!({"path": "b.txt"})));
    }
}
