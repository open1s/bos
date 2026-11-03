#![warn(missing_docs)]
//! Author-facing SDK for the Ferris extension framework.
//!
//! An extension implements Extension and builds its manifest with
//! ManifestBuilder. Contributions are typed by ContributionSpec, and
//! ExtensionRuntime adapts the author interface to the framework runtime
//! trait. There is no macro DSL: the manifest remains the single entry point.

pub mod contribution;
pub mod error;
pub mod manifest;
pub mod runtime;

// Host-side facade: BSH and other hosts consume Ferris through the SDK.
#[cfg(feature = "tokio")]
pub use ferris_core::manifest as core_manifest;
#[cfg(feature = "tokio")]
pub use ferris_core::{
    capability, composable, config, events, lifecycle, manager, manifest_v2, package, resolver,
    signing, sources, terminal, types, ui,
};
#[cfg(feature = "tokio")]
pub use ferris_core::{
    CapabilityRegistry, CapabilitySpec, ContributionLease, ContributionSink, HostSpec, LeaseHost,
    Risk, Runtime, RuntimeResolver, Scope, ScopePath, ServiceContractRegistry,
};
#[cfg(feature = "host-runtime")]
pub use ferris_runtime as host_runtime;

pub use ferris_core::manifest::{
    ConfigurationSpec, ResourcesSpec, ServiceProvideSpec, ServiceRequireSpec, ServiceScope,
    ServicesSpec,
};
pub use ferris_core::types::{ExtensionId, Requirement, StartupPolicy, TrustLevel};
pub use ferris_core::{Contribution, ContributionRef, InvokeContext, RuntimeContext};
pub use semver::Version;
pub use toml;

pub use contribution::{kinds, ContributionSpec};
pub use error::{ErrorCode, ExtError, Result};
pub use manifest::{ExtensionManifest, ManifestBuilder};
pub use runtime::{Call, Extension, ExtensionContext, ExtensionRuntime, TestHarness};

/// Common authoring imports.
pub mod prelude {
    pub use crate::contribution::{kinds, ContributionSpec};
    pub use crate::error::{ErrorCode, ExtError, Result};
    pub use crate::manifest::{ExtensionManifest, ManifestBuilder};
    pub use crate::runtime::{Call, Extension, ExtensionContext, ExtensionRuntime, TestHarness};
    pub use crate::{
        ConfigurationSpec, Contribution, ContributionRef, ExtensionId, InvokeContext, Requirement,
        ResourcesSpec, RuntimeContext, ServiceProvideSpec, ServiceRequireSpec, ServiceScope,
        ServicesSpec, StartupPolicy, TrustLevel, Version,
    };
    pub use async_trait::async_trait;
    pub use serde_json::{json, Value};
    pub use toml;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contribution::ContributionSpec;
    use serde_json::Value;

    struct Echo;

    #[async_trait::async_trait]
    impl Extension for Echo {
        fn manifest(&self) -> ExtensionManifest {
            ManifestBuilder::new("com.example.echo", "Echo", "1.0.0", "Echoes its input")
                .capability("agent.tools.register")
                .contribution(ContributionSpec::tool("echo"))
                .build()
                .expect("manifest")
        }

        async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>> {
            Ok(vec![ContributionSpec::tool("echo").into_contribution(
                serde_json::json!({"description": "Echo"}),
            )])
        }

        async fn invoke(&self, call: &Call, _ctx: &InvokeContext) -> Result<Value> {
            Ok(call.input.clone())
        }
    }

    #[tokio::test]
    async fn the_runtime_adapter_starts_and_invokes() {
        let root = std::path::PathBuf::from("/tmp/echo");
        let harness = TestHarness::new(Echo, root.clone()).expect("harness");
        let contributions = harness.start().await.expect("start");
        assert_eq!(contributions.len(), 1);
        let value = harness
            .invoke("echo", serde_json::json!({"ok": true}))
            .await
            .expect("invoke");
        assert_eq!(value, serde_json::json!({"ok": true}));
        harness.shutdown().await.expect("shutdown");
    }

    #[test]
    fn every_standard_contribution_kind_is_typed() {
        let specs = [
            ContributionSpec::tool("t"),
            ContributionSpec::command("c"),
            ContributionSpec::panel("p"),
            ContributionSpec::prompt("pr"),
            ContributionSpec::context("ctx"),
            ContributionSpec::skill("s"),
            ContributionSpec::workflow("w"),
            ContributionSpec::theme("th"),
            ContributionSpec::preset("ps"),
            ContributionSpec::model_provider("mp"),
            ContributionSpec::agent_provider("ap"),
            ContributionSpec::language("lang"),
            ContributionSpec::integration("int"),
            ContributionSpec::subscribe("event.one"),
            ContributionSpec::publish("event.two"),
            ContributionSpec::hook("model.request"),
        ];
        assert_eq!(specs.len(), 16);
        assert_eq!(specs[0].kind, kinds::TOOL);
        assert_eq!(specs[13].kind, kinds::EVENT_SUBSCRIBE);
        assert_eq!(specs[15].kind, kinds::HOOK);
    }

    #[test]
    fn the_manifest_builder_covers_resources_config_and_dependencies() {
        let manifest = ManifestBuilder::new("com.example.full", "Full", "1.2.3", "full")
            .dependency("com.example.base", "^1.0")
            .max_memory_mb(128)
            .max_execution_ms(5_000)
            .max_concurrent_calls(2)
            .max_output_bytes(65_536)
            .configuration_schema("schemas/config.schema.json")
            .configuration_defaults(toml::toml! { enabled = true }.into())
            .startup(StartupPolicy::OnStartup)
            .build()
            .expect("manifest");
        let text = manifest.to_toml();
        assert!(text.contains("com.example.base"));
        assert!(text.contains("max_memory_mb = 128"));
        assert!(text.contains("schemas/config.schema.json"));
        assert!(text.contains("on-startup"));
    }

    #[test]
    fn the_manifest_builder_renders_a_valid_2_0_manifest() {
        let manifest = ManifestBuilder::new("com.example.demo", "Demo", "1.0.0", "demo")
            .author("Ferris")
            .license("MIT")
            .host(">=3.0.0")
            .extension_api(">=1.0.0")
            .capability("agent.tools.register")
            .contribution(ContributionSpec::tool("echo"))
            .contribution(ContributionSpec::command("refresh"))
            .build()
            .expect("manifest");
        assert_eq!(manifest.id().as_str(), "com.example.demo");
        assert_eq!(manifest.runtime_kind(), "declarative");
        assert!(manifest.to_toml().contains("schema_version = \"2.0\""));
        assert!(manifest.to_toml().contains("[[contribution]]"));
    }
}
