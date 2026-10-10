//! Governance policy for MCP server and tool invocation.
//!
//! An [`McpPolicy`](crate::mcp::policy::McpPolicy) holds allow/deny rules keyed by MCP server and tool. A rule
//! field may be an exact name or `*` to match anything. Matching is explicit and
//! total: a matching `Deny` always beats a matching `Allow`, and when nothing
//! matches the policy returns its default (an explicit choice, [`McpDecision::Ask`](crate::mcp::policy::McpDecision::Ask)
//! unless configured otherwise).

/// The effect of one [`McpRule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEffect {
    /// Permit the call.
    Allow,
    /// Forbid the call.
    Deny,
}

/// The decision reached for a server/tool pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpDecision {
    /// A matching allow rule permits the call.
    Allow,
    /// A matching deny rule forbids the call.
    Deny,
    /// Nothing matched, so the caller should ask the user.
    Ask,
}

impl McpDecision {
    /// Whether this decision permits the call.
    pub fn is_allowed(&self) -> bool {
        matches!(self, McpDecision::Allow)
    }

    /// Whether this decision forbids the call.
    pub fn is_denied(&self) -> bool {
        matches!(self, McpDecision::Deny)
    }

    /// Whether this decision leaves the call to the user.
    pub fn is_ask(&self) -> bool {
        matches!(self, McpDecision::Ask)
    }
}

/// One allow/deny rule for an MCP server and tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRule {
    /// Server name, or `*` to match any server.
    pub server: String,
    /// Tool name, or `*` to match any tool.
    pub tool: String,
    /// What the rule does when it matches.
    pub effect: McpEffect,
}

impl McpRule {
    /// Create an allow rule for `server`/`tool`.
    pub fn allow(server: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            tool: tool.into(),
            effect: McpEffect::Allow,
        }
    }

    /// Create a deny rule for `server`/`tool`.
    pub fn deny(server: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            tool: tool.into(),
            effect: McpEffect::Deny,
        }
    }

    /// Whether this rule matches the given server and tool.
    pub fn matches(&self, server: &str, tool: &str) -> bool {
        field_matches(&self.server, server) && field_matches(&self.tool, tool)
    }
}

fn field_matches(pattern: &str, value: &str) -> bool {
    pattern == "*" || pattern == value
}

/// A governance policy over MCP servers and tools.
#[derive(Debug, Clone)]
pub struct McpPolicy {
    rules: Vec<McpRule>,
    default: McpDecision,
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl McpPolicy {
    /// Create a policy with the [`McpDecision::Ask`] default and no rules.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            default: McpDecision::Ask,
        }
    }

    /// Create a policy with an explicit default decision.
    pub fn with_default(default: McpDecision) -> Self {
        Self {
            rules: Vec::new(),
            default,
        }
    }

    /// The default decision applied when no rule matches.
    pub fn default_decision(&self) -> McpDecision {
        self.default
    }

    /// Append an allow rule.
    pub fn allow(&mut self, server: impl Into<String>, tool: impl Into<String>) -> &mut Self {
        self.rules.push(McpRule::allow(server, tool));
        self
    }

    /// Append a deny rule.
    pub fn deny(&mut self, server: impl Into<String>, tool: impl Into<String>) -> &mut Self {
        self.rules.push(McpRule::deny(server, tool));
        self
    }

    /// Append a prebuilt rule.
    pub fn add_rule(&mut self, rule: McpRule) -> &mut Self {
        self.rules.push(rule);
        self
    }

    /// Decide whether `server`/`tool` is allowed.
    ///
    /// A matching deny beats a matching allow; otherwise a matching allow wins;
    /// otherwise the configured default is returned.
    pub fn decide(&self, server: &str, tool: &str) -> McpDecision {
        if self
            .rules
            .iter()
            .any(|rule| rule.effect == McpEffect::Deny && rule.matches(server, tool))
        {
            McpDecision::Deny
        } else if self
            .rules
            .iter()
            .any(|rule| rule.effect == McpEffect::Allow && rule.matches(server, tool))
        {
            McpDecision::Allow
        } else {
            self.default
        }
    }

    /// The configured rules, in insertion order.
    pub fn list(&self) -> &[McpRule] {
        &self.rules
    }

    /// Number of configured rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether no rule is configured.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_rule_permits_a_matching_call() {
        let mut policy = McpPolicy::new();
        policy.allow("files", "read");

        assert_eq!(policy.decide("files", "read"), McpDecision::Allow);
        assert!(policy.decide("files", "read").is_allowed());
    }

    #[test]
    fn deny_rule_forbids_a_matching_call() {
        let mut policy = McpPolicy::new();
        policy.deny("files", "delete");

        assert_eq!(policy.decide("files", "delete"), McpDecision::Deny);
        assert!(policy.decide("files", "delete").is_denied());
    }

    #[test]
    fn deny_beats_allow() {
        let mut policy = McpPolicy::new();
        policy.allow("files", "write");
        policy.deny("files", "write");

        assert_eq!(policy.decide("files", "write"), McpDecision::Deny);
    }

    #[test]
    fn deny_beats_a_wildcard_allow() {
        let mut policy = McpPolicy::new();
        policy.allow("*", "*");
        policy.deny("danger", "shell");

        assert_eq!(policy.decide("danger", "shell"), McpDecision::Deny);
        assert_eq!(policy.decide("safe", "echo"), McpDecision::Allow);
    }

    #[test]
    fn default_is_ask_when_nothing_matches() {
        let policy = McpPolicy::new();
        assert_eq!(policy.decide("unknown", "tool"), McpDecision::Ask);
        assert!(policy.decide("unknown", "tool").is_ask());
    }

    #[test]
    fn default_is_explicit_and_configurable() {
        let policy = McpPolicy::with_default(McpDecision::Deny);
        assert_eq!(policy.default_decision(), McpDecision::Deny);
        assert_eq!(policy.decide("unknown", "tool"), McpDecision::Deny);

        let allow_by_default = McpPolicy::with_default(McpDecision::Allow);
        assert_eq!(allow_by_default.decide("x", "y"), McpDecision::Allow);
    }

    #[test]
    fn wildcards_match_server_and_tool_independently() {
        let mut policy = McpPolicy::new();
        policy.allow("github", "*");
        policy.allow("*", "echo");

        assert_eq!(policy.decide("github", "anything"), McpDecision::Allow);
        assert_eq!(policy.decide("any", "echo"), McpDecision::Allow);
        assert_eq!(policy.decide("any", "other"), McpDecision::Ask);
    }

    #[test]
    fn list_returns_configured_rules() {
        let mut policy = McpPolicy::new();
        policy.allow("a", "b");
        policy.deny("c", "d");

        assert_eq!(policy.len(), 2);
        assert!(!policy.is_empty());
        assert_eq!(policy.list()[0], McpRule::allow("a", "b"));
        assert_eq!(policy.list()[1], McpRule::deny("c", "d"));
    }

    #[test]
    fn empty_policy_is_empty() {
        let policy = McpPolicy::default();
        assert!(policy.is_empty());
        assert_eq!(policy.len(), 0);
    }
}
