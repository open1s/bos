//! The federation management CLI and the files a node keeps.
//!
//! A node's identity and its peer grants are files under bsh's own directory,
//! so the same commands work headless: status, enroll, revoke, grant, and
//! revoke-grant. Nothing here starts a runtime; it manages the state a runtime
//! loads.

use std::io::Write;
use std::path::{Path, PathBuf};

use agent::capability::{unix_ms, CapabilityId, NodeId};
use federation::identity::is_valid_node_id;
use federation::{
    DelegationState, DelegationStore, NodeIdentity, PeerStore, RotationRecord, TrustState,
};

/// The directory a federation node keeps its files in.
pub(crate) fn dir() -> PathBuf {
    crate::settings::dir().join("federation")
}

/// The name a node shows to peers: the configured one, or the machine's name.
///
/// The host name is the useful default — a reader recognises the machine a peer
/// runs on far better than the first eight characters of a public key.
pub(crate) fn resolve_node_name(configured: Option<&str>) -> String {
    if let Some(name) = configured.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_string();
    }
    let host = hostname::get()
        .ok()
        .and_then(|host| host.into_string().ok())
        .unwrap_or_default();
    let host = host.trim().trim_end_matches(".local");
    if host.is_empty() {
        "bsh node".to_string()
    } else {
        host.to_string()
    }
}

const USAGE: &str = "\
bsh federation <command>

  status                                  show this node's id, its peers, and
                                          the last decisions
  enroll <node-id>                        trust a peer
  revoke <node-id>                        revoke a peer and its grants
  grant <node-id> <capability-id>         let a peer invoke one capability
  revoke-grant <node-id> <capability-id>  withdraw one grant
  rotate [reason]                         replace this node's key and write a
                                          signed rotation record for peers
  accept-rotation <record.json>           apply a peer's signed key rotation

Identity and peers live in bsh's own directory; a peer id is a base64 Ed25519
public key, which is what a peer prints as its node id.";

/// Run one federation command against the files under `root`.
pub(crate) fn command(
    args: &[String],
    root: &Path,
    name: Option<&str>,
    out: &mut dyn Write,
) -> i32 {
    let identity_path = root.join("node.key");
    let peers_path = root.join("peers.json");
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            let _ = writeln!(out, "{USAGE}");
            0
        }
        Some("status") => match NodeIdentity::load_or_create(&identity_path) {
            Ok(identity) => {
                let _ = writeln!(out, "node {}", identity.node_id());
                if let Some(name) = name {
                    let _ = writeln!(out, "name {name}");
                }
                let _ = writeln!(out, "identity {}", identity_path.display());
                match PeerStore::load(&peers_path) {
                    Ok(store) => {
                        let mut peers: Vec<_> = store.iter().collect();
                        peers.sort_by(|left, right| left.node.cmp(&right.node));
                        let _ = writeln!(out, "peers {}", peers.len());
                        for record in peers {
                            let grants: Vec<String> = record
                                .grants
                                .iter()
                                .map(|grant| grant.capability.to_string())
                                .collect();
                            let state = match record.state {
                                TrustState::Pending => "pending",
                                TrustState::Trusted => "trusted",
                                TrustState::Revoked => "revoked",
                            };
                            let _ = writeln!(
                                out,
                                "  {} {} grants={} {}",
                                record.node,
                                state,
                                grants.len(),
                                grants.join(",")
                            );
                        }
                        // The last decisions this node made, from the log the
                        // runtime writes. Read once per invocation, never per
                        // render: this command is the only reader here.
                        // A maintenance command is the right place to drop
                        // files no live process can still be writing.
                        let _ = crate::federation_audit::prune_stale(
                            &root.join("audit.jsonl"),
                            std::time::SystemTime::now(),
                        );
                        let delegation_path = root.join("delegations.json");
                        if let Ok(delegations) = DelegationStore::load(&delegation_path) {
                            let mut records: Vec<_> = delegations.iter().collect();
                            records.sort_by(|left, right| {
                                right.updated_at_ms.cmp(&left.updated_at_ms)
                            });
                            let active = records
                                .iter()
                                .filter(|record| record.state.is_active())
                                .count();
                            let _ =
                                writeln!(out, "delegations {} active {}", records.len(), active);
                            for record in records.into_iter().take(5) {
                                let state = match record.state {
                                    DelegationState::Created => "created",
                                    DelegationState::Accepted => "accepted",
                                    DelegationState::Running => "running",
                                    DelegationState::Succeeded => "succeeded",
                                    DelegationState::Failed => "failed",
                                    DelegationState::Cancelled => "cancelled",
                                    DelegationState::Rejected => "rejected",
                                };
                                let _ = writeln!(
                                    out,
                                    "  {} {} {} {}",
                                    record.request.id,
                                    state,
                                    record.request.target,
                                    record.request.objective
                                );
                            }
                        }
                        let recent = crate::federation_audit::load(&root.join("audit.jsonl"), 5);
                        if !recent.is_empty() {
                            let _ = writeln!(out, "recent {}", recent.len());
                            for event in recent {
                                let subject = event.subject.unwrap_or_default();
                                let _ = writeln!(
                                    out,
                                    "  {:?} {} {:?} {} bytes={}",
                                    event.direction,
                                    event.operation,
                                    event.outcome,
                                    subject,
                                    event.bytes.unwrap_or(0)
                                );
                            }
                        }
                        0
                    }
                    Err(error) => fail(out, &error.to_string()),
                }
            }
            Err(error) => fail(out, &error.to_string()),
        },
        Some("rotate") => {
            let _lock = match crate::federation_lock::DataDirLock::acquire(root) {
                Ok(lock) => lock,
                Err(error) => return fail(out, &error),
            };
            let old = match NodeIdentity::load_or_create(&identity_path) {
                Ok(identity) => identity,
                Err(error) => return fail(out, &error.to_string()),
            };
            let new = match NodeIdentity::generate() {
                Ok(identity) => identity,
                Err(error) => return fail(out, &error.to_string()),
            };
            let reason = args
                .iter()
                .skip(1)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            let record = RotationRecord::sign(
                &old,
                &new,
                unix_ms(),
                (!reason.is_empty()).then_some(reason),
            );
            let record_path = root.join("rotation.json");
            if let Err(error) = std::fs::write(&record_path, record.to_json().unwrap_or_default()) {
                return fail(out, &error.to_string());
            }
            if let Err(error) = new.save(&identity_path) {
                return fail(out, &error.to_string());
            }
            let _ = writeln!(out, "old {}", old.node_id());
            let _ = writeln!(out, "new {}", new.node_id());
            let _ = writeln!(out, "rotation {}", record_path.display());
            let _ = writeln!(
                out,
                "give the rotation record to every peer and run: bsh federation accept-rotation <file>"
            );
            0
        }
        Some("accept-rotation") => {
            let _lock = match crate::federation_lock::DataDirLock::acquire(root) {
                Ok(lock) => lock,
                Err(error) => return fail(out, &error),
            };
            let Some(path) = args.get(1) else {
                return fail(out, "accept-rotation needs a record file");
            };
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => return fail(out, &error.to_string()),
            };
            let record = match RotationRecord::from_json(&text) {
                Ok(record) => record,
                Err(error) => return fail(out, &error.to_string()),
            };
            let mut store = match PeerStore::load(&peers_path) {
                Ok(store) => store,
                Err(error) => return fail(out, &error.to_string()),
            };
            match store.rotate(&record, unix_ms()) {
                Ok(()) => match store.save(&peers_path) {
                    Ok(()) => {
                        let _ = writeln!(out, "rotated {} -> {}", record.old, record.new);
                        0
                    }
                    Err(error) => fail(out, &error.to_string()),
                },
                Err(error) => fail(out, &error.to_string()),
            }
        }
        Some("enroll") => {
            // Mutations take the data directory's writer lock: a running app
            // owns it, and a CLI write behind its back would be lost.
            let _lock = match crate::federation_lock::DataDirLock::acquire(root) {
                Ok(lock) => lock,
                Err(error) => return fail(out, &error),
            };
            let node = match node_arg(args.get(1), out) {
                Some(node) => node,
                None => return 1,
            };
            let mut store = match PeerStore::load(&peers_path) {
                Ok(store) => store,
                Err(error) => return fail(out, &error.to_string()),
            };
            store.enroll(node.clone(), unix_ms(), None);
            match store.save(&peers_path) {
                Ok(()) => {
                    let _ = writeln!(out, "enrolled {node}");
                    0
                }
                Err(error) => fail(out, &error.to_string()),
            }
        }
        Some("revoke") => {
            // Mutations take the data directory's writer lock: a running app
            // owns it, and a CLI write behind its back would be lost.
            let _lock = match crate::federation_lock::DataDirLock::acquire(root) {
                Ok(lock) => lock,
                Err(error) => return fail(out, &error),
            };
            let node = match node_arg(args.get(1), out) {
                Some(node) => node,
                None => return 1,
            };
            let mut store = match PeerStore::load(&peers_path) {
                Ok(store) => store,
                Err(error) => return fail(out, &error.to_string()),
            };
            if !store.revoke(&node) {
                return fail(out, &format!("peer {node} is not enrolled"));
            }
            match store.save(&peers_path) {
                Ok(()) => {
                    let _ = writeln!(out, "revoked {node}");
                    0
                }
                Err(error) => fail(out, &error.to_string()),
            }
        }
        Some("grant") => {
            // Mutations take the data directory's writer lock: a running app
            // owns it, and a CLI write behind its back would be lost.
            let _lock = match crate::federation_lock::DataDirLock::acquire(root) {
                Ok(lock) => lock,
                Err(error) => return fail(out, &error),
            };
            let node = match node_arg(args.get(1), out) {
                Some(node) => node,
                None => return 1,
            };
            let capability = match capability_arg(args.get(2), out) {
                Some(capability) => capability,
                None => return 1,
            };
            let mut store = match PeerStore::load(&peers_path) {
                Ok(store) => store,
                Err(error) => return fail(out, &error.to_string()),
            };
            match store.grant(&node, capability.clone(), unix_ms(), None) {
                Ok(()) => match store.save(&peers_path) {
                    Ok(()) => {
                        let _ = writeln!(out, "granted {capability} to {node}");
                        0
                    }
                    Err(error) => fail(out, &error.to_string()),
                },
                Err(error) => fail(out, &error.to_string()),
            }
        }
        Some("revoke-grant") => {
            // Mutations take the data directory's writer lock: a running app
            // owns it, and a CLI write behind its back would be lost.
            let _lock = match crate::federation_lock::DataDirLock::acquire(root) {
                Ok(lock) => lock,
                Err(error) => return fail(out, &error),
            };
            let node = match node_arg(args.get(1), out) {
                Some(node) => node,
                None => return 1,
            };
            let capability = match capability_arg(args.get(2), out) {
                Some(capability) => capability,
                None => return 1,
            };
            let mut store = match PeerStore::load(&peers_path) {
                Ok(store) => store,
                Err(error) => return fail(out, &error.to_string()),
            };
            if !store.revoke_grant(&node, &capability) {
                return fail(out, &format!("{node} holds no grant for {capability}"));
            }
            match store.save(&peers_path) {
                Ok(()) => {
                    let _ = writeln!(out, "withdrew {capability} from {node}");
                    0
                }
                Err(error) => fail(out, &error.to_string()),
            }
        }
        Some(other) => fail(
            out,
            &format!("unknown command {other}; bsh federation help"),
        ),
    }
}

fn node_arg(value: Option<&String>, out: &mut dyn Write) -> Option<NodeId> {
    let Some(value) = value else {
        fail(out, "a node id is required");
        return None;
    };
    let Ok(node) = NodeId::new(value.clone()) else {
        fail(out, "a node id may not be empty");
        return None;
    };
    if !is_valid_node_id(&node) {
        fail(out, "a node id is a base64 Ed25519 public key");
        return None;
    }
    Some(node)
}

fn capability_arg(value: Option<&String>, out: &mut dyn Write) -> Option<CapabilityId> {
    let Some(value) = value else {
        fail(out, "a capability id is required");
        return None;
    };
    match value.parse::<CapabilityId>() {
        Ok(capability) => Some(capability),
        Err(error) => {
            fail(out, &format!("invalid capability id: {error}"));
            None
        }
    }
}

fn fail(out: &mut dyn Write, message: &str) -> i32 {
    let _ = writeln!(out, "error: {message}");
    1
}
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("bsh-fed-cli-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        root
    }

    fn run(root: &Path, args: &[&str]) -> (i32, String) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let mut out = Vec::new();
        let code = command(&args, root, None, &mut out);
        (code, String::from_utf8(out).expect("utf8 output"))
    }

    fn peer() -> String {
        NodeIdentity::from_seed([7u8; 32]).node_id().to_string()
    }

    #[test]
    fn rotate_replaces_the_key_and_a_peer_can_accept_the_record() {
        let root = scratch("rotate");
        let old = NodeIdentity::load_or_create(&root.join("node.key")).expect("old identity");
        let old_id = old.node_id();
        let (code, output) = run(&root, &["rotate", "disk"]);
        assert_eq!(code, 0, "{output}");
        let new = NodeIdentity::load_or_create(&root.join("node.key")).expect("new identity");
        assert_ne!(new.node_id(), old_id);
        let record_path = root.join("rotation.json");
        let record = federation::RotationRecord::from_json(
            &std::fs::read_to_string(&record_path).expect("record"),
        )
        .expect("parse");
        assert_eq!(record.old, old_id);
        assert_eq!(record.new, new.node_id());
        assert!(record.verify());

        let peer_root = scratch("rotate-peer");
        let mut store = federation::PeerStore::new();
        let capability: CapabilityId = "builtin/read_file".parse().expect("capability");
        store.enroll(old_id.clone(), unix_ms(), None);
        store
            .grant(&old_id, capability.clone(), unix_ms(), None)
            .expect("grant");
        store.save(&peer_root.join("peers.json")).expect("peers");
        let (code, output) = run(
            &peer_root,
            &["accept-rotation", record_path.to_str().expect("path")],
        );
        assert_eq!(code, 0, "{output}");
        let store = federation::PeerStore::load(&peer_root.join("peers.json")).expect("load");
        assert!(store.is_trusted(&new.node_id(), unix_ms()));
        store
            .authorize(&new.node_id(), &capability, unix_ms())
            .expect("grant moved");
        assert!(!store.is_trusted(&old_id, unix_ms()));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&peer_root);
    }

    #[test]
    fn status_creates_an_identity_and_reports_it() {
        let root = scratch("status");

        let mut named = Vec::new();
        let code = command(
            &["status".to_string()],
            &root,
            Some("studio-mac"),
            &mut named,
        );
        let named = String::from_utf8(named).expect("utf8 output");
        assert_eq!(code, 0, "{named}");
        assert!(named.contains("name studio-mac"), "{named}");

        let (code, output) = run(&root, &["status"]);

        assert_eq!(code, 0, "{output}");
        assert!(output.contains("node "), "{output}");
        assert!(output.contains("peers 0"), "{output}");
        assert!(root.join("node.key").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_reports_the_recent_federation_decisions() {
        let root = scratch("audit");
        let event = federation::FederationEvent::new(
            federation::Direction::Inbound,
            "peer-a",
            "resource",
            None,
            "quota_exceeded",
        )
        .with_subject("lease-1")
        .with_bytes(4);
        crate::federation_audit::record_bounded(&root.join("audit.jsonl"), &event, 64 * 1024, 10)
            .expect("a record");

        let (code, output) = run(&root, &["status"]);

        assert_eq!(code, 0, "{output}");
        assert!(output.contains("recent 1"), "{output}");
        assert!(output.contains("resource"), "{output}");
        assert!(output.contains("quota_exceeded"), "{output}");
        assert!(output.contains("lease-1"), "{output}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mutations_refuse_while_the_app_holds_the_directory() {
        let root = scratch("locked");
        let _held = crate::federation_lock::DataDirLock::acquire(&root).expect("the app");

        let (code, output) = run(&root, &["enroll", &peer()]);

        assert_ne!(code, 0, "a CLI write behind a running app is refused");
        assert!(output.contains("another BSH instance"), "{output}");

        // Reading still works: status takes no writer lock.
        let (code, _) = run(&root, &["status"]);
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn status_prunes_a_stale_process_file() {
        let root = scratch("prune");
        let stale = root.join("audit-99999.jsonl");
        std::fs::write(&stale, b"{}\n").expect("a stale file");
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .expect("the stale file")
            .set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600),
            )
            .expect("an old mtime");

        let (code, output) = run(&root, &["status"]);

        assert_eq!(code, 0, "{output}");
        assert!(!stale.exists(), "status cleans up what no process owns");
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn enroll_then_status_lists_the_peer() {
        let root = scratch("enroll");
        let peer = peer();

        let (code, output) = run(&root, &["enroll", &peer]);
        assert_eq!(code, 0, "{output}");
        let (code, output) = run(&root, &["status"]);

        assert_eq!(code, 0, "{output}");
        assert!(output.contains("peers 1"), "{output}");
        assert!(output.contains(&peer), "{output}");
        assert!(output.contains("trusted"), "{output}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_grant_lets_the_peer_invoke_the_capability() {
        let root = scratch("grant");
        let peer = peer();
        run(&root, &["enroll", &peer]);

        let (code, output) = run(&root, &["grant", &peer, "builtin/read_file"]);

        assert_eq!(code, 0, "{output}");
        let store = PeerStore::load(&root.join("peers.json")).expect("the store");
        let node = NodeId::new(peer).expect("a node");
        let capability: CapabilityId = "builtin/read_file".parse().expect("a capability");
        assert!(store.authorize(&node, &capability, unix_ms()).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn revoke_withdraws_the_peer_and_its_grants() {
        let root = scratch("revoke");
        let peer = peer();
        run(&root, &["enroll", &peer]);
        run(&root, &["grant", &peer, "builtin/read_file"]);

        let (code, output) = run(&root, &["revoke", &peer]);

        assert_eq!(code, 0, "{output}");
        let store = PeerStore::load(&root.join("peers.json")).expect("the store");
        let node = NodeId::new(peer).expect("a node");
        assert_eq!(
            store.record(&node).map(|record| record.state),
            Some(TrustState::Revoked)
        );
        let capability: CapabilityId = "builtin/read_file".parse().expect("a capability");
        assert!(store.authorize(&node, &capability, unix_ms()).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_invalid_node_id_is_refused() {
        let root = scratch("invalid");

        let (code, output) = run(&root, &["enroll", "not-a-key"]);

        assert_eq!(code, 1, "{output}");
        assert!(output.contains("base64"), "{output}");
        assert!(!root.join("peers.json").is_file(), "nothing was written");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unknown_command_is_refused() {
        let root = scratch("unknown");

        let (code, output) = run(&root, &["dance"]);

        assert_eq!(code, 1, "{output}");
        assert!(output.contains("unknown command"), "{output}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
