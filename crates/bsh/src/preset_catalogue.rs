//! The built-in preset CATALOGUE and the resolved authorization scope.
//!
//! The catalogue is not a second copy of the shipped declarations: it walks
//! [`GlobalSettings::preset_ids`] and asks [`GlobalSettings::preset`] for each
//! one, so a card and the build that runs the preset cannot describe two
//! different presets. A shipped preset therefore appears with the level its
//! declaration (or the derivation) produces, and a reader's declaration replaces
//! it wholesale, exactly as the build does.
//!
//! The resolved scope is the second half: a card shows what the preset will
//! ACTUALLY allow once the session's permission and the workspace's egress
//! state are folded in, not only what the file declares. A read-only session
//! shows "no writes" even when the preset grants a write scope; a declared
//! write scope shows the roots AND that the baseline still confines them to the
//! workspace.

use super::settings::global::{
    GlobalSettings, PolicyMode, Preset, SandboxChoice, MAX_PRESET_LEVEL,
};
use agent::policy::PolicySet;

/// One preset as the catalogue and the cards read it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogueEntry {
    /// The id the settings file and the session name.
    pub id: String,
    /// The declared display name, if any (shipped copy lives in the locale).
    pub name: Option<String>,
    /// The declared description, if any.
    pub description: Option<String>,
    /// Whether the shipped list serves this id (a declaration replaces it).
    pub builtin: bool,
    /// The authorization level in force (declared, or derived).
    pub level: u8,
    /// The level the declaration names, when it names one.
    pub declared_level: Option<u8>,
    /// The permission ceiling the declaration names, if any.
    pub permission: Option<String>,
    /// The shell sandbox choice the declaration names, if any.
    pub sandbox: Option<SandboxChoice>,
    /// The declared read scope (absent = the whole workspace).
    pub read: Option<Vec<String>>,
    /// The declared write scope (absent = the whole workspace).
    pub write: Option<Vec<String>>,
    /// The declared deny list.
    pub deny: Vec<String>,
    /// The declared egress list (absent = inherit the workspace).
    pub egress: Option<Vec<String>>,
    /// The hosted web-search choice (absent = inherit).
    pub web_search: Option<bool>,
    /// The policy mode in force (absent is required).
    pub policy_mode: PolicyMode,
    /// The declaration itself, so a card never re-reads a second source.
    pub preset: Preset,
}

/// Every preset on the roster, in display order, with its derived level.
pub(crate) fn catalogue(global: &GlobalSettings) -> Vec<CatalogueEntry> {
    global
        .preset_ids()
        .into_iter()
        .filter_map(|id| {
            let preset = global.preset(&id)?;
            Some(CatalogueEntry {
                builtin: global.preset_is_shipped(&id),
                level: preset.effective_level(),
                declared_level: preset.level,
                name: preset.name.clone(),
                description: preset.description.clone(),
                permission: preset.permission.clone(),
                sandbox: preset.sandbox,
                read: preset.read.clone(),
                write: preset.write.clone(),
                deny: preset.deny.clone(),
                egress: preset.egress.clone(),
                web_search: preset.web_search,
                policy_mode: preset.policy_mode.unwrap_or_default(),
                preset,
                id,
            })
        })
        .collect()
}

/// How a path scope resolves after the session tier and the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScopeReach {
    /// The whole workspace: the preset declared no list.
    Workspace,
    /// Nothing: the preset declared an empty list, or the tier forbids writes.
    None,
    /// The declared entries, matched literally or as globs.
    Entries(Vec<String>),
}

/// The workspace's egress state, as the baseline holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EgressReach {
    /// No policy has been built yet, so the workspace state is unknown.
    Unknown,
    /// No outbound connection is allowed.
    DenyAll,
    /// The workspace declared no allowlist: the network is reachable.
    Unrestricted,
    /// The declared rules. `workspace_restricted` says a workspace allowlist
    /// also applies, so the entries shown are the preset's and are intersected
    /// with the workspace's.
    Rules {
        /// The entries in display form.
        entries: Vec<String>,
        /// Whether the workspace allowlist also constrains them.
        workspace_restricted: bool,
    },
}

/// Read the workspace reach out of the live policy's data document.
///
/// No file is read here: the policy already parsed the workspace allowlist at
/// build time, and the pane must not touch the disk per frame.
pub(crate) fn workspace_reach(policy: Option<&PolicySet>) -> EgressReach {
    let Some(policy) = policy else {
        return EgressReach::Unknown;
    };
    let data = policy.data();
    if data.egress_unrestricted {
        return EgressReach::Unrestricted;
    }
    let mut entries = data.egress_allowlist.clone();
    entries.extend(data.egress_hosts.iter().cloned());
    entries.extend(data.egress_ports.iter().map(u16::to_string));
    if entries.is_empty() {
        EgressReach::DenyAll
    } else {
        EgressReach::Rules {
            entries,
            workspace_restricted: true,
        }
    }
}

/// The preset's egress reach intersected with the workspace's.
///
/// `None` inherits the workspace; a declared empty list denies all; a declared
/// list is the preset's, further narrowed by the workspace list when one
/// exists. The intersection is never a union.
pub(crate) fn egress_reach_list(egress: Option<&[String]>, workspace: &EgressReach) -> EgressReach {
    match egress {
        None => workspace.clone(),
        Some([]) => EgressReach::DenyAll,
        Some(list) => match workspace {
            EgressReach::DenyAll => EgressReach::DenyAll,
            EgressReach::Unknown => EgressReach::Rules {
                entries: list.to_vec(),
                workspace_restricted: false,
            },
            EgressReach::Unrestricted => EgressReach::Rules {
                entries: list.to_vec(),
                workspace_restricted: false,
            },
            EgressReach::Rules {
                workspace_restricted,
                ..
            } => EgressReach::Rules {
                entries: list.to_vec(),
                workspace_restricted: *workspace_restricted,
            },
        },
    }
}

/// Everything a card shows about the scope in force for one preset.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedPresetScope {
    /// The level in force.
    pub level: u8,
    /// The permission in force: the session's mode capped by the preset.
    pub permission: String,
    /// Reads: the preset's list, or the whole workspace, or none.
    pub read: ScopeReach,
    /// Writes: none when the tier is read-only, else the preset's list or the
    /// whole workspace.
    pub write: ScopeReach,
    /// Denied paths: the preset's, plus the baseline's `.git`/`.bsh`.
    pub deny: Vec<String>,
    /// Egress after intersecting the preset with the workspace.
    pub egress: EgressReach,
    /// Whether hosted web search is offered.
    pub web_search: bool,
    /// Required or advisory policy refusals.
    pub policy_mode: PolicyMode,
}

impl ResolvedPresetScope {
    /// Resolve one bare declaration against the session's permission and the
    /// workspace's reach.
    ///
    /// Test-only: the UI reads [`Self::from_catalogue`], and this is the same
    /// computation for a declaration that is not in a catalogue. It stays in
    /// one place so the two cannot drift.
    #[cfg(test)]
    pub(crate) fn resolve(
        preset: &Preset,
        session_permission: &str,
        workspace: &EgressReach,
    ) -> Self {
        Self::from_parts(
            preset.effective_level(),
            preset.permission.as_deref(),
            preset.read.as_deref(),
            preset.write.as_deref(),
            &preset.deny,
            preset.egress.as_deref(),
            preset.web_search,
            preset.policy_mode.unwrap_or_default(),
            session_permission,
            workspace,
        )
    }

    /// Resolve one CATALOGUE entry, the list the UI actually reads.
    ///
    /// It reads the entry's declared fields directly, so the catalogue is the
    /// one source a card needs; [`Self::resolve`] is the same computation for
    /// tests that hold a bare declaration.
    pub(crate) fn from_catalogue(
        entry: &CatalogueEntry,
        session_permission: &str,
        workspace: &EgressReach,
    ) -> Self {
        Self::from_parts(
            entry.level,
            entry.permission.as_deref(),
            entry.read.as_deref(),
            entry.write.as_deref(),
            &entry.deny,
            entry.egress.as_deref(),
            entry.web_search,
            entry.policy_mode,
            session_permission,
            workspace,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        level: u8,
        permission: Option<&str>,
        read: Option<&[String]>,
        write: Option<&[String]>,
        deny: &[String],
        egress: Option<&[String]>,
        web_search: Option<bool>,
        policy_mode: PolicyMode,
        session_permission: &str,
        workspace: &EgressReach,
    ) -> Self {
        let permission =
            super::settings::resolve::PermissionResolution::resolve(session_permission, permission);
        let reads = match read {
            None => ScopeReach::Workspace,
            Some([]) => ScopeReach::None,
            Some(list) => ScopeReach::Entries(list.to_vec()),
        };
        let writes = if !super::caps::allows_writes(&permission.effective) {
            ScopeReach::None
        } else {
            match write {
                None => ScopeReach::Workspace,
                Some([]) => ScopeReach::None,
                Some(list) => ScopeReach::Entries(list.to_vec()),
            }
        };
        let mut deny = deny.to_vec();
        for baseline in [".git", ".bsh"] {
            if !deny.iter().any(|entry| entry == baseline) {
                deny.push(baseline.to_string());
            }
        }
        Self {
            level,
            permission: permission.effective,
            read: reads,
            write: writes,
            deny,
            egress: egress_reach_list(egress, workspace),
            web_search: web_search.unwrap_or(true),
            policy_mode,
        }
    }
}

/// One consequence a widening preset carries, named in the dialog.
///
/// The words are the same facts the audit and the bash result use:
/// `danger-full-access` for the tier, `PRESET_SANDBOX_OFF_REASON` for the
/// released shell, the egress allowlist for the closed network and the
/// recorded refusal for an advisory policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RiskFact {
    /// The preset's ceiling is `danger-full-access`.
    FullAccess,
    /// The preset turns the shell sandbox off (`sandbox = "none"`).
    SandboxOff,
    /// The preset denies ALL egress (`egress = []`).
    EgressDeniedAll,
    /// The preset's own policy refusals are advisory.
    AdvisoryPolicy,
}

/// Every risk fact one declaration carries, whether or not it widens anything.
pub(crate) fn risk_facts(preset: &Preset) -> Vec<RiskFact> {
    let mut facts = Vec::new();
    let full = preset.permission.as_deref().map(str::trim) == Some("danger-full-access");
    if full {
        facts.push(RiskFact::FullAccess);
    }
    if full && preset.sandbox == Some(SandboxChoice::Off) {
        facts.push(RiskFact::SandboxOff);
    }
    if preset.egress.as_ref().is_some_and(Vec::is_empty) {
        facts.push(RiskFact::EgressDeniedAll);
    }
    if preset.policy_mode == Some(PolicyMode::Advisory) && preset.policy.is_some() {
        facts.push(RiskFact::AdvisoryPolicy);
    }
    facts
}

/// The consequences to confirm when `target` would widen over `current`.
///
/// A change widens when it RAISES the authorization level, or when it makes
/// the preset's own policy advisory (a refusal that no longer stops the call)
/// without lowering the level. A preset that only narrows - read-only, or a
/// lower grade - never prompts, which is why picking one of those is still a
/// single click. The returned facts are what the dialog names.
pub(crate) fn widening_facts(current: Option<&Preset>, target: &Preset) -> Vec<RiskFact> {
    let current_level = current.map(Preset::effective_level).unwrap_or(0);
    let target_level = target.effective_level();
    let raises_level = target_level > current_level;
    let becomes_advisory = target.policy_mode == Some(PolicyMode::Advisory)
        && target.policy.is_some()
        && current.and_then(|preset| preset.policy_mode) != Some(PolicyMode::Advisory)
        && target_level >= current_level;
    if !raises_level && !becomes_advisory {
        return Vec::new();
    }
    // A level beyond the documented table cannot be confirmed into existence.
    debug_assert!(target_level <= MAX_PRESET_LEVEL);
    risk_facts(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::global::{built_in_preset, PolicyMode};

    fn shipped(id: &str) -> Preset {
        built_in_preset(id).expect("a shipped preset")
    }

    /// The level table, measured from what each shipped preset actually allows.
    ///
    /// read-only denies writes and the shell and denies local egress - grade 1.
    /// standard, ptc and creator declare NO ceiling and keep the shell, so the
    /// session tier decides - they are one grade, 2, and inventing a different
    /// one would contradict the code. The deriving rule points the same way.
    #[test]
    fn each_shipped_preset_resolves_to_a_level_consistent_with_what_it_allows() {
        let table: [(&str, u8, Option<&str>, bool, bool); 4] = [
            ("read-only", 1, Some("read-only"), false, true),
            ("standard", 2, None, true, false),
            ("ptc", 2, None, true, false),
            ("creator", 2, None, true, false),
        ];
        for (id, level, permission, shell, egress_denied) in table {
            let preset = shipped(id);
            assert_eq!(
                preset.effective_level(),
                level,
                "{id}: declared/derived level"
            );
            assert_eq!(
                preset.derived_level(),
                level,
                "{id}: the derivation agrees with the declaration"
            );
            assert_eq!(preset.permission.as_deref(), permission, "{id}");
            assert_eq!(
                preset.plugins.iter().any(|plugin| plugin == "shell"),
                shell,
                "{id}: the shell bundle is what the level is about"
            );
            assert_eq!(
                preset.egress.as_ref().is_some_and(Vec::is_empty),
                egress_denied,
                "{id}: local egress"
            );
        }
        // The catalogue is a real list derived from the settings tree.
        let global = GlobalSettings::default();
        let entries = catalogue(&global);
        assert_eq!(entries.len(), 4, "{entries:?}");
        for entry in &entries {
            assert_eq!(entry.level, entry.preset.effective_level());
            assert!(entry.builtin, "the default roster is all shipped");
        }
    }

    /// The absent fields behave exactly as before: standard, ptc and creator
    /// resolve to "the workspace" for reads and writes, while read-only keeps
    /// its declared read root and its empty write list.
    #[test]
    fn an_absent_scope_still_resolves_to_the_whole_workspace() {
        let workspace = EgressReach::Unrestricted;
        for id in ["standard", "ptc", "creator"] {
            let scope = ResolvedPresetScope::resolve(&shipped(id), "workspace-write", &workspace);
            assert_eq!(scope.read, ScopeReach::Workspace, "{id} read");
            assert_eq!(scope.write, ScopeReach::Workspace, "{id} write");
            assert!(
                scope.deny.iter().any(|entry| entry == ".git"),
                "{id} keeps the baseline deny list"
            );
            assert_eq!(scope.egress, EgressReach::Unrestricted, "{id} egress");
        }
        let read_only =
            ResolvedPresetScope::resolve(&shipped("read-only"), "workspace-write", &workspace);
        assert_eq!(read_only.read, ScopeReach::Entries(vec![".".to_string()]));
        assert_eq!(read_only.write, ScopeReach::None);
        assert_eq!(read_only.egress, EgressReach::DenyAll);
        assert!(
            read_only.web_search,
            "hosted search still runs on the provider"
        );
    }

    /// The session tier intersects the write scope: a read-only session shows
    /// no writes even when the preset grants them.
    #[test]
    fn the_session_tier_intersects_the_resolved_scope() {
        let mut preset = shipped("standard");
        preset.write = Some(vec!["docs".to_string()]);
        let workspace = EgressReach::Unrestricted;
        let writing = ResolvedPresetScope::resolve(&preset, "workspace-write", &workspace);
        assert_eq!(writing.write, ScopeReach::Entries(vec!["docs".to_string()]));
        let read_only = ResolvedPresetScope::resolve(&preset, "read-only", &workspace);
        assert_eq!(
            read_only.write,
            ScopeReach::None,
            "the tier, not the file, decides that a read-only turn writes nothing"
        );
        assert_eq!(read_only.permission, "read-only");
    }

    /// Egress is an intersection: a preset list narrows an unrestricted
    /// workspace, and a workspace deny-all narrows every preset.
    #[test]
    fn egress_is_intersected_never_unioned() {
        let mut preset = shipped("standard");
        preset.egress = Some(vec!["203.0.113.7".to_string()]);
        assert_eq!(
            egress_reach_list(preset.egress.as_deref(), &EgressReach::Unrestricted),
            EgressReach::Rules {
                entries: vec!["203.0.113.7".to_string()],
                workspace_restricted: false,
            }
        );
        assert_eq!(
            egress_reach_list(preset.egress.as_deref(), &EgressReach::DenyAll),
            EgressReach::DenyAll,
            "a workspace deny-all wins"
        );
        preset.egress = Some(Vec::new());
        assert_eq!(
            egress_reach_list(preset.egress.as_deref(), &EgressReach::Unrestricted),
            EgressReach::DenyAll
        );
        preset.egress = None;
        assert_eq!(
            egress_reach_list(preset.egress.as_deref(), &EgressReach::Unrestricted),
            EgressReach::Unrestricted
        );
    }

    /// A widening change names its consequences; a narrowing one does not.
    #[test]
    fn widening_facts_name_the_consequences_and_narrowing_never_prompts() {
        let standard = shipped("standard");
        let mut wide = Preset {
            permission: Some("danger-full-access".to_string()),
            sandbox: Some(SandboxChoice::Off),
            egress: Some(Vec::new()),
            policy: Some(
                "package policy.wide\n# rule: wide\nverdict := {\"allow\": true}".to_string(),
            ),
            policy_mode: Some(PolicyMode::Advisory),
            ..Preset::default()
        };
        wide.level = Some(4);
        let facts = widening_facts(Some(&standard), &wide);
        for fact in [
            RiskFact::FullAccess,
            RiskFact::SandboxOff,
            RiskFact::EgressDeniedAll,
            RiskFact::AdvisoryPolicy,
        ] {
            assert!(facts.contains(&fact), "missing {fact:?}: {facts:?}");
        }

        assert!(
            widening_facts(Some(&standard), &shipped("read-only")).is_empty(),
            "reading only is a narrowing and must not prompt"
        );
        assert!(
            widening_facts(Some(&standard), &standard).is_empty(),
            "the same preset is not a change"
        );
        assert!(
            widening_facts(Some(&wide), &standard).is_empty(),
            "coming back down never prompts"
        );
    }
}
