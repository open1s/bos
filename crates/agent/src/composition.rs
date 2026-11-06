//! The model-facing composition producer.
//!
//! The capability model can validate a proposal, but something has to produce
//! one. This module is that producer: it renders the catalogue into a prompt,
//! parses the model's JSON into a `CompositionSpec`, and then takes the
//! approved path (parse, approve, resolve_approved). The model is never
//! trusted: unknown fields are errors rather than ignored, and a capability the
//! catalogue does not offer is refused before validation.

use capability::composition::{
    CapabilityRequirement, CompositionError, CompositionSpec, LocalScope, RemotePolicy,
    ResolvedComposition,
};
use capability::proposal::{approve, resolve_approved, ProposalError, ProposalPolicy};
use capability::{CapabilityId, Catalogue, NodeId};
use semver::{Version, VersionReq};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

/// Why a proposal could not become a plan.
#[derive(Debug, Error)]
pub enum ComposeError {
    /// The JSON does not have the proposal shape.
    #[error("the proposal is not valid JSON: {0}")]
    Json(String),
    /// A top-level or per-requirement field this producer does not accept.
    #[error("the proposal has an unknown field `{0}`")]
    UnknownField(String),
    /// The proposal asks for nothing.
    #[error("the proposal names no requirements")]
    Empty,
    /// A capability id that does not parse.
    #[error("invalid capability id `{value}`: {message}")]
    InvalidId {
        /// What the model wrote.
        value: String,
        /// Why it is not an id.
        message: String,
    },
    /// A capability the catalogue does not offer.
    #[error("unknown capability `{0}`")]
    UnknownCapability(String),
    /// A version range that does not parse.
    #[error("invalid version range `{value}`: {message}")]
    InvalidVersion {
        /// What the model wrote.
        value: String,
        /// Why it is not a range.
        message: String,
    },
    /// A remote policy this producer does not accept.
    #[error("invalid remote policy `{0}` (expected forbidden, allowed, or peer:<node-id>)")]
    InvalidRemote(String),
    /// The host refused the proposal.
    #[error(transparent)]
    Refused(#[from] ProposalError),
    /// The host accepted it but the resolver could not build a plan.
    #[error(transparent)]
    Unresolvable(#[from] CompositionError),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProposal {
    requirements: Vec<RawRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequirement {
    capability: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    remote: Option<String>,
}

/// Render the catalogue into the prompt a model answers with a proposal.
///
/// The prompt lists the exact ids the catalogue offers and the JSON shape the
/// parser accepts, so a model that follows it cannot name a capability the
/// host could not already see.
pub fn prompt(catalogue: &Catalogue, goal: &str) -> String {
    let mut text =
        String::from("Compose the fewest capabilities that satisfy this goal. Use only the ");
    text.push_str("capability ids listed below; do not invent ids.\n\nGoal: ");
    text.push_str(goal);
    text.push_str("\n\nAvailable capabilities:\n");
    for descriptor in catalogue.iter() {
        let side_effects: Vec<&str> = descriptor
            .side_effects
            .iter()
            .map(|effect| effect.as_str())
            .collect();
        text.push_str(&format!(
            "- {} (version {}, risk {}, side effects: {})\n",
            descriptor.id,
            descriptor.version,
            descriptor.risk.as_str(),
            if side_effects.is_empty() {
                "none".to_string()
            } else {
                side_effects.join(", ")
            }
        ));
    }
    text.push_str(
        "\nReply with JSON only, with no other fields:\n\
{\"requirements\":[{\"capability\":\"<provider>/<name>\",\"version\":\"<semver range>\",\"remote\":\"forbidden\"}]}\n",
    );
    text
}

/// Parse a model proposal into a specification, without validating authority.
///
/// Parsing is deliberately strict: an unknown field is an error, so a model
/// cannot smuggle a setting the host never reads. Authority is checked by
/// [`compose`], not here.
pub fn parse(value: &Value, catalogue: &Catalogue) -> Result<CompositionSpec, ComposeError> {
    let raw: RawProposal = serde_json::from_value(value.clone())
        .map_err(|error| ComposeError::Json(error.to_string()))?;
    if raw.requirements.is_empty() {
        return Err(ComposeError::Empty);
    }

    let mut spec = CompositionSpec::new("compose:model", Version::new(1, 0, 0), "a model proposal");
    for requirement in raw.requirements {
        let id: CapabilityId =
            requirement
                .capability
                .parse()
                .map_err(|error: capability::InvalidId| ComposeError::InvalidId {
                    value: requirement.capability.clone(),
                    message: error.to_string(),
                })?;
        if catalogue.get(&id).is_none() {
            return Err(ComposeError::UnknownCapability(requirement.capability));
        }
        let version = match requirement.version {
            Some(text) => {
                VersionReq::parse(&text).map_err(|error| ComposeError::InvalidVersion {
                    value: text.clone(),
                    message: error.to_string(),
                })?
            }
            None => VersionReq::STAR,
        };
        let remote =
            match requirement.remote.as_deref() {
                None | Some("forbidden") => RemotePolicy::Forbidden,
                Some("allowed") => RemotePolicy::Allowed,
                Some(other) => match other.strip_prefix("peer:") {
                    Some(node) => RemotePolicy::Required(NodeId::new(node).map_err(|error| {
                        ComposeError::InvalidRemote(format!("{other}: {error}"))
                    })?),
                    None => return Err(ComposeError::InvalidRemote(other.to_string())),
                },
            };
        spec = spec.with_requirement(CapabilityRequirement::new(id, version).with_remote(remote));
    }
    Ok(spec)
}

/// Parse, approve, and resolve a model proposal.
///
/// This is the whole producer path: there is no variant that skips the gate.
pub fn compose(
    value: &Value,
    catalogue: &Catalogue,
    scope: &LocalScope,
    policy: &ProposalPolicy<'_>,
) -> Result<ResolvedComposition, ComposeError> {
    let spec = parse(value, catalogue)?;
    let approved = approve(&spec, catalogue, policy)?;
    Ok(resolve_approved(approved, catalogue, &[], scope)?)
}
