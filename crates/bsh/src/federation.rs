//! The federation management CLI and the files a node keeps.
//!
//! A node's identity and its peer grants are files under bsh's own directory,
//! so the same commands work headless: status, enroll, revoke, grant, and
//! revoke-grant. Nothing here starts a runtime; it manages the state a runtime
//! loads.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use agent::capability::{unix_ms, CapabilityId, NodeId};
use federation::identity::is_valid_node_id;
use federation::{
    DelegationState, DelegationStore, NodeIdentity, PeerStore, RotationRecord, TrustState,
};

/// The directory a federation node keeps its files in.
pub(crate) fn dir() -> PathBuf {
    crate::settings::dir().join("federation")
}

/// The profile name used when nothing names one.
pub(crate) const DEFAULT_PROFILE: &str = "default";

/// The launch-time profile override (`--federation-profile`).
///
/// A process-global because a launch has one node: the flag is read before the
/// window exists and the node's files are resolved from it later, on the
/// federation thread.
static PROFILE: OnceLock<String> = OnceLock::new();

/// Set the profile this process runs; later calls are ignored.
pub(crate) fn set_profile(name: &str) {
    let _ = PROFILE.set(name.trim().to_string());
}

/// Whether a name can be a profile.
///
/// A profile becomes a directory under bsh's federation directory, so the rule
/// is a directory-name rule: no separators, no `.`/`..`, no hidden leading
/// dot, and a bounded length. Everything else is refused where the reader can
/// see it instead of being silently rewritten.
pub(crate) fn valid_profile(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// The profile this process runs: the launch flag, then the environment, then
/// the saved setting, then `default`.
///
/// Two app instances on one machine share `settings.toml`, so the flag and the
/// environment are what let the second instance run its own node without
/// editing the document the first one also reads.
pub(crate) fn resolve_profile(configured: Option<&str>) -> String {
    if let Some(name) = PROFILE.get() {
        return name.clone();
    }
    if let Ok(name) = std::env::var("BSH_FEDERATION_PROFILE") {
        if valid_profile(&name) {
            return name.trim().to_string();
        }
    }
    configured
        .filter(|name| valid_profile(name))
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|| DEFAULT_PROFILE.to_string())
}

/// The directory one profile's files live in.
///
/// The default profile keeps today's paths, so an existing node is not moved by
/// this feature; every other profile is a subdirectory of the same root and
/// therefore a node of its own - its own key, peer store, delegations and audit
/// log.
pub(crate) fn node_dir_for(profile: &str) -> PathBuf {
    node_dir_in(&dir(), profile)
}

/// The directory one profile's files live in under an explicit root.
///
/// The tests and the pairing command need the same layout without touching the
/// process's real data directory.
pub(crate) fn node_dir_in(root: &Path, profile: &str) -> PathBuf {
    if profile == DEFAULT_PROFILE {
        root.to_path_buf()
    } else {
        root.join(profile)
    }
}

/// The name a node shows to peers, tagged with its profile when it is not the
/// default: two nodes on one machine must be distinguishable in a peer list.
pub(crate) fn display_name(configured: Option<&str>, profile: &str) -> String {
    let base = resolve_node_name(configured);
    if profile == DEFAULT_PROFILE {
        base
    } else {
        format!("{base} ({profile})")
    }
}

/// Run this profile's node headlessly until the process is interrupted.
///
/// This is what makes several nodes on one machine a one-liner: no window per
/// node, no second settings file, and each node holds its own profile's locks.
/// The node's tasks live on the shared runtime, so this thread only has to keep
/// the process alive.
pub(crate) fn serve(settings: &crate::settings::Settings, profiles: &[String]) -> i32 {
    // A headless node must be diagnosable: the transport's own reasons (a taken
    // port, a refused endpoint) are written to the log, and without this they
    // go nowhere.
    let log_dir = crate::settings::dir().join("log");
    std::fs::create_dir_all(&log_dir).ok();
    logging::auto_init_tracing_in(log_dir, "bsh");
    if !settings.global.federation.enabled {
        eprintln!("federation is disabled: set [federation] enabled = true in settings.toml");
        return 1;
    }
    if profiles.is_empty() {
        eprintln!("serve needs at least one profile");
        return 2;
    }
    // One process, one node per profile: several heads, one supervisor.
    for (index, profile) in profiles.iter().enumerate() {
        if !crate::federation_node::start_profile(settings, profile, index == 0) {
            eprintln!(
                "the federation node did not start (profile {profile}); another process may already own profile {profile}'s files, or a TLS node is already running in this one"
            );
            return 1;
        }
    }
    let mut last: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (profile, runtime) in crate::federation_node::running_profiles() {
        println!(
            "[{profile}] {} identity {}",
            runtime.status(),
            node_dir_for(&profile).join("node.key").display()
        );
        last.insert(profile, runtime.status());
    }
    println!("stop with Ctrl-C");
    // Watch every node rather than simply sleeping: a peer that appears is the
    // one fact the reader came for, and it is otherwise invisible without a
    // window.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let running = crate::federation_node::running_profiles();
        if running.is_empty() {
            eprintln!("all federation nodes stopped");
            return 2;
        }
        for (profile, runtime) in running {
            let status = runtime.status();
            if last.get(&profile) != Some(&status) {
                println!("[{profile}] {status}");
                last.insert(profile, status);
            }
        }
    }
}

/// List the node profiles on this machine: one row per profile that has a key.
///
/// This is the "what is running here" answer for a host that runs several
/// nodes: the state column is the directory lock (running when another process
/// owns it), so the list is live rather than a stored flag.
pub(crate) fn profiles(root: &Path, name: Option<&str>, out: &mut dyn Write) -> i32 {
    let mut rows: Vec<(String, PathBuf)> = vec![(DEFAULT_PROFILE.to_string(), root.to_path_buf())];
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(profile) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if valid_profile(&profile) {
                rows.push((profile, path));
            }
        }
    }
    rows.sort_by(|left, right| left.0.cmp(&right.0));

    let _ = writeln!(out, "PROFILE\tSTATE\tPEERS\tNODE\tNAME");
    let mut shown = 0;
    for (profile, directory) in rows {
        let identity_path = directory.join("node.key");
        if !identity_path.is_file() {
            continue;
        }
        let Ok(identity) = NodeIdentity::load_or_create(&identity_path) else {
            continue;
        };
        let peers = PeerStore::load(&directory.join("peers.json"))
            .map(|store| store.iter().count())
            .unwrap_or(0);
        // The lock IS the liveness probe: a node holds it for its lifetime, so a
        // profile we can lock is not running anywhere.
        let state = match crate::federation_lock::DataDirLock::acquire(&directory) {
            Ok(lock) => {
                drop(lock);
                "idle"
            }
            Err(_) => "running",
        };
        let _ = writeln!(
            out,
            "{profile}\t{state}\t{peers}\t{}\t{}",
            identity.node_id(),
            display_name(name, &profile)
        );
        shown += 1;
    }
    if shown == 0 {
        let _ = writeln!(
            out,
            "no federation profiles under {} (the app creates one on first launch)",
            root.display()
        );
    }
    let _ = writeln!(out, "profiles {shown}");
    0
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

/// Enroll two local profiles with each other.
///
/// Linking is automatic (the transport finds the first node's port), but trust
/// is explicit by design: this is the one command that writes both peer
/// stores. `--grant CAP` records that the OTHER profile may invoke CAP on
/// THIS one; repeat it for several capabilities. The reverse direction is a
/// second run with the profiles swapped, because a grant is always something
/// one side gives, never something both sides obtain at once.
pub(crate) fn pair(
    root: &Path,
    profile: &str,
    other: &str,
    grants: &[String],
    out: &mut dyn Write,
) -> i32 {
    if !valid_profile(other) {
        return fail(out, &format!("'{other}' is not a valid profile name"));
    }
    if other == profile {
        return fail(out, "a profile cannot pair with itself");
    }
    let this_dir = node_dir_in(root, profile);
    let other_dir = node_dir_in(root, other);
    if !other_dir.join("node.key").is_file() {
        return fail(
            out,
            &format!(
                "profile '{other}' has no node yet; run 'bsh federation --profile {other} status' once"
            ),
        );
    }
    // Both peer stores are read-modify-write files, so take both directory
    // locks - in sorted order, so a refused pair can never deadlock.
    let mut dirs = vec![this_dir.clone(), other_dir.clone()];
    dirs.sort();
    let mut locks = Vec::new();
    for dir in &dirs {
        match crate::federation_lock::DataDirLock::acquire(dir) {
            Ok(lock) => locks.push(lock),
            Err(message) => return fail(out, &message),
        }
    }
    let this = match NodeIdentity::load_or_create(&this_dir.join("node.key")) {
        Ok(identity) => identity,
        Err(error) => return fail(out, &error.to_string()),
    };
    let peer = match NodeIdentity::load_or_create(&other_dir.join("node.key")) {
        Ok(identity) => identity,
        Err(error) => return fail(out, &error.to_string()),
    };
    let now = unix_ms();
    let mut this_store = PeerStore::load(&this_dir.join("peers.json")).unwrap_or_default();
    let mut other_store = PeerStore::load(&other_dir.join("peers.json")).unwrap_or_default();
    // Enrolling REPLACES a peer record, grants included, so an already-trusted
    // peer is left alone: pairing twice (once per direction) must not wipe the
    // grant the first call recorded.
    let peer_id = peer.node_id();
    if !this_store
        .iter()
        .any(|record| record.node == peer_id && record.state == TrustState::Trusted)
    {
        this_store.enroll(peer_id.clone(), now, Some(format!("profile {other}")));
    }
    let this_id = this.node_id();
    if !other_store
        .iter()
        .any(|record| record.node == this_id && record.state == TrustState::Trusted)
    {
        other_store.enroll(this_id.clone(), now, Some(format!("profile {profile}")));
    }

    let mut granted = Vec::new();
    for name in grants {
        let capability: CapabilityId = match name.parse() {
            Ok(capability) => capability,
            Err(error) => return fail(out, &format!("invalid capability id '{name}': {error}")),
        };
        if let Err(error) = this_store.grant(&peer_id, capability.clone(), now, None) {
            return fail(out, &error.to_string());
        }
        granted.push(capability.to_string());
    }
    if let Err(error) = this_store.save(&this_dir.join("peers.json")) {
        return fail(
            out,
            &format!("the {profile} peer store could not be written: {error}"),
        );
    }
    if let Err(error) = other_store.save(&other_dir.join("peers.json")) {
        return fail(
            out,
            &format!("the {other} peer store could not be written: {error}"),
        );
    }
    let _ = writeln!(
        out,
        "paired {} ({profile}) <-> {} ({other})",
        this.node_id(),
        peer.node_id()
    );
    for capability in granted {
        let _ = writeln!(out, "  grant: {other} may invoke {capability} on {profile}");
    }
    let _ = writeln!(out, "both directions enrolled; start both nodes to use it");
    0
}

const USAGE: &str = "\
bsh federation [--profile NAME] <command>

  (no command) / help                     this text
  serve                                   run this profile's node in the
                                          foreground, without a window; repeat
                                          --profile to run several nodes in
                                          this one process (Ctrl-C stops all)
  pair <profile> [--grant <capability>]...
                                          enroll this profile and another local
                                          profile with each other; --grant lets
                                          the OTHER profile invoke the
                                          capability on THIS one
  profiles                                every node profile on this machine:
                                          state, peers, node id and name
  status                                  show this node's id, its peers, and
                                          the last decisions
  enroll <node-id>                        trust a peer
  revoke <node-id>                        revoke a peer and its grants
  grant <node-id> <capability-id>         let a peer invoke one capability
  revoke-grant <node-id> <capability-id>  withdraw one grant
  rotate [reason]                         replace this node's key and write a
                                          signed rotation record for peers
  accept-rotation <record.json>           apply a peer's signed key rotation

  --profile NAME                          act on that profile's node instead of
                                          the default one (repeatable for serve)

Identity and peers live in bsh's own directory, one subdirectory per profile;
a peer id is a base64 Ed25519 public key, which is what a peer prints as its
node id.

Several nodes on one machine: give each one its own profile. The app runs the
default profile unless launched with --federation-profile=NAME or
BSH_FEDERATION_PROFILE=NAME; bsh federation --profile NAME ... manages that
node's files while the app is closed.";

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
                                TrustState::Invited => "invited",
                                TrustState::Pending => "pending",
                                TrustState::Trusted => "trusted",
                                TrustState::Suspended => "suspended",
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

    /// A profile becomes a directory, so the rule is a directory-name rule.
    #[test]
    fn a_profile_is_a_directory_name() {
        for good in ["default", "work", "second-2", "a.b_c"] {
            assert!(valid_profile(good), "{good}");
        }
        for bad in ["", " ", ".", "..", ".hidden", "a/b", "a\\b"] {
            assert!(!valid_profile(bad), "{bad:?}");
        }
        assert!(!valid_profile(&"x".repeat(65)), "bounded");
    }

    /// The default profile keeps today's paths; a named one is its own node.
    #[test]
    fn the_default_profile_keeps_the_legacy_layout() {
        assert_eq!(node_dir_for(DEFAULT_PROFILE), dir());
        assert_eq!(node_dir_for("work"), dir().join("work"));
        assert_ne!(node_dir_for("one"), node_dir_for("two"));
    }

    /// Two nodes on one machine must be distinguishable in a peer list.
    #[test]
    fn a_named_profile_is_visible_in_the_node_name() {
        assert_eq!(display_name(Some("Mac"), DEFAULT_PROFILE), "Mac");
        assert_eq!(display_name(Some("Mac"), "work"), "Mac (work)");
    }

    /// The roster lists every node on the machine with its live state.
    #[test]
    fn profiles_lists_each_nodes_state_and_id() {
        let root = scratch("profiles");
        let first = NodeIdentity::load_or_create(&root.join("node.key")).expect("default node");
        let work = root.join("work");
        std::fs::create_dir_all(&work).expect("profile dir");
        let second = NodeIdentity::load_or_create(&work.join("node.key")).expect("work node");
        assert_ne!(
            first.node_id(),
            second.node_id(),
            "each profile is a node of its own"
        );

        let mut out = Vec::new();
        assert_eq!(profiles(&root, Some("Mac"), &mut out), 0);
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("default	"), "{text}");
        assert!(text.contains("work	"), "{text}");
        assert!(text.contains("profiles 2"), "{text}");
        assert!(text.contains("Mac (work)"), "{text}");
    }

    /// Pairing writes both peer stores: each side trusts the other, and a grant
    /// is recorded on the side that GIVES it.
    #[test]
    fn pair_enrolls_both_profiles_and_grants_one_direction() {
        let root = scratch("pair");
        let work = root.join("work");
        std::fs::create_dir_all(&work).expect("profile dir");
        let this = NodeIdentity::load_or_create(&root.join("node.key")).expect("this node");
        let peer = NodeIdentity::load_or_create(&work.join("node.key")).expect("peer node");

        let mut out = Vec::new();
        let code = pair(
            &root,
            DEFAULT_PROFILE,
            "work",
            &["builtin/read_file".to_string()],
            &mut out,
        );
        let text = String::from_utf8(out).expect("utf8");
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("paired"), "{text}");

        let mine = PeerStore::load(&root.join("peers.json")).expect("my store");
        let record = mine
            .iter()
            .find(|record| record.node == peer.node_id())
            .expect("the peer is enrolled here");
        assert_eq!(record.state, TrustState::Trusted);
        assert!(
            record
                .grants
                .iter()
                .any(|grant| grant.capability.to_string() == "builtin/read_file"),
            "the grant is recorded on the giving side"
        );

        // Pairing again, in the other direction, must not wipe the grant the
        // first call recorded: enrolling a peer replaces its record.
        let mut again = Vec::new();
        assert_eq!(
            pair(&root, "work", DEFAULT_PROFILE, &[], &mut again),
            0,
            "{}",
            String::from_utf8_lossy(&again)
        );
        let mine_again = PeerStore::load(&root.join("peers.json")).expect("my store");
        assert!(
            mine_again
                .iter()
                .find(|record| record.node == peer.node_id())
                .expect("the peer is still enrolled")
                .grants
                .iter()
                .any(|grant| grant.capability.to_string() == "builtin/read_file"),
            "the grant survives a second pair"
        );

        let theirs = PeerStore::load(&work.join("peers.json")).expect("their store");
        let back = theirs
            .iter()
            .find(|record| record.node == this.node_id())
            .expect("this node is enrolled there");
        assert_eq!(back.state, TrustState::Trusted);
        assert!(
            back.grants.is_empty(),
            "the other side obtained no grant out of it"
        );
    }

    /// Pairing a profile that does not exist says where to create it.
    #[test]
    fn pair_refuses_a_profile_without_a_node() {
        let root = scratch("pair-missing");
        let mut out = Vec::new();
        let code = pair(&root, DEFAULT_PROFILE, "work", &[], &mut out);
        assert_eq!(code, 1);
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("no node yet"), "{text}");
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
