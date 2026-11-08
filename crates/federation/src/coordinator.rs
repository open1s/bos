//! An optional coordinator: membership administration and topology only.
//!
//! The coordinator is NOT a data-plane proxy and never grants execution
//! authority. It records who a federation has admitted, what each member
//! advertises (catalogue revision, policy revision, endpoint) and its
//! administrative status. Provider-side authorization remains local: a member
//! that the coordinator marks active still needs the local grants to invoke
//! anything. Nodes keep operating peer-to-peer while the coordinator is down.

use std::collections::BTreeMap;

use capability::NodeId;
use serde::{Deserialize, Serialize};

/// How a member stands in the federation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    /// Admitted, not yet confirmed by the member.
    Invited,
    /// Active.
    Active,
    /// Temporarily stopped.
    Suspended,
    /// Removed.
    Revoked,
}

/// One member the coordinator knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRecord {
    /// The member Node.
    pub node: NodeId,
    /// Administrative status.
    pub status: MemberStatus,
    /// The member's last advertised catalogue revision.
    #[serde(default)]
    pub catalogue_revision: u64,
    /// The member's last advertised policy revision.
    #[serde(default)]
    pub policy_revision: u64,
    /// The member's advertised endpoint, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// When the coordinator last heard from it, in milliseconds since the epoch.
    #[serde(default)]
    pub updated_at_ms: u64,
}

/// A topology snapshot an operator can inspect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    /// The members, in Node order.
    pub members: Vec<MemberRecord>,
    /// The revision of the membership document.
    pub revision: u64,
    /// The policy revision the coordinator considers current.
    pub policy_revision: u64,
}

/// Why a coordinator operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoordinatorError {
    /// The caller is not the configured administrator.
    #[error("only the coordinator's administrator may do that")]
    NotAdmin,
    /// A member with that Node id already exists in an incompatible state.
    #[error("member {node} is already registered")]
    Duplicate {
        /// The member.
        node: NodeId,
    },
    /// No member with that id.
    #[error("member {node} is unknown")]
    Unknown {
        /// The member.
        node: NodeId,
    },
    /// The membership document could not be read or written.
    #[error("cannot access {path}: {message}")]
    Storage {
        /// The file.
        path: String,
        /// What happened.
        message: String,
    },
    /// A policy revision cannot move backwards.
    #[error("policy revision {requested} is older than {current}")]
    StalePolicy {
        /// What the administrator asked for.
        requested: u64,
        /// The current revision.
        current: u64,
    },
}

/// The optional coordinator state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coordinator {
    #[serde(default)]
    members: BTreeMap<NodeId, MemberRecord>,
    #[serde(default)]
    revision: u64,
    /// The policy revision the coordinator considers current.
    #[serde(default)]
    policy_revision: u64,
    /// The one Node allowed to administer the membership. Absent means the
    /// coordinator is read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admin: Option<NodeId>,
}

impl Coordinator {
    /// A coordinator administered by one Node.
    pub fn new(admin: Option<NodeId>) -> Self {
        Self {
            members: BTreeMap::new(),
            revision: 0,
            policy_revision: 0,
            admin,
        }
    }

    /// The administrator, when one is configured.
    pub fn admin(&self) -> Option<&NodeId> {
        self.admin.as_ref()
    }

    /// Record or update a member. Only the administrator may call it.
    pub fn register(
        &mut self,
        actor: &NodeId,
        member: MemberRecord,
    ) -> Result<(), CoordinatorError> {
        if self.admin.as_ref() != Some(actor) {
            return Err(CoordinatorError::NotAdmin);
        }
        self.members.insert(member.node.clone(), member);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    /// Suspend a member. Only the administrator may call it.
    pub fn suspend(&mut self, actor: &NodeId, node: &NodeId) -> Result<(), CoordinatorError> {
        self.set_status(actor, node, MemberStatus::Suspended)
    }

    /// Remove a member from the membership. Only the administrator may call it.
    pub fn revoke(&mut self, actor: &NodeId, node: &NodeId) -> Result<(), CoordinatorError> {
        self.set_status(actor, node, MemberStatus::Revoked)
    }

    fn set_status(
        &mut self,
        actor: &NodeId,
        node: &NodeId,
        status: MemberStatus,
    ) -> Result<(), CoordinatorError> {
        if self.admin.as_ref() != Some(actor) {
            return Err(CoordinatorError::NotAdmin);
        }
        let record = self
            .members
            .get_mut(node)
            .ok_or_else(|| CoordinatorError::Unknown { node: node.clone() })?;
        record.status = status;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    /// Set the current policy revision. Only the administrator may call it;
    /// the revision never moves backwards.
    pub fn set_policy_revision(
        &mut self,
        actor: &NodeId,
        revision: u64,
    ) -> Result<(), CoordinatorError> {
        if self.admin.as_ref() != Some(actor) {
            return Err(CoordinatorError::NotAdmin);
        }
        if revision < self.policy_revision {
            return Err(CoordinatorError::StalePolicy {
                requested: revision,
                current: self.policy_revision,
            });
        }
        self.policy_revision = revision;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    /// Members whose last advertised policy revision is behind the current one.
    ///
    /// The coordinator never pushes policy; this is the report an operator (or
    /// a member reading the topology) uses to decide who must be re-informed.
    pub fn members_behind(&self) -> Vec<MemberRecord> {
        self.members
            .values()
            .filter(|member| member.policy_revision < self.policy_revision)
            .cloned()
            .collect()
    }

    /// The current topology.
    pub fn topology(&self) -> Topology {
        Topology {
            members: self.members.values().cloned().collect(),
            revision: self.revision,
            policy_revision: self.policy_revision,
        }
    }

    /// One member.
    pub fn member(&self, node: &NodeId) -> Option<&MemberRecord> {
        self.members.get(node)
    }

    /// Serialize for persistence.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Load a membership document, or an empty one when the file is missing.
    pub fn load(path: &std::path::Path) -> Result<Self, CoordinatorError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| CoordinatorError::Storage {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(CoordinatorError::Storage {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }

    /// Write the membership atomically.
    pub fn save(&self, path: &std::path::Path) -> Result<(), CoordinatorError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| CoordinatorError::Storage {
                path: parent.display().to_string(),
                message: error.to_string(),
            })?;
        }
        let temporary = path.with_extension("json.part");
        std::fs::write(&temporary, self.to_json()).map_err(|error| CoordinatorError::Storage {
            path: temporary.display().to_string(),
            message: error.to_string(),
        })?;
        std::fs::rename(&temporary, path).map_err(|error| CoordinatorError::Storage {
            path: path.display().to_string(),
            message: error.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(value: &str) -> NodeId {
        NodeId::new(value).expect("a node id")
    }

    fn member(value: &str) -> MemberRecord {
        MemberRecord {
            node: node(value),
            status: MemberStatus::Active,
            catalogue_revision: 3,
            policy_revision: 7,
            endpoint: None,
            updated_at_ms: 1_000,
        }
    }

    #[test]
    fn only_the_administrator_can_change_the_membership() {
        let admin = node("node-admin");
        let mut coordinator = Coordinator::new(Some(admin.clone()));
        coordinator
            .register(&admin, member("node-b"))
            .expect("the admin registers");
        assert_eq!(coordinator.topology().members.len(), 1);
        assert_eq!(coordinator.topology().revision, 1);

        let stranger = node("node-x");
        assert_eq!(
            coordinator.register(&stranger, member("node-c")),
            Err(CoordinatorError::NotAdmin)
        );
        assert_eq!(coordinator.topology().members.len(), 1);
        assert_eq!(
            coordinator.suspend(&stranger, &node("node-b")),
            Err(CoordinatorError::NotAdmin)
        );
        coordinator
            .suspend(&admin, &node("node-b"))
            .expect("the admin suspends");
        assert_eq!(
            coordinator.member(&node("node-b")).expect("member").status,
            MemberStatus::Suspended
        );
    }

    #[test]
    fn a_read_only_coordinator_refuses_every_change() {
        let mut coordinator = Coordinator::new(None);
        assert_eq!(
            coordinator.register(&node("node-a"), member("node-b")),
            Err(CoordinatorError::NotAdmin)
        );
    }

    #[test]
    fn policy_revisions_are_admin_gated_and_monotonic() {
        let admin = node("node-admin");
        let mut coordinator = Coordinator::new(Some(admin.clone()));
        coordinator
            .set_policy_revision(&admin, 5)
            .expect("the admin sets the revision");
        assert_eq!(coordinator.topology().policy_revision, 5);
        assert_eq!(
            coordinator.set_policy_revision(&admin, 4),
            Err(CoordinatorError::StalePolicy {
                requested: 4,
                current: 5
            })
        );
        assert_eq!(
            coordinator.set_policy_revision(&node("node-x"), 6),
            Err(CoordinatorError::NotAdmin)
        );
    }

    #[test]
    fn members_behind_the_policy_are_reported() {
        let admin = node("node-admin");
        let mut coordinator = Coordinator::new(Some(admin.clone()));
        coordinator.set_policy_revision(&admin, 5).expect("policy");
        let mut behind = member("node-b");
        behind.policy_revision = 3;
        let mut current = member("node-c");
        current.policy_revision = 5;
        coordinator.register(&admin, behind).expect("registered");
        coordinator.register(&admin, current).expect("registered");
        let stale = coordinator.members_behind();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].node, node("node-b"));
        assert_eq!(coordinator.topology().members.len(), 2);
    }

    #[test]
    fn the_membership_document_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("federation-coordinator-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("coordinator.json");
        let admin = node("node-admin");
        let mut coordinator = Coordinator::new(Some(admin.clone()));
        coordinator.set_policy_revision(&admin, 7).expect("policy");
        coordinator
            .register(&admin, member("node-b"))
            .expect("registered");
        coordinator.save(&path).expect("save");
        let loaded = Coordinator::load(&path).expect("load");
        assert_eq!(loaded.topology(), coordinator.topology());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
