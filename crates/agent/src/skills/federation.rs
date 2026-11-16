//! Governed federated Skill metadata discovery.
//!
//! A peer publishes descriptors only; instruction bodies never cross this
//! boundary by default. The directory applies local policy (peer, workspace,
//! publisher, digest, expiry), merges deterministically, and withdraws metadata
//! that a peer stops publishing. Loading remote instructions is a separate,
//! explicit grant.

use super::index::{RemoteAdmission, RemoteSkill, SkillIndex};
use super::SkillMetadata;
use std::collections::{BTreeMap, BTreeSet};

/// One peer's metadata endpoint.
pub trait SkillMetadataTransport: Send + Sync {
    /// Fetch the peer's descriptors for one workspace.
    fn fetch(&self, peer: &str, workspace: &str) -> Result<Vec<RemoteSkill>, String>;
}

/// Local policy for federated discovery.
#[derive(Debug, Clone)]
pub struct FederationPolicy {
    /// Peers this node may query.
    pub allowed_peers: BTreeSet<String>,
    /// Workspaces for which remote metadata is in scope.
    pub allowed_workspaces: BTreeSet<String>,
    /// Maximum descriptors accepted from one peer per refresh.
    pub max_descriptors: usize,
    /// Whether remote instruction bodies may ever be loaded.
    pub allow_remote_instructions: bool,
    /// Publishers whose remote instructions are trusted when loading is on.
    pub trusted_publishers: BTreeSet<String>,
}

impl Default for FederationPolicy {
    fn default() -> Self {
        Self {
            allowed_peers: BTreeSet::new(),
            allowed_workspaces: BTreeSet::new(),
            max_descriptors: 128,
            // Discovery is safe by default; execution/instruction loading is
            // an explicit grant.
            allow_remote_instructions: false,
            trusted_publishers: BTreeSet::new(),
        }
    }
}

/// Why federated discovery or a remote load was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FederationError {
    /// The peer is not in the local allow list.
    #[error("peer '{0}' is not allowed")]
    PeerDenied(String),
    /// The workspace is not in the local allow list.
    #[error("workspace '{0}' is not allowed")]
    WorkspaceDenied(String),
    /// The peer could not be reached.
    #[error("peer '{peer}' is unavailable: {reason}")]
    Unreachable {
        /// The peer.
        peer: String,
        /// The transport's reason.
        reason: String,
    },
    /// No trusted remote descriptor answers to the name.
    #[error("remote skill '{0}' is not available")]
    NotAvailable(String),
    /// The host has not granted remote instruction loading.
    #[error("remote instructions for '{0}' require an explicit trust grant")]
    InstructionsDenied(String),
    /// The publisher is not trusted for instructions.
    #[error("publisher of '{0}' is not trusted")]
    PublisherDenied(String),
    /// A remote descriptor has no digest, so its integrity cannot be checked.
    #[error("remote skill '{0}' has no content digest")]
    MissingDigest(String),
}

/// What one refresh did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshReport {
    /// Descriptors admitted.
    pub accepted: usize,
    /// Identical descriptors already present.
    pub duplicates: usize,
    /// Expired descriptors.
    pub stale: usize,
    /// Descriptors shadowed by a local Skill.
    pub shadowed: usize,
    /// Previously published descriptors withdrawn.
    pub withdrawn: usize,
    /// Per-peer failures, in peer order.
    pub errors: Vec<FederationError>,
}

/// A local catalogue plus the remote descriptors admitted into it.
pub struct FederatedSkills {
    index: SkillIndex,
    policy: FederationPolicy,
    /// Names each peer published on the last successful refresh.
    owned: BTreeMap<String, BTreeSet<String>>,
}

impl FederatedSkills {
    /// Wrap a local catalogue in the policy.
    pub fn new(index: SkillIndex, policy: FederationPolicy) -> Self {
        Self {
            index,
            policy,
            owned: BTreeMap::new(),
        }
    }

    /// The merged catalogue.
    pub fn index(&self) -> &SkillIndex {
        &self.index
    }

    /// Refresh from the given peers.
    ///
    /// Peers are visited in name order, so the merged result does not depend
    /// on transport order. A peer that is not listed is treated as withdrawn:
    /// its previously published names are removed. An unreachable peer keeps
    /// its last good metadata and reports the failure.
    pub fn refresh(
        &mut self,
        transports: &[(&str, &dyn SkillMetadataTransport)],
        workspace: &str,
        now_ms: u64,
    ) -> RefreshReport {
        let mut report = RefreshReport::default();
        if !self.policy.allowed_workspaces.contains(workspace) {
            report
                .errors
                .push(FederationError::WorkspaceDenied(workspace.to_string()));
            return report;
        }

        let mut listed: Vec<&str> = transports.iter().map(|(peer, _)| *peer).collect();
        listed.sort();
        listed.dedup();

        // Peers no longer listed are withdrawn entirely.
        let withdrawn_peers: Vec<String> = self
            .owned
            .keys()
            .filter(|peer| !listed.iter().any(|listed| listed == peer))
            .cloned()
            .collect();
        for peer in withdrawn_peers {
            if let Some(names) = self.owned.remove(&peer) {
                for name in names {
                    self.withdraw(&peer, &name, &mut report);
                }
            }
        }

        let mut sorted: Vec<&(&str, &dyn SkillMetadataTransport)> = transports.iter().collect();
        sorted.sort_by(|left, right| left.0.cmp(right.0));
        for (peer, transport) in sorted {
            if !self.policy.allowed_peers.contains(*peer) {
                report
                    .errors
                    .push(FederationError::PeerDenied((*peer).to_string()));
                continue;
            }
            let fetched = match transport.fetch(peer, workspace) {
                Ok(fetched) => fetched,
                Err(reason) => {
                    report.errors.push(FederationError::Unreachable {
                        peer: (*peer).to_string(),
                        reason,
                    });
                    continue;
                }
            };
            let mut fetched = fetched;
            fetched.sort_by(|left, right| left.metadata.name.cmp(&right.metadata.name));
            fetched.truncate(self.policy.max_descriptors);
            let names: BTreeSet<String> = fetched
                .iter()
                .map(|candidate| candidate.metadata.name.clone())
                .collect();

            // Withdraw names this peer used to publish and no longer does.
            if let Some(previous) = self.owned.get(*peer).cloned() {
                for name in previous.difference(&names) {
                    self.withdraw(peer, name, &mut report);
                }
            }

            for candidate in fetched {
                let name = candidate.metadata.name.clone();
                let candidate = RemoteSkill {
                    peer: (*peer).to_string(),
                    ..candidate
                };
                let merged = self.index.merge_remote(std::iter::once(candidate), now_ms);
                match merged
                    .admissions
                    .first()
                    .map(|(_, admission)| admission.clone())
                {
                    Some(RemoteAdmission::Accepted) => report.accepted += 1,
                    Some(RemoteAdmission::Duplicate) => report.duplicates += 1,
                    Some(RemoteAdmission::Stale) => report.stale += 1,
                    Some(RemoteAdmission::ShadowedLocal) => report.shadowed += 1,
                    None => {}
                }
                let _ = name;
            }
            self.owned.insert((*peer).to_string(), names);
        }
        report
    }

    /// Remove one stale remote entry, but only if the peer still owns it.
    fn withdraw(&mut self, peer: &str, name: &str, report: &mut RefreshReport) {
        let _ = peer;
        let removable = self
            .index
            .get(name)
            .is_some_and(|metadata| metadata.source == super::SkillSource::Federated);
        if removable && self.index.remove(name).is_some() {
            report.withdrawn += 1;
        }
    }

    /// Authorize loading a remote Skill's instructions.
    ///
    /// Discovery never implies this grant: the host must enable remote
    /// instructions, trust the publisher, and the descriptor must carry a
    /// digest for integrity verification.
    pub fn authorize_instructions(&self, name: &str) -> Result<&SkillMetadata, FederationError> {
        let metadata = self
            .index
            .get(name)
            .ok_or_else(|| FederationError::NotAvailable(name.to_string()))?;
        if metadata.source != super::SkillSource::Federated {
            return Err(FederationError::NotAvailable(name.to_string()));
        }
        if !self.policy.allow_remote_instructions {
            return Err(FederationError::InstructionsDenied(name.to_string()));
        }
        let trusted = metadata
            .publisher
            .as_deref()
            .is_some_and(|publisher| self.policy.trusted_publishers.contains(publisher));
        if !trusted {
            return Err(FederationError::PublisherDenied(name.to_string()));
        }
        if metadata.digest.is_empty() {
            return Err(FederationError::MissingDigest(name.to_string()));
        }
        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillMetadata, SkillSource, SkillVersion};
    use std::path::PathBuf;

    struct FakeTransport {
        descriptors: Vec<RemoteSkill>,
        error: Option<String>,
    }

    impl SkillMetadataTransport for FakeTransport {
        fn fetch(&self, _peer: &str, _workspace: &str) -> Result<Vec<RemoteSkill>, String> {
            match &self.error {
                Some(error) => Err(error.clone()),
                None => Ok(self.descriptors.clone()),
            }
        }
    }

    fn remote(name: &str, version: (u32, u32, u32), digest: &str, peer: &str) -> RemoteSkill {
        let mut metadata = SkillMetadata::new(
            name.to_string(),
            format!("{name} from {peer}"),
            PathBuf::from(format!("/remote/{name}/SKILL.md")),
        );
        metadata.version = SkillVersion::new(version.0, version.1, version.2);
        metadata.digest = digest.to_string();
        metadata.source = SkillSource::Federated;
        RemoteSkill {
            metadata,
            expires_at_ms: None,
            peer: peer.to_string(),
        }
    }

    fn policy() -> FederationPolicy {
        FederationPolicy {
            allowed_peers: ["peer-a".to_string(), "peer-b".to_string()]
                .into_iter()
                .collect(),
            allowed_workspaces: ["ws".to_string()].into_iter().collect(),
            max_descriptors: 8,
            allow_remote_instructions: false,
            trusted_publishers: BTreeSet::new(),
        }
    }

    #[test]
    fn refresh_merges_reachable_peers_and_reports_unreachable_ones() {
        let a = FakeTransport {
            descriptors: vec![remote("alpha", (1, 0, 0), "da", "peer-a")],
            error: None,
        };
        let b = FakeTransport {
            descriptors: Vec::new(),
            error: Some("timeout".to_string()),
        };
        let mut directory = FederatedSkills::new(SkillIndex::default(), policy());
        let report = directory.refresh(&[("peer-b", &b), ("peer-a", &a)], "ws", 0);
        assert_eq!(report.accepted, 1);
        assert_eq!(report.errors.len(), 1);
        assert!(matches!(
            report.errors[0],
            FederationError::Unreachable { .. }
        ));
        assert!(directory.index().get("alpha").is_some());
    }

    #[test]
    fn a_second_refresh_withdraws_metadata_a_peer_stopped_publishing() {
        let first = FakeTransport {
            descriptors: vec![
                remote("alpha", (1, 0, 0), "da", "peer-a"),
                remote("beta", (1, 0, 0), "db", "peer-a"),
            ],
            error: None,
        };
        let mut directory = FederatedSkills::new(SkillIndex::default(), policy());
        directory.refresh(&[("peer-a", &first)], "ws", 0);
        assert!(directory.index().get("beta").is_some());

        let second = FakeTransport {
            descriptors: vec![remote("alpha", (1, 0, 0), "da", "peer-a")],
            error: None,
        };
        let report = directory.refresh(&[("peer-a", &second)], "ws", 0);
        assert_eq!(report.withdrawn, 1);
        assert!(directory.index().get("alpha").is_some());
        assert!(
            directory.index().get("beta").is_none(),
            "a withdrawn descriptor is not presented as current"
        );
    }

    #[test]
    fn a_local_skill_wins_a_remote_name_collision() {
        let mut local = SkillMetadata::new(
            "alpha".to_string(),
            "local alpha".to_string(),
            PathBuf::from("/local/alpha/SKILL.md"),
        );
        local.digest = "local-digest".to_string();
        local.source = SkillSource::User;
        let a = FakeTransport {
            descriptors: vec![remote("alpha", (9, 0, 0), "remote-digest", "peer-a")],
            error: None,
        };
        let mut directory = FederatedSkills::new(SkillIndex::from_metadata([local]), policy());
        let report = directory.refresh(&[("peer-a", &a)], "ws", 0);
        assert_eq!(report.shadowed, 1);
        let winner = directory
            .index()
            .get("alpha")
            .expect("local entry survives");
        assert_eq!(winner.source, SkillSource::User);
        assert_eq!(winner.digest, "local-digest");
        assert_eq!(winner.publisher, None);
    }

    #[test]
    fn policy_refuses_a_denied_workspace_or_peer_before_fetching() {
        let a = FakeTransport {
            descriptors: vec![remote("alpha", (1, 0, 0), "da", "peer-a")],
            error: None,
        };
        let mut directory = FederatedSkills::new(SkillIndex::default(), policy());
        let report = directory.refresh(&[("peer-a", &a)], "other", 0);
        assert_eq!(
            report.errors,
            vec![FederationError::WorkspaceDenied("other".to_string())]
        );
        assert!(directory.index().is_empty());

        let mut directory = FederatedSkills::new(SkillIndex::default(), policy());
        let report = directory.refresh(&[("peer-z", &a)], "ws", 0);
        assert_eq!(
            report.errors,
            vec![FederationError::PeerDenied("peer-z".to_string())]
        );
    }

    #[test]
    fn duplicate_identities_and_equal_versions_are_deterministic() {
        let a = FakeTransport {
            descriptors: vec![remote("same", (1, 0, 0), "dsame", "peer-a")],
            error: None,
        };
        let b = FakeTransport {
            descriptors: vec![remote("same", (1, 0, 0), "dsame", "peer-b")],
            error: None,
        };
        let mut directory = FederatedSkills::new(SkillIndex::default(), policy());
        let report = directory.refresh(&[("peer-a", &a), ("peer-b", &b)], "ws", 0);
        assert_eq!(report.accepted, 1);
        assert_eq!(report.duplicates, 1);
        assert_eq!(directory.index().len(), 1, "one identity, not two rows");
    }

    #[test]
    fn loading_remote_instructions_needs_an_explicit_trust_grant() {
        let a = FakeTransport {
            descriptors: vec![remote("alpha", (1, 0, 0), "da", "peer-a")],
            error: None,
        };
        let mut directory = FederatedSkills::new(SkillIndex::default(), policy());
        directory.refresh(&[("peer-a", &a)], "ws", 0);
        assert_eq!(
            directory.authorize_instructions("alpha"),
            Err(FederationError::InstructionsDenied("alpha".to_string()))
        );

        let mut granted = policy();
        granted.allow_remote_instructions = true;
        granted.trusted_publishers.insert("peer-a".to_string());
        let mut directory = FederatedSkills::new(SkillIndex::default(), granted);
        directory.refresh(&[("peer-a", &a)], "ws", 0);
        assert!(directory.authorize_instructions("alpha").is_ok());

        let mut untrusted = policy();
        untrusted.allow_remote_instructions = true;
        let mut directory = FederatedSkills::new(SkillIndex::default(), untrusted);
        directory.refresh(&[("peer-a", &a)], "ws", 0);
        assert_eq!(
            directory.authorize_instructions("alpha"),
            Err(FederationError::PublisherDenied("alpha".to_string()))
        );
    }

    #[test]
    fn a_remote_descriptor_without_a_digest_cannot_be_loaded() {
        let a = FakeTransport {
            descriptors: vec![remote("alpha", (1, 0, 0), "", "peer-a")],
            error: None,
        };
        let mut granted = policy();
        granted.allow_remote_instructions = true;
        granted.trusted_publishers.insert("peer-a".to_string());
        let mut directory = FederatedSkills::new(SkillIndex::default(), granted);
        directory.refresh(&[("peer-a", &a)], "ws", 0);
        assert_eq!(
            directory.authorize_instructions("alpha"),
            Err(FederationError::MissingDigest("alpha".to_string()))
        );
    }
}
