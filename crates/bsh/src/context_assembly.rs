//! Unified context assembly for BSH (issues #332-#337).
//!
//! Persona, runtime facts, workspace instructions, retrieved memory and tool
//! guidance used to be assembled across separate paths with no shared account
//! of what reached the model. This module is the single seam that names each
//! context source, its trust boundary, and its inclusion status, and produces
//! the per-request diagnostics record. It is the concrete first slice of the
//! Context Assembly Engine design in docs/context-assembly-engine.md (#337):
//! the sources are typed and reported here today, and become independently
//! composable providers along the migration plan without a rewrite.

/// The provenance of one context source (#333).
///
/// The distinction survives to the assembly boundary so policy and diagnostics
/// can tell host-authored guidance from workspace- or memory-supplied data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextSource {
    /// The persona / agent identity (user-configurable).
    Persona,
    /// Host-owned runtime contract and execution constraints.
    HostRuntime,
    /// Workspace instruction files (AGENTS.md and friends).
    WorkspaceInstructions,
    /// Retrieved persistent memory.
    RetrievedMemory,
    /// Capability / per-tool guidance.
    ToolGuidance,
    /// The reader's own conversation input.
    UserInput,
}

/// The trust boundary a source sits on (#333).
///
/// Host authority is the only source that may state policy; workspace and
/// retrieved content are DATA the model reads, never authority to override the
/// host's enforced controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trust {
    /// Host-authored guidance and invariants.
    HostAuthority,
    /// Untrusted data (workspace files, retrieved memory, user input).
    UntrustedData,
}

impl ContextSource {
    /// The trust boundary this source sits on.
    pub(crate) fn trust(self) -> Trust {
        match self {
            // The persona is user-configurable text; it is data, not host
            // authority. The runtime contract and tool guidance are the host's.
            ContextSource::Persona
            | ContextSource::WorkspaceInstructions
            | ContextSource::RetrievedMemory
            | ContextSource::UserInput => Trust::UntrustedData,
            ContextSource::HostRuntime | ContextSource::ToolGuidance => Trust::HostAuthority,
        }
    }

    /// The source an inject-event block kind names, so the provenance survives
    /// from the assembly boundary to the transcript (issue #333).
    pub(crate) fn from_injection_kind(kind: &str) -> Option<ContextSource> {
        match kind {
            "persona" => Some(ContextSource::Persona),
            "context" => Some(ContextSource::HostRuntime),
            "instructions" => Some(ContextSource::WorkspaceInstructions),
            _ => None,
        }
    }

    /// A stable, human-readable label for diagnostics.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ContextSource::Persona => "persona",
            ContextSource::HostRuntime => "host-runtime",
            ContextSource::WorkspaceInstructions => "workspace-instructions",
            ContextSource::RetrievedMemory => "retrieved-memory",
            ContextSource::ToolGuidance => "tool-guidance",
            ContextSource::UserInput => "user-input",
        }
    }
}

/// The memory lifecycle state (#334).
///
/// A configured, enabled memory that failed to open is distinct from one that
/// is disabled or simply empty, so the failure is surfaced rather than silently
/// disabling the feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemoryState {
    /// Switched off in configuration.
    Disabled,
    /// Opened and holding items.
    Ready,
    /// Opened successfully but holding nothing.
    Empty,
    /// Enabled but the store could not be opened; the reason is actionable and
    /// carries no memory content.
    InitFailed { reason: String },
}

impl MemoryState {
    /// Whether a memory store is attached and usable.
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, MemoryState::Ready | MemoryState::Empty)
    }

    /// A short, non-sensitive diagnostic label.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            MemoryState::Disabled => "disabled",
            MemoryState::Ready => "ready",
            MemoryState::Empty => "empty",
            MemoryState::InitFailed { .. } => "init-failed",
        }
    }
}

/// One source's inclusion status in an assembled request (#336).
#[derive(Debug, Clone)]
pub(crate) struct SourceStatus {
    /// Which source this is.
    pub(crate) source: ContextSource,
    /// Whether it was included in the request.
    pub(crate) included: bool,
    /// Why it was omitted, when it was.
    pub(crate) reason: Option<String>,
    /// Estimated tokens it contributes (0 when omitted).
    pub(crate) tokens: usize,
}

/// The per-request context assembly report (#336).
///
/// A privacy-conscious account of what actually reached the model: source
/// inclusion/omission, memory state, retrieval hits, token estimates and a
/// stable fingerprint for comparing equivalent requests. It carries counts and
/// labels, never raw memory or workspace content.
#[derive(Debug, Clone)]
pub(crate) struct ContextAssemblyReport {
    /// Whether the persona ran in complete mode.
    pub(crate) persona_complete: bool,
    /// Each source's inclusion status.
    pub(crate) sources: Vec<SourceStatus>,
    /// The memory lifecycle state.
    pub(crate) memory: MemoryState,
    /// How many memory items retrieval returned this turn.
    pub(crate) retrieval_hits: usize,
    /// Why retrieval was skipped, when it was (e.g. no usable query).
    pub(crate) retrieval_skip: Option<String>,
    /// Estimated total context tokens (documented estimator).
    pub(crate) estimated_tokens: usize,
    /// A stable fingerprint of the assembled composition.
    pub(crate) fingerprint: u64,
}

impl ContextAssemblyReport {
    /// A fingerprint over the composition: the ordered source labels, their
    /// inclusion, the persona mode and the memory state. Equivalent assemblies
    /// produce the same value; it is not a hash of any private content.
    pub(crate) fn compute_fingerprint(
        persona_complete: bool,
        sources: &[SourceStatus],
        memory: &MemoryState,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        persona_complete.hash(&mut hasher);
        memory.label().hash(&mut hasher);
        for status in sources {
            status.source.label().hash(&mut hasher);
            status.included.hash(&mut hasher);
        }
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_and_memory_are_untrusted_data() {
        assert_eq!(
            ContextSource::WorkspaceInstructions.trust(),
            Trust::UntrustedData
        );
        assert_eq!(ContextSource::RetrievedMemory.trust(), Trust::UntrustedData);
        assert_eq!(ContextSource::UserInput.trust(), Trust::UntrustedData);
        assert_eq!(ContextSource::Persona.trust(), Trust::UntrustedData);
        assert_eq!(ContextSource::HostRuntime.trust(), Trust::HostAuthority);
        assert_eq!(ContextSource::ToolGuidance.trust(), Trust::HostAuthority);
    }

    #[test]
    fn memory_states_are_distinct() {
        assert!(!MemoryState::Disabled.is_ready());
        assert!(MemoryState::Ready.is_ready());
        assert!(MemoryState::Empty.is_ready());
        assert!(!MemoryState::InitFailed { reason: "x".into() }.is_ready());
        assert_eq!(
            MemoryState::InitFailed { reason: "x".into() }.label(),
            "init-failed"
        );
    }

    #[test]
    fn injection_kinds_map_to_their_trust_boundary() {
        // Workspace instructions and the persona are data, never host authority:
        // injection-like text in them cannot claim the host's policy voice.
        let instr = ContextSource::from_injection_kind("instructions").unwrap();
        assert_eq!(instr.trust(), Trust::UntrustedData);
        let persona = ContextSource::from_injection_kind("persona").unwrap();
        assert_eq!(persona.trust(), Trust::UntrustedData);
        let runtime = ContextSource::from_injection_kind("context").unwrap();
        assert_eq!(runtime.trust(), Trust::HostAuthority);
        assert!(ContextSource::from_injection_kind("unknown").is_none());
    }

    #[test]
    fn fingerprint_is_stable_and_content_free() {
        let sources = vec![
            SourceStatus {
                source: ContextSource::Persona,
                included: true,
                reason: None,
                tokens: 10,
            },
            SourceStatus {
                source: ContextSource::HostRuntime,
                included: true,
                reason: None,
                tokens: 20,
            },
        ];
        let a = ContextAssemblyReport::compute_fingerprint(false, &sources, &MemoryState::Ready);
        let b = ContextAssemblyReport::compute_fingerprint(false, &sources, &MemoryState::Ready);
        assert_eq!(a, b, "equivalent assemblies share a fingerprint");
        let c = ContextAssemblyReport::compute_fingerprint(true, &sources, &MemoryState::Ready);
        assert_ne!(a, c, "the persona mode changes the fingerprint");
        // Token counts do not affect the composition fingerprint.
        let mut other = sources.clone();
        other[0].tokens = 999;
        let d = ContextAssemblyReport::compute_fingerprint(false, &other, &MemoryState::Ready);
        assert_eq!(a, d, "the fingerprint names the composition, not the size");
    }
}
