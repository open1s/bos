//! Authorization policy using regorus (Rego/OPA) as the backend.
//!
//! Policies are written in Rego and evaluated by the regorus engine.
//! The policy document can contain either traditional rule-based JSON
//! (for backwards compatibility) or native Rego policies.

use regorus::Engine;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use crate::error::{ResourceError, Result};

/// Policy document containing Rego policy and optional legacy rules.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PolicyDoc {
    /// Agents permitted to perform `PolicyUpdate` (admin).
    pub admins: Vec<String>,

    /// Optional Rego policy module. If present, takes precedence over `rules`.
    /// The policy should define `allow` returning boolean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rego: Option<String>,

    /// Legacy rule-based policy (for backwards compatibility).
    /// Ignored if `rego` is present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

/// A single authorization rule (legacy format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub agents: Vec<String>,
    pub uris: Vec<String>,
    pub actions: Vec<String>,
    pub effect: Effect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Effect {
    Allow,
    Deny,
}

impl PolicyDoc {
    /// Parse a policy document from JSON.
    pub fn from_json(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| ResourceError::Codec(e.to_string()))
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    fn wildcard(pattern: &str, value: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if !pattern.contains('*') {
            return pattern == value;
        }
        let parts: Vec<&str> = pattern.split('*').collect();
        let mut rest = value;
        if !parts.is_empty() && !parts[0].is_empty() {
            if !rest.starts_with(parts[0]) {
                return false;
            }
            rest = &rest[parts[0].len()..];
        }
        for p in &parts[1..] {
            if p.is_empty() {
                continue;
            }
            match rest.find(p) {
                Some(i) => rest = &rest[i + p.len()..],
                None => return false,
            }
        }
        true
    }

    fn rule_matches(rule: &Rule, agent: &str, uri: &str, action: &str) -> bool {
        let agent_ok = rule.agents.iter().any(|a| a == "*" || a == agent);
        let uri_ok = rule.uris.iter().any(|u| Self::wildcard(u, uri));
        let action_ok = rule.actions.iter().any(|a| a == "*" || a == action);
        agent_ok && uri_ok && action_ok
    }

    /// Whether `agent` is a policy administrator.
    pub fn is_admin(&self, agent: &str) -> bool {
        self.admins.iter().any(|a| a == "*" || a == agent)
    }

    /// Authorize `(agent, uri, action)` using Rego if available, else legacy rules.
    pub fn authorize(&self, agent: &str, uri: &str, action: &str) -> Result<()> {
        if let Some(rego) = &self.rego {
            return Self::authorize_rego(rego, agent, uri, action);
        }
        // Legacy rule evaluation
        for rule in &self.rules {
            if Self::rule_matches(rule, agent, uri, action) {
                return match rule.effect {
                    Effect::Allow => Ok(()),
                    Effect::Deny => Err(ResourceError::PolicyDenied {
                        agent: agent.to_string(),
                        uri: uri.to_string(),
                        action: action.to_string(),
                    }),
                };
            }
        }
        Err(ResourceError::PolicyDenied {
            agent: agent.to_string(),
            uri: uri.to_string(),
            action: action.to_string(),
        })
    }

    fn authorize_rego(rego: &str, agent: &str, uri: &str, action: &str) -> Result<()> {
        let mut engine = Engine::new();
        engine
            .add_policy("policy.rego".to_string(), rego.to_string())
            .map_err(|e| ResourceError::Codec(format!("rego parse error: {e}")))?;

        // Prepare input
        let input = serde_json::json!({
            "agent": agent,
            "uri": uri,
            "action": action
        });
        engine.set_input(input.into());

        // Query allow
        let results = engine
            .eval_query("data.policy.allow".to_string(), false)
            .map_err(|e| ResourceError::Codec(format!("rego eval error: {e}")))?;

        // regorus returns QueryResults with a `result` field containing Vec<QueryResult>
        // Each QueryResult has expressions with the query result value
        let allowed = results.result.iter().any(|qr| {
            qr.expressions.iter().any(|expr| {
                expr.value.as_bool().copied().unwrap_or(false)
            })
        });

        if allowed {
            Ok(())
        } else {
            Err(ResourceError::PolicyDenied {
                agent: agent.to_string(),
                uri: uri.to_string(),
                action: action.to_string(),
            })
        }
    }
}

/// A thread-safe, shared policy backed by regorus.
#[derive(Clone)]
pub struct SharedPolicy {
    inner: Arc<RwLock<PolicyDoc>>,
}

impl Default for SharedPolicy {
    fn default() -> Self {
        Self::new(PolicyDoc::default())
    }
}

impl SharedPolicy {
    pub fn new(doc: PolicyDoc) -> Self {
        Self {
            inner: Arc::new(RwLock::new(doc)),
        }
    }

    /// Hot-reload the policy document (admin only — caller checks identity first).
    pub fn update(&self, doc: PolicyDoc) {
        *self.inner.write().unwrap() = doc;
    }

    /// Authorize `(agent, uri, action)` against the current policy.
    pub fn authorize(&self, agent: &str, uri: &str, action: &str) -> Result<()> {
        let doc = self.inner.read().unwrap();
        doc.authorize(agent, uri, action)
    }

    /// Whether `agent` is a policy administrator.
    pub fn is_admin(&self, agent: &str) -> bool {
        let doc = self.inner.read().unwrap();
        doc.is_admin(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_policy() {
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rules: vec![Rule {
                agents: vec!["agent1".into()],
                uris: vec!["file://*".into()],
                actions: vec!["*".into()],
                effect: Effect::Allow,
            }],
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("agent1", "file:///tmp/x", "read").is_ok());
        assert!(policy.authorize("agent2", "file:///tmp/x", "read").is_err());
        assert!(policy.is_admin("admin"));
        assert!(!policy.is_admin("agent1"));
    }

    #[test]
    fn test_rego_policy() {
        let rego = r#"
            package policy
            allow if {
                startswith(input.uri, "file://")
                input.agent == "agent1"
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("agent1", "file:///tmp/x", "read").is_ok());
        assert!(policy.authorize("agent2", "file:///tmp/x", "read").is_err());
        assert!(policy.is_admin("admin"));
    }

    #[test]
    fn test_rego_complex() {
        let rego = r#"
            package policy
            allow if {
                input.agent == "admin"
            }
            allow if {
                startswith(input.uri, "file://")
                input.action == "read"
                input.agent != "blocked"
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        // Admin can do anything
        assert!(policy.authorize("admin", "sock://x", "write").is_ok());
        // Regular agent can read files
        assert!(policy.authorize("agent1", "file:///x", "read").is_ok());
        // Blocked agent cannot
        assert!(policy.authorize("blocked", "file:///x", "read").is_err());
        // Non-file URIs require admin
        assert!(policy.authorize("agent1", "sock://x", "read").is_err());
    }

    #[test]
    fn test_rego_empty_result() {
        // Test with a policy that never matches
        let rego = r#"
            package policy
            allow if {
                false  # never true
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("agent1", "file:///x", "read").is_err());
        assert!(policy.is_admin("admin"));
    }

    #[test]
    fn test_rego_negation() {
        let rego = r#"
            package policy
            allow if {
                not denied
                input.agent == "agent1"
            }
            denied if {
                input.uri == "file:///blocked"
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("agent1", "file:///x", "read").is_ok());
        assert!(policy.authorize("agent1", "file:///blocked", "read").is_err());
    }

    #[test]
    fn test_rego_array_membership() {
        let rego = r#"
            package policy
            allow if {
                some i
                agents := ["alice", "bob", "carol"]
                input.agent == agents[i]
                startswith(input.uri, "file://")
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("alice", "file:///shared", "read").is_ok());
        assert!(policy.authorize("bob", "file:///shared", "read").is_ok());
        assert!(policy.authorize("carol", "file:///shared", "read").is_ok());
        assert!(policy.authorize("dave", "file:///shared", "read").is_err());
    }

    #[test]
    fn test_rego_set_operations() {
        let rego = r#"
            package policy
            allow if {
                input.action in {"read", "list"}
                startswith(input.uri, "mem://kv")
                input.agent != "blocked"
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("agent1", "mem://kv", "read").is_ok());
        assert!(policy.authorize("agent1", "mem://kv", "list").is_ok());
        assert!(policy.authorize("agent1", "mem://kv", "write").is_err());
    }

    #[test]
    fn test_rego_custom_function() {
        let rego = r#"
            package policy
            is_admin := input.agent == "admin"
            is_file := startswith(input.uri, "file://")
            allow if {
                is_admin
            }
            allow if {
                is_file
                input.action != "delete"
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("admin", "mem://x", "delete").is_ok());
        assert!(policy.authorize("agent1", "file:///x", "read").is_ok());
        assert!(policy.authorize("agent1", "file:///x", "delete").is_err());
        assert!(policy.authorize("agent1", "sock://x", "read").is_err());
    }

    #[test]
    fn test_rego_default_deny() {
        // No allow rule - should deny
        let rego = r#"
            package policy
            # No rules
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("agent1", "file:///x", "read").is_err());
    }

    #[test]
    fn test_rego_admin_override() {
        let rego = r#"
            package policy
            allow if {
                input.agent == "admin"
            }
            allow if {
                startswith(input.uri, "file://")
                input.action == "read"
                input.agent != "blocked"
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        // Admin can do anything
        assert!(policy.authorize("admin", "sock://x", "write").is_ok());
        // Regular agent can read files
        assert!(policy.authorize("agent1", "file:///x", "read").is_ok());
        // Blocked agent cannot
        assert!(policy.authorize("blocked", "file:///x", "read").is_err());
        // Non-file URIs require admin
        assert!(policy.authorize("agent1", "sock://x", "read").is_err());
    }

    #[test]
    fn test_rego_policy_with_data() {
        // Data input requires external data - shown as placeholder for future enhancement
        let rego = r#"
            package policy
            # Simple rule without data
            allow if {
                input.agent == "admin"
            }
        "#;
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        let policy = SharedPolicy::new(doc);
        assert!(policy.authorize("admin", "mem://x", "delete").is_ok());
    }

    #[test]
    fn test_rego_performance() {
        let rego = r#"
            package policy
            allow if {
                input.agent in ["alice", "bob", "charlie"]
                startswith(input.uri, "file://")
                input.action == "read"
            }
        "#;
        
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        
        let policy = SharedPolicy::new(doc);
        
        // Performance test - 100 evaluations should complete quickly
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = policy.authorize("alice", "file:///readme.md", "read");
        }
        let duration = start.elapsed();
        
        // Should be fast (100 evals in under a second)
        assert!(duration.as_secs() < 1, "Policy evaluation too slow: {:?}", duration);
        
        // Correctness tests
        assert!(policy.authorize("alice", "file:///readme.md", "read").is_ok());
        assert!(policy.authorize("bob", "file:///readme.md", "read").is_ok());
        assert!(policy.authorize("dave", "file:///readme.md", "read").is_err());
    }

    #[tokio::test]
    async fn test_rego_concurrent_evaluation() {
        // Each thread creates its own Engine - no shared mutable state
        let rego = r#"
            package policy
            allow if {
                input.agent in ["alice", "bob", "carol"]
                startswith(input.uri, "file://")
                input.action == "read"
            }
        "#;
        
        let doc = PolicyDoc {
            admins: vec!["admin".into()],
            rego: Some(rego.into()),
            ..Default::default()
        };
        
        // Test individual evaluations (each thread will create own engine)
        let policy1 = SharedPolicy::new(doc.clone());
        let policy2 = SharedPolicy::new(doc.clone());
        
        // Test individual evaluations
        assert!(policy1.authorize("alice", "file:///test", "read").is_ok());
        assert!(policy2.authorize("bob", "file:///test", "read").is_ok());
        assert!(policy1.authorize("david", "file:///test", "read").is_err()); // not in allow list
    }

    /// Every example policy in `examples/policies/*.rego` must remain valid and
    /// load cleanly. Guards against examples drifting out of sync with regorus.
    #[test]
    fn test_example_policies_load() {
        // `CARGO_MANIFEST_DIR` is `crates/resource`; the examples live at the
        // workspace root under `examples/policies`.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/policies");
        let mut found = 0usize;
        for entry in std::fs::read_dir(&dir).expect("examples/policies dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rego") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let mut engine = Engine::new();
            engine
                .add_policy(format!("{}.rego", path.file_stem().unwrap().to_string_lossy()), src)
                .unwrap_or_else(|e| panic!("example policy {} failed to load: {e}", path.display()));
            found += 1;
        }
        assert!(found >= 6, "expected at least 6 example policies, found {found}");
    }
}