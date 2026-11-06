//! Assembly is deterministic, policy-gated, and activates only when asked.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use capability::assembly::{AssemblyEngine, AssemblyError, AssemblyRequest};
use capability::composition::{
    Binding, Budgets, CapabilityRequirement, GrantRef, PeerCatalogue, PeerRef, RemotePolicy,
};
use capability::descriptor::{CapabilityDescriptor, OutputContract};
use capability::id::{
    CapabilityId, CapabilityKind, NodeId, ProviderId, SurfaceName, VersionSource, WorkspaceId,
};
use capability::proposal::ProposalPolicy;
use capability::provider::{
    CapabilityCall, CapabilityOutput, CapabilityProvider, ProviderHealth, ProviderKind,
};
use capability::{CapabilityError, Catalogue, Risk};
use semver::{Version, VersionReq};
use serde_json::json;

fn cap(value: &str) -> CapabilityId {
    value.parse().expect("a capability id")
}

fn node(value: &str) -> NodeId {
    NodeId::new(value).expect("a node id")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:assembly").expect("a workspace")
}

fn descriptor(value: &str, version: &str, risk: Risk) -> CapabilityDescriptor {
    let id = cap(value);
    let surface = SurfaceName::new(id.name().to_string());
    CapabilityDescriptor::new(
        id,
        CapabilityKind::Tool,
        Version::parse(version).expect("a version"),
        VersionSource::Declared,
    )
    .with_display_name(value)
    .with_description("a test capability")
    .with_input_schema(json!({ "type": "object" }))
    .with_output(OutputContract::json())
    .with_surface(surface)
    .with_risk(risk)
}

fn catalogue(entries: &[(&str, &str, Risk)]) -> Catalogue {
    let mut catalogue = Catalogue::new();
    for (id, version, risk) in entries {
        catalogue
            .insert(descriptor(id, version, *risk))
            .expect("a unique descriptor");
    }
    catalogue
}

fn requirement(value: &str, range: &str) -> CapabilityRequirement {
    CapabilityRequirement::new(cap(value), VersionReq::parse(range).expect("a range"))
}

fn request(requirements: Vec<CapabilityRequirement>, peers: Vec<PeerRef>) -> AssemblyRequest {
    let mut request = AssemblyRequest::new("compose a build", workspace());
    request.requirements = requirements;
    request.peers = peers;
    request.budgets = Budgets::default();
    request
}

fn policy(allowed: &[&str], max_risk: Risk, allow_remote: bool) -> ProposalPolicy<'static> {
    let allowed: BTreeSet<CapabilityId> = allowed.iter().map(|id| cap(id)).collect();
    let allowed = Box::leak(Box::new(allowed));
    ProposalPolicy {
        allowed,
        max_risk,
        allow_remote,
    }
}

fn trusted_build_peer() -> PeerCatalogue {
    PeerCatalogue::new(
        node("node-b"),
        catalogue(&[("builtin/build", "1.4.0", Risk::Low)]),
    )
}

fn build_peer() -> PeerRef {
    PeerRef::new(node("node-b")).with_grant(GrantRef::new("grant:b"))
}

#[test]
fn selection_is_deterministic_and_explains_every_binding() {
    let local = catalogue(&[("builtin/read_file", "1.0.0", Risk::Low)]);
    let peers = [trusted_build_peer()];
    let engine = AssemblyEngine::new(&local, &peers, [node("node-b")]);
    let request = request(
        vec![
            requirement("builtin/read_file", "^1"),
            requirement("builtin/build", "^1").with_remote(RemotePolicy::Allowed),
        ],
        vec![build_peer()],
    );
    let policy = policy(&["builtin/read_file", "builtin/build"], Risk::Medium, true);

    let first = engine.assemble(&request, &policy).expect("assembled");
    let second = engine.assemble(&request, &policy).expect("assembled again");

    assert_eq!(first, second, "same inputs and policy are deterministic");
    assert_eq!(first.fingerprint, second.fingerprint);
    assert!(!first.rationale.is_empty());
    assert_eq!(first.selected().len(), 2);
    assert!(first
        .selected()
        .iter()
        .all(|candidate| candidate.binding.is_some()));
    let remote = first
        .selected()
        .into_iter()
        .find(|candidate| candidate.requirement == cap("builtin/build"))
        .expect("the remote selection");
    assert!(matches!(remote.binding, Some(Binding::Remote { .. })));

    let plan = engine.approve(&first, &policy).expect("approved");
    assert_eq!(plan.lock.bindings.len(), 2);
    assert_eq!(plan.lock.fingerprint, plan.lock.recompute_fingerprint());
    assert_eq!(plan.resolved.workspace, workspace());
}

#[test]
fn approval_rejects_a_changed_policy_or_catalogue() {
    let local = catalogue(&[("builtin/read_file", "1.0.0", Risk::Low)]);
    let engine = AssemblyEngine::new(&local, &[], []);
    let request = request(vec![requirement("builtin/read_file", "^1")], Vec::new());
    let first = engine
        .assemble(
            &request,
            &policy(&["builtin/read_file"], Risk::Medium, false),
        )
        .expect("assembled");

    assert_eq!(
        engine
            .approve(&first, &policy(&["builtin/read_file"], Risk::High, false))
            .expect_err("the ceiling changed"),
        AssemblyError::PolicyChanged
    );

    let changed = catalogue(&[("builtin/read_file", "1.1.0", Risk::Low)]);
    let changed_engine = AssemblyEngine::new(&changed, &[], []);
    assert_eq!(
        changed_engine
            .approve(&first, &policy(&["builtin/read_file"], Risk::Medium, false))
            .expect_err("the catalogue changed"),
        AssemblyError::CatalogueChanged
    );
}

#[test]
fn an_adversarial_model_request_cannot_widen_authority() {
    let local = catalogue(&[("builtin/read_file", "1.0.0", Risk::Low)]);
    let peers = [trusted_build_peer()];
    let allowed = policy(&["builtin/read_file"], Risk::Medium, true);
    let engine = AssemblyEngine::new(&local, &peers, [node("node-b")]);

    let unapproved = request(vec![requirement("builtin/build", "^1")], vec![build_peer()]);
    assert!(matches!(
        engine.assemble(&unapproved, &allowed),
        Err(AssemblyError::Policy(_))
    ));

    let untrusted = AssemblyEngine::new(&local, &peers, []);
    let remote = request(
        vec![requirement("builtin/read_file", "^1")],
        vec![build_peer()],
    );
    assert!(matches!(
        untrusted.assemble(&remote, &allowed),
        Err(AssemblyError::UntrustedPeer { .. })
    ));

    let risky_local = catalogue(&[("builtin/read_file", "1.0.0", Risk::High)]);
    let risky_engine = AssemblyEngine::new(&risky_local, &[], []);
    let risky = request(vec![requirement("builtin/read_file", "^1")], Vec::new());
    assert!(matches!(
        risky_engine.assemble(&risky, &policy(&["builtin/read_file"], Risk::Medium, true)),
        Err(AssemblyError::Policy(_))
    ));

    let duplicate = request(
        vec![
            requirement("builtin/read_file", "^1"),
            requirement("builtin/read_file", "^1"),
        ],
        Vec::new(),
    );
    assert!(matches!(
        engine.assemble(&duplicate, &allowed),
        Err(AssemblyError::Duplicate { .. })
    ));
}

#[test]
fn an_unavailable_required_peer_fails_but_an_optional_one_is_skipped() {
    let local = catalogue(&[("builtin/read_file", "1.0.0", Risk::Low)]);
    let engine = AssemblyEngine::new(&local, &[], [node("node-b")]);
    let policy = policy(&["builtin/build"], Risk::Medium, true);

    let required = request(
        vec![requirement("builtin/build", "^1").with_remote(RemotePolicy::Required(node("node-b")))],
        vec![build_peer()],
    );
    assert!(matches!(
        engine.assemble(&required, &policy),
        Err(AssemblyError::Policy(
            capability::proposal::ProposalError::Unknown { .. }
        )) | Err(AssemblyError::Unresolvable(_))
    ));

    let optional = request(
        vec![requirement("builtin/build", "^1")
            .with_remote(RemotePolicy::Allowed)
            .with_required(false)],
        vec![build_peer()],
    );
    let proposal = engine.assemble(&optional, &policy).expect("optional skips");
    assert!(proposal.selected().is_empty());
    assert!(proposal
        .rationale
        .iter()
        .any(|line| line.contains("no provider")));
}

struct CountingProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    fail_activate: bool,
}

#[async_trait]
impl CapabilityProvider for CountingProvider {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn activate(&self) -> Result<(), CapabilityError> {
        if self.fail_activate {
            return Err(CapabilityError::ProviderFailed {
                message: "cannot activate".to_string(),
            });
        }
        Ok(())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(CapabilityOutput::text("ok"))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

#[tokio::test]
async fn approval_does_not_activate_and_activation_is_a_separate_call() {
    let local = catalogue(&[("builtin/read_file", "1.0.0", Risk::Low)]);
    let engine = AssemblyEngine::new(&local, &[], []);
    let request = request(vec![requirement("builtin/read_file", "^1")], Vec::new());
    let policy = policy(&["builtin/read_file"], Risk::Medium, false);
    let proposal = engine.assemble(&request, &policy).expect("assembled");
    let plan = engine.approve(&proposal, &policy).expect("approved");

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
        fail_activate: false,
    });
    let lifecycle = plan
        .activate(&[provider as Arc<dyn CapabilityProvider>])
        .await
        .expect("active");
    assert_eq!(lifecycle.state(), capability::WorkspaceState::Active);
    let id = cap("builtin/read_file");
    lifecycle
        .invoke(
            &id,
            json!({}),
            &capability::InvocationContext::local(
                capability::InvocationId::new("inv:assembly"),
                capability::Principal::Local {
                    session: "s".to_string(),
                    preset: "p".to_string(),
                },
                workspace(),
            ),
        )
        .await
        .expect("the active workspace calls");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    lifecycle
        .shutdown(std::time::Duration::from_millis(100))
        .await
        .expect("stopped");

    let failing = Arc::new(CountingProvider {
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        fail_activate: true,
    });
    let error = plan
        .activate(&[failing as Arc<dyn CapabilityProvider>])
        .await
        .expect_err("activation fails");
    assert!(
        matches!(error, AssemblyError::Activation { .. }),
        "{error:?}"
    );
}

#[test]
fn remote_requests_are_refused_when_the_policy_forbids_remote() {
    let local = catalogue(&[("builtin/read_file", "1.0.0", Risk::Low)]);
    let peers = [trusted_build_peer()];
    let engine = AssemblyEngine::new(&local, &peers, [node("node-b")]);
    let request = request(
        vec![requirement("builtin/read_file", "^1").with_remote(RemotePolicy::Allowed)],
        Vec::new(),
    );
    assert!(matches!(
        engine.assemble(
            &request,
            &policy(&["builtin/read_file"], Risk::Medium, false)
        ),
        Err(AssemblyError::Policy(_))
    ));
}
