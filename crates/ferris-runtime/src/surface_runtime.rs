//! The cordis-rs surface-provider runtime.
//!
//! This is the runtime adapter for surfaces, and it lives beside the other
//! runtime adapters, not in the contract crate. Cordis is the execution host
//! for surface providers: a provider is admitted as a Cordis Plugin, its typed
//! contract is prepared before admission, its registration is owned by the
//! apply generation, and its teardown runs through Fiber disposal. The surface
//! engine in ferris-core stays the sync state machine for identity, generation
//! and revision; it is not a second lifecycle authority.
//!
//! This is a different "Cordis" from [crate::cordis]: that adapter runs a
//! JavaScript Cordis package over the Node bridge, while this one hosts Rust
//! surface providers on cordis-core. Cordis types do not appear in this
//! module's public API, so a host can swap the runtime without changing a
//! contract or a document.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use cordis_core::lifecycle::FiberHandle;
use cordis_core::{Context, Plugin, PreparedPlugin};

use ferris_core::surface::{
    Generation, InstanceId, Revision, SurfaceCatalog, SurfaceDisposalReport, SurfaceDocument,
    SurfaceEngine, SurfaceError, SurfaceFrame, SurfaceId, SurfaceInstance, SurfaceLimits,
    SurfaceProvider, SurfaceRequest,
};
use ferris_core::surface::engine::InstanceState;

/// A provider installed in the Cordis runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSurface {
    /// The surface instance the engine admitted.
    pub instance: InstanceId,
    /// The provider-qualified surface.
    pub surface: SurfaceId,
    /// The first generation.
    pub generation: Generation,
}

/// The surface providers and instances hosted by one cordis-rs runtime.
pub struct SurfaceProviderRuntime {
    context: Context,
    engine: SurfaceEngine,
    registry: Arc<Mutex<BTreeMap<SurfaceId, Arc<dyn SurfaceProvider>>>>,
    fibers: BTreeMap<InstanceId, FiberHandle>,
    limits: SurfaceLimits,
}

impl SurfaceProviderRuntime {
    /// Open a runtime with the default surface limits.
    pub fn new() -> Self {
        Self::with_limits(SurfaceLimits::default())
    }

    /// Open a runtime with explicit limits.
    pub fn with_limits(limits: SurfaceLimits) -> Self {
        Self {
            context: Context::new(),
            engine: SurfaceEngine::new(),
            registry: Arc::new(Mutex::new(BTreeMap::new())),
            fibers: BTreeMap::new(),
            limits,
        }
    }

    /// The engine that owns identity, generations and revisions.
    pub fn engine(&self) -> &SurfaceEngine {
        &self.engine
    }

    /// The engine, mutably, for a host that applies frames itself.
    pub fn engine_mut(&mut self) -> &mut SurfaceEngine {
        &mut self.engine
    }

    /// The limits this runtime enforces.
    pub fn limits(&self) -> &SurfaceLimits {
        &self.limits
    }

    /// Admit a provider as a Cordis Plugin and return its installation.
    ///
    /// The contract is validated during prepare, before Cordis creates any
    /// lifecycle state. A provider for a surface that is already installed in
    /// this runtime is refused: one runtime hosts one provider per surface, and
    /// a second provider is a composition decision, not an overwrite.
    pub async fn install(
        &mut self,
        provider: Arc<dyn SurfaceProvider>,
    ) -> Result<InstalledSurface, SurfaceError> {
        let contract = provider.contract().clone();
        contract.ensure_valid()?;
        let surface = contract.surface.clone();
        if self
            .registry
            .lock()
            .expect("registry lock")
            .contains_key(&surface)
        {
            return Err(SurfaceError::Invalid(format!(
                "surface {surface} is already installed in this runtime"
            )));
        }
        let instance = self.engine.admit(contract)?;
        let input = ProviderInput {
            provider,
            surface: surface.clone(),
            registry: Arc::clone(&self.registry),
        };
        let prepared = PreparedPlugin::from_input(SurfaceProviderPlugin, input);
        let fiber =
            self.context.spawn(prepared).await.map_err(|error| {
                SurfaceError::Invalid(format!("cordis admission failed: {error}"))
            })?;
        self.fibers.insert(instance, fiber);
        Ok(InstalledSurface {
            instance,
            surface,
            generation: Generation::initial(),
        })
    }

    /// Dispose one provider's Fiber and the instance it owns.
    pub async fn uninstall(
        &mut self,
        instance: InstanceId,
    ) -> Result<SurfaceDisposalReport, SurfaceError> {
        if let Some(fiber) = self.fibers.remove(&instance) {
            fiber.dispose().await.map_err(|error| {
                SurfaceError::Invalid(format!("cordis disposal failed: {error}"))
            })?;
        }
        self.engine.dispose(instance)
    }

    /// Dispose every installed provider.
    ///
    /// Dropping a Fiber handle does not dispose its Fiber, so a host that
    /// closes a workspace must call this rather than dropping the runtime.
    pub async fn shutdown(&mut self) -> Vec<Result<SurfaceDisposalReport, SurfaceError>> {
        let instances: Vec<InstanceId> = self.fibers.keys().copied().collect();
        let mut reports = Vec::with_capacity(instances.len());
        for instance in instances {
            reports.push(self.uninstall(instance).await);
        }
        reports
    }

    /// The catalogue of installed contracts, for slot resolution.
    pub fn catalog(&self) -> SurfaceCatalog {
        let mut catalog = SurfaceCatalog::new();
        for provider in self.registry.lock().expect("registry lock").values() {
            let _ = catalog.register(provider.contract().clone());
        }
        catalog
    }

    /// Ask an installed provider for a frame and validate it, without applying
    /// it to the engine.
    pub fn frame(
        &self,
        instance: InstanceId,
        props: serde_json::Value,
    ) -> Result<SurfaceDocument, SurfaceError> {
        let instance_state = self.engine.instance(instance)?;
        if instance_state.state() == InstanceState::Disposed {
            return Err(SurfaceError::Disposed);
        }
        let contract = instance_state.contract();
        let provider = self.provider(&contract.surface)?;
        let request = SurfaceRequest {
            instance,
            generation: instance_state.generation(),
            revision: instance_state.revision(),
            props,
        };
        let document = provider.frame(&request)?;
        let report = document.validate_against(contract, &self.limits);
        if !report.is_valid() {
            return Err(SurfaceError::Invalid(
                report
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        Ok(document)
    }

    /// Read the provider once and apply the result as the next full frame.
    pub fn refresh(
        &mut self,
        instance: InstanceId,
        props: serde_json::Value,
    ) -> Result<Revision, SurfaceError> {
        let current = self.engine.instance(instance)?;
        let generation = current.generation();
        let next = current.revision().next();
        let document = self.frame(instance, props)?;
        let frame = SurfaceFrame::full(instance, generation, next, document);
        self.engine.apply_frame(&frame, &self.limits)
    }

    /// The instance states, for a diagnostics pane.
    pub fn instances(&self) -> impl Iterator<Item = &SurfaceInstance> {
        self.engine.instances()
    }

    fn provider(&self, surface: &SurfaceId) -> Result<Arc<dyn SurfaceProvider>, SurfaceError> {
        self.registry
            .lock()
            .expect("registry lock")
            .get(surface)
            .cloned()
            .ok_or_else(|| SurfaceError::NoProvider(surface.to_string()))
    }
}

impl Default for SurfaceProviderRuntime {
    fn default() -> Self {
        Self::new()
    }
}

struct ProviderInput {
    provider: Arc<dyn SurfaceProvider>,
    surface: SurfaceId,
    registry: Arc<Mutex<BTreeMap<SurfaceId, Arc<dyn SurfaceProvider>>>>,
}

/// The Cordis plugin that admits one surface provider.
struct SurfaceProviderPlugin;

impl Plugin for SurfaceProviderPlugin {
    type Config = ProviderInput;
    type Input = ProviderInput;
    type PrepareError = SurfaceError;
    type ApplyError = SurfaceError;

    fn name(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed("ferris.surface.provider")
    }

    fn prepare(&self, input: ProviderInput) -> Result<ProviderInput, SurfaceError> {
        input.provider.contract().ensure_valid()?;
        Ok(input)
    }

    async fn apply(&self, ctx: Context, input: &ProviderInput) -> Result<(), SurfaceError> {
        let surface = input.surface.clone();
        let registry = Arc::clone(&input.registry);
        {
            let mut guard = registry.lock().expect("registry lock");
            guard.insert(surface.clone(), Arc::clone(&input.provider));
        }
        let cleanup_registry = Arc::clone(&registry);
        let cleanup_surface = surface.clone();
        let registration = ctx.effect_sync(move || {
            cleanup_registry
                .lock()
                .expect("registry lock")
                .remove(&cleanup_surface);
        });
        if let Err(error) = registration {
            registry.lock().expect("registry lock").remove(&surface);
            return Err(SurfaceError::Invalid(format!(
                "the surface registration is not generation-owned: {error}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_core::surface::contract::{ActionSpec, Confirmation, Profile, SurfaceContract};
    use ferris_core::surface::document::{
        ActionBinding, ActionMode, DispatchPrincipal, SurfaceDocument, SURFACE_SCHEMA_VERSION,
    };
    use ferris_core::surface::node::SurfaceNode;
    use ferris_core::Risk;

    struct StatusProvider {
        contract: SurfaceContract,
    }

    impl StatusProvider {
        fn new() -> Self {
            let contract = SurfaceContract::new(
                SurfaceId::parse("acme.git/status").expect("id"),
                semver::Version::new(1, 2, 3),
            )
            .with_profile(Profile::Text)
            .with_action(ActionSpec {
                id: "refresh".to_string(),
                label: "Refresh".to_string(),
                capability: "acme.git/refresh".to_string(),
                risk: Risk::Low,
                confirmation: Confirmation::Inherit,
            });
            Self { contract }
        }
    }

    impl SurfaceProvider for StatusProvider {
        fn contract(&self) -> &SurfaceContract {
            &self.contract
        }

        fn frame(&self, request: &SurfaceRequest) -> Result<SurfaceDocument, SurfaceError> {
            let mut bindings = BTreeMap::new();
            bindings.insert(
                "refresh".to_string(),
                ActionBinding {
                    capability: "acme.git/refresh".to_string(),
                    operation: None,
                    input: serde_json::json!({"repo": request.props}),
                    principal: DispatchPrincipal::Producer,
                    required: true,
                    mode: ActionMode::Once,
                    debounce_ms: 0,
                },
            );
            Ok(SurfaceDocument {
                schema_version: SURFACE_SCHEMA_VERSION.to_string(),
                bindings,
                hidden_actions: Vec::new(),
                root: SurfaceNode::Column {
                    id: None,
                    children: vec![SurfaceNode::Text {
                        id: None,
                        text: format!("revision {}", request.revision.value()),
                        tone: crate::ui::Tone::Neutral,
                    }],
                },
            })
        }
    }

    #[tokio::test]
    async fn a_provider_is_admitted_by_cordis_and_framed() {
        let mut runtime = SurfaceProviderRuntime::new();
        let installed = runtime
            .install(Arc::new(StatusProvider::new()))
            .await
            .expect("install");
        assert_eq!(installed.surface.to_string(), "acme.git/status");
        let revision = runtime
            .refresh(installed.instance, serde_json::json!({"repo": "/work"}))
            .expect("refresh");
        assert_eq!(revision, Revision::initial());
        let instance = runtime
            .engine()
            .instance(installed.instance)
            .expect("instance");
        assert_eq!(instance.revision(), Revision::initial());
        assert!(instance.document().is_some());
    }

    #[tokio::test]
    async fn uninstall_disposes_the_fiber_and_the_instance() {
        let mut runtime = SurfaceProviderRuntime::new();
        let installed = runtime
            .install(Arc::new(StatusProvider::new()))
            .await
            .expect("install");
        runtime
            .refresh(installed.instance, serde_json::Value::Null)
            .expect("refresh");
        let report = runtime
            .uninstall(installed.instance)
            .await
            .expect("uninstall");
        assert!(!report.already_disposed);
        assert!(runtime.catalog().contracts().is_empty());
        assert!(matches!(
            runtime.refresh(installed.instance, serde_json::Value::Null),
            Err(SurfaceError::Disposed)
        ));
        let again = runtime
            .uninstall(installed.instance)
            .await
            .expect("uninstall again");
        assert!(again.already_disposed);
    }

    #[tokio::test]
    async fn a_second_provider_for_one_surface_is_refused() {
        let mut runtime = SurfaceProviderRuntime::new();
        runtime
            .install(Arc::new(StatusProvider::new()))
            .await
            .expect("install");
        let error = runtime
            .install(Arc::new(StatusProvider::new()))
            .await
            .expect_err("a duplicate provider is refused");
        assert!(error.to_string().contains("already installed"));
    }
}
