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

/// Build the per-request assembly report from what actually reached the model
/// (issue #336). Extracted from the runner so the inclusion/omission, retrieval
/// and token accounting are testable across success, missing-source, retrieval-
/// failure and complete-persona cases. Counts and labels only - no raw content.
pub(crate) fn assemble_report(
    persona_complete: bool,
    persona: &str,
    snapshot: &str,
    instructions: Option<&str>,
    memory_state: &MemoryState,
    has_query: bool,
    user_input_tokens: usize,
) -> ContextAssemblyReport {
    let memory_recalled = persona.contains("Relevant memory:");
    let retrieval_hits = if memory_recalled {
        persona
            .split("Relevant memory:")
            .nth(1)
            .map(|block| block.lines().filter(|l| l.starts_with("- ")).count())
            .unwrap_or(0)
    } else {
        0
    };
    let retrieval_skip = if !has_query {
        Some("no usable text query".to_string())
    } else if !memory_state.is_ready() {
        Some(format!("memory {}", memory_state.label()))
    } else if retrieval_hits == 0 {
        Some("no matching items".to_string())
    } else {
        None
    };
    let persona_tokens = agent::agent::context::estimate_tokens(persona);
    let snapshot_tokens = agent::agent::context::estimate_tokens(snapshot);
    let instruction_tokens = instructions
        .map(agent::agent::context::estimate_tokens)
        .unwrap_or(0);
    // Memory and tool guidance ride inside the system prompt, so their tokens
    // are already counted in persona_tokens.
    let sources = vec![
        SourceStatus {
            source: ContextSource::Persona,
            included: !persona.trim().is_empty(),
            reason: None,
            tokens: persona_tokens,
        },
        SourceStatus {
            source: ContextSource::HostRuntime,
            included: !snapshot.is_empty(),
            reason: None,
            tokens: snapshot_tokens,
        },
        SourceStatus {
            source: ContextSource::WorkspaceInstructions,
            included: instructions.is_some(),
            reason: instructions
                .is_none()
                .then(|| "no instruction file".to_string()),
            tokens: instruction_tokens,
        },
        SourceStatus {
            source: ContextSource::RetrievedMemory,
            included: memory_recalled,
            reason: retrieval_skip.clone(),
            tokens: 0,
        },
        SourceStatus {
            source: ContextSource::ToolGuidance,
            included: !persona_complete,
            reason: persona_complete.then(|| "complete persona".to_string()),
            tokens: 0,
        },
        SourceStatus {
            source: ContextSource::UserInput,
            included: true,
            reason: None,
            tokens: user_input_tokens,
        },
    ];
    ContextAssemblyReport {
        persona_complete,
        fingerprint: ContextAssemblyReport::compute_fingerprint(
            persona_complete,
            &sources,
            memory_state,
        ),
        sources,
        memory: memory_state.clone(),
        retrieval_hits,
        retrieval_skip,
        estimated_tokens: persona_tokens + snapshot_tokens + instruction_tokens,
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

    /// The report reflects the assembled request across success, missing
    /// sources, retrieval failure and complete-persona cases (issue #336).
    #[test]
    fn report_covers_success_missing_sources_failure_and_complete() {
        let source = |r: &ContextAssemblyReport, s: ContextSource| {
            r.sources.iter().find(|x| x.source == s).unwrap().clone()
        };

        // Successful assembly: memory recalled, instructions present.
        let persona = "PERSONA\n\nRelevant memory:\n- a note\n- another";
        let r = assemble_report(
            false,
            persona,
            "SNAPSHOT",
            Some("INSTRUCTIONS"),
            &MemoryState::Ready,
            true,
            5,
        );
        assert!(source(&r, ContextSource::RetrievedMemory).included);
        assert_eq!(r.retrieval_hits, 2);
        assert!(r.retrieval_skip.is_none());
        assert!(source(&r, ContextSource::WorkspaceInstructions).included);
        assert!(r.estimated_tokens > 0);

        // Missing sources: no instruction file, no matching memory.
        let r = assemble_report(
            false,
            "PERSONA",
            "SNAPSHOT",
            None,
            &MemoryState::Empty,
            true,
            5,
        );
        let instr = source(&r, ContextSource::WorkspaceInstructions);
        assert!(!instr.included);
        assert_eq!(instr.reason.as_deref(), Some("no instruction file"));
        assert_eq!(r.retrieval_hits, 0);
        assert_eq!(r.retrieval_skip.as_deref(), Some("no matching items"));

        // Retrieval skipped: no usable query (a media-only turn).
        let r = assemble_report(
            false,
            "PERSONA",
            "SNAPSHOT",
            None,
            &MemoryState::Ready,
            false,
            0,
        );
        assert_eq!(r.retrieval_skip.as_deref(), Some("no usable text query"));

        // Retrieval unavailable: memory failed to initialize.
        let r = assemble_report(
            false,
            "PERSONA",
            "SNAPSHOT",
            None,
            &MemoryState::InitFailed { reason: "x".into() },
            true,
            5,
        );
        assert_eq!(r.retrieval_skip.as_deref(), Some("memory init-failed"));

        // Complete persona: tool guidance omitted, mode reported.
        let r = assemble_report(
            true,
            "PERSONA",
            "SNAPSHOT",
            None,
            &MemoryState::Disabled,
            true,
            5,
        );
        let guidance = source(&r, ContextSource::ToolGuidance);
        assert!(!guidance.included);
        assert_eq!(guidance.reason.as_deref(), Some("complete persona"));
        assert!(r.persona_complete);
    }

    /// Composition is deterministic and bounded: equivalent inputs give the
    /// same fingerprint, and the report never carries raw content (issue #337).
    #[test]
    fn composition_is_deterministic_and_bounded() {
        let a = assemble_report(false, "P", "S", Some("I"), &MemoryState::Ready, true, 3);
        let b = assemble_report(false, "P", "S", Some("I"), &MemoryState::Ready, true, 3);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.estimated_tokens, b.estimated_tokens);
        // The report is an account of counts/labels, not a copy of the content.
        assert_eq!(a.sources.len(), 6, "one status per context source");
    }
}
