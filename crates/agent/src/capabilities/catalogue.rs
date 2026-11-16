//! Lowering registered tools into the capability catalogue.
//!
//! This is the seam between the engine's dispatch registry and the capability
//! model: it reads names, descriptions, schemas, flags, and any declared
//! metadata from the tools that are actually registered, so a catalogue cannot
//! describe a tool the registry does not have.
//!
//! A tool that declares metadata is trusted for exactly what it declares: the
//! declaration is validated (an unknown risk class is an error, not a silent
//! default), but it is not independently verified against the body's behavior.
//! A tool that declares nothing gets conservative defaults and an Unknown
//! version source, which is what makes the gap visible.

use ::capability::{
    CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind, Catalogue, FeatureSet,
    InvocationLimits, OutputContract, Permission, ProviderId, Risk, SideEffect, SurfaceName,
    VersionSource, Visibility,
};
use react::tool::ToolMetadata;
use semver::Version;

use crate::tools::ToolRegistry;

/// The contract version a built-in tool is described at until it declares one.
///
/// The version source is Derived, so a range check against it is known to be
/// advisory rather than a promise.
pub fn builtin_contract_version() -> Version {
    Version::new(0, 1, 0)
}

/// The provider every built-in tool belongs to.
pub fn builtin_provider() -> ProviderId {
    ProviderId::builtin()
}

/// Describe every registered tool as one capability catalogue.
///
/// Both the synchronous and the asynchronous registry are walked. A tool whose
/// name cannot form a capability id is an error rather than a silently dropped
/// entry, because a dropped entry is a capability the model is told exists and
/// the catalogue does not know about.
pub fn catalogue_from_registry(registry: &ToolRegistry) -> Result<Catalogue, CapabilityError> {
    let mut catalogue = Catalogue::new();
    for name in registry.list() {
        if let Some(tool) = registry.get(&name) {
            catalogue.insert(describe(
                &name,
                tool.description(),
                tool.json_schema(),
                tool.is_cancelable(),
                false,
                tool.metadata(),
            )?)?;
        } else if let Some(tool) = registry.get_async(&name) {
            catalogue.insert(describe(
                &name,
                tool.description(),
                tool.json_schema(),
                tool.is_cancelable(),
                tool.supports_streaming(),
                tool.metadata(),
            )?)?;
        }
    }
    Ok(catalogue)
}

#[allow(clippy::too_many_arguments)]
fn describe(
    name: &str,
    description: String,
    schema: serde_json::Value,
    cancelable: bool,
    streaming: bool,
    metadata: Option<&ToolMetadata>,
) -> Result<CapabilityDescriptor, CapabilityError> {
    let id =
        CapabilityId::new(builtin_provider(), name).map_err(|error| CapabilityError::Invalid {
            message: error.to_string(),
        })?;

    // A declaration that names no version is Unknown, not Derived: an MCP tool
    // has no version of its own, and a range check against a made-up one is a
    // promise the provider never made. A tool with no declaration at all is
    // Derived, because the host read it from the tool the model calls.
    let (version, version_source) = match metadata {
        Some(meta) => match meta.version.as_deref() {
            Some(raw) => {
                let parsed = Version::parse(raw).map_err(|error| CapabilityError::Invalid {
                    message: format!("tool {name} declares version {raw:?}: {error}"),
                })?;
                (parsed, VersionSource::Declared)
            }
            None => (builtin_contract_version(), VersionSource::Unknown),
        },
        None => (builtin_contract_version(), VersionSource::Derived),
    };

    let mut risk = Risk::Medium;
    let mut side_effects = Vec::new();
    let mut permissions = Vec::new();
    let mut limits = InvocationLimits::default();
    if let Some(meta) = metadata {
        if let Some(raw) = meta.risk.as_deref() {
            risk = Risk::parse(raw).ok_or_else(|| CapabilityError::Invalid {
                message: format!("tool {name} declares unknown risk class {raw:?}"),
            })?;
        }
        for raw in &meta.side_effects {
            side_effects.push(
                SideEffect::parse(raw).ok_or_else(|| CapabilityError::Invalid {
                    message: format!("tool {name} declares unknown side effect {raw:?}"),
                })?,
            );
        }
        permissions = meta
            .permissions
            .iter()
            .cloned()
            .map(Permission::new)
            .collect();
        limits.timeout_ms = meta.timeout_ms;
        limits.max_output_bytes = meta.max_output_bytes;
    }

    Ok(
        CapabilityDescriptor::new(id, CapabilityKind::Tool, version, version_source)
            .with_display_name(name)
            .with_description(description)
            .with_input_schema(schema)
            .with_output(OutputContract::json())
            .with_permissions(permissions)
            .with_side_effects(side_effects)
            .with_risk(risk)
            .with_limits(limits)
            .with_supports(FeatureSet {
                streaming,
                cancellation: cancelable,
                progress: streaming,
            })
            .with_surface(SurfaceName::new(name))
            .with_visibility(Visibility::Granted),
    )
}
