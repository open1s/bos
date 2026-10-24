//! The permission MODE: the reader's one access decision.
//!
//! Three choices, in the reference's own vocabulary - Read Only, Workspace
//! Write and Full access - each mapped onto the tier the backend already
//! enforces. The mode is the only permission choice the front of the shell
//! offers; every other field (path scopes, egress, sandbox, web search, policy
//! mode, level) is a detail behind the preset's Advanced disclosure and keeps
//! enforcing exactly as before.
//!
//! One source of truth: the chip, the slash picker, the General row and the
//! preset card all call these functions, so no two surfaces can name or explain
//! the same mode differently.
//!
//! # The network clause is DERIVED
//!
//! A mode line that described files and commands but said nothing about the
//! network was false by omission: Workspace Write can reach the network, while
//! Read Only cannot write and its session tier refuses local egress. The clause
//! for a WRITABLE mode is therefore built from the resolved egress state at the
//! moment the row is drawn:
//!
//! - the workspace's `.bsh/egress-allowlist` is TRI-STATE - ABSENT leaves the
//!   network unrestricted, PRESENT with no rule is a deliberate deny-all, and
//!   PRESENT with rules is the allowlist;
//! - a preset's own `egress` field intersects (never unions) with that state;
//!   `egress = []` denies every destination, entries narrow the reach.
//!
//! The classification goes through the policy's own parser, so a row cannot
//! claim an "unrestricted" network a deny-all preset forbids, nor a "bounded"
//! one when no file and no preset narrows anything.
//!
//! Read Only is the ONE exception, and it is deliberately not derived: the
//! session tier (`session-read-only-egress` in
//! `agent::policy::SESSION_PERMISSION_REGO`) refuses every local egress at that
//! mode, so no workspace allowlist can make a local socket reachable. Its row
//! states that refusal and names hosted web search - which runs on the model
//! provider and opens no local socket - as the one thing still available.

use crate::caps::WorkspaceEgress;
use crate::i18n::{tr, tr_args, Lang};
use crate::settings::resolve::{PermissionResolution, SandboxResolution};
use crate::settings::Effective;
use crate::state::AppState;

/// The three modes, in the reference's own order.
pub(crate) const MODES: [Mode; 3] = [Mode::ReadOnly, Mode::WorkspaceWrite, Mode::FullAccess];

/// One permission mode, mapped onto the tier the build obeys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Nothing is written and no command runs.
    ReadOnly,
    /// Writes and commands are confined to the workspace.
    WorkspaceWrite,
    /// No confinement: files anywhere can be written, and the shell's own
    /// network bypasses the sandbox allowlist.
    FullAccess,
}

impl Mode {
    /// The stored tier value, exactly the one `Settings.permission` holds.
    pub(crate) fn value(self) -> &'static str {
        match self {
            Self::ReadOnly => crate::caps::READ_ONLY,
            Self::WorkspaceWrite => crate::caps::WORKSPACE_WRITE,
            Self::FullAccess => crate::caps::FULL_ACCESS,
        }
    }

    /// The mode a stored permission value names. Unknown text is read-only,
    /// matching `PermissionResolution`'s fail-closed normalisation.
    pub(crate) fn from_permission(value: &str) -> Self {
        match value.trim() {
            crate::caps::WORKSPACE_WRITE => Self::WorkspaceWrite,
            crate::caps::FULL_ACCESS => Self::FullAccess,
            _ => Self::ReadOnly,
        }
    }

    /// Rank for "lesser-or-equal" comparisons: read-only < workspace < full.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::ReadOnly => 0,
            Self::WorkspaceWrite => 1,
            Self::FullAccess => 2,
        }
    }

    /// The reference's own label key.
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::ReadOnly => "permission.presetReadOnly",
            Self::WorkspaceWrite => "permission.presetWorkspaceWrite",
            Self::FullAccess => "permission.presetFullAccess",
        }
    }

    /// The one-line consequence key, in the reader's words.
    pub(crate) fn consequence_key(self) -> &'static str {
        match self {
            Self::ReadOnly => "permission.consequenceReadOnly",
            Self::WorkspaceWrite => "permission.consequenceWorkspace",
            Self::FullAccess => "permission.consequenceFull",
        }
    }

    /// The label in the active language.
    pub(crate) fn label(self, lang: Lang) -> String {
        tr(lang, self.label_key())
    }

    /// The one-line consequence in the active language, network included.
    ///
    /// The filesystem and command half is the mode's own sentence; the network
    /// half of a WRITABLE mode is built from `network` so the row cannot state
    /// a reach the turn does not get. Read Only is deliberately NOT derived:
    /// its local egress is refused by the session tier, so no workspace
    /// allowlist can make the browser, a page fetch or MCP HTTP reachable.
    /// Under Full access a preset that keeps the shell confined swaps in the
    /// sentence that names that constraint, because it is the fact the reader
    /// cannot otherwise see.
    pub(crate) fn consequence(
        self,
        lang: Lang,
        preset_forces_heel: bool,
        network: &NetworkFacts<'_>,
    ) -> String {
        let head = if self == Self::FullAccess && preset_forces_heel {
            "permission.consequenceFullShell"
        } else {
            self.consequence_key()
        };
        let clause = match self {
            // NOT derived from `network`: the session tier refuses every local
            // egress at this mode regardless of the workspace allowlist, and
            // hosted web search never opens a local socket. See the module doc.
            Self::ReadOnly => tr(lang, "permission.netReadOnly"),
            // With the shell confined by the preset, the shell and the tools
            // share one reach, so the row names the network itself instead of
            // one class of user.
            Self::FullAccess if preset_forces_heel => network_sentence(network.reach(), None, lang),
            Self::WorkspaceWrite => network_sentence(
                network.reach(),
                Some(tr(lang, "permission.netActorsCommands")),
                lang,
            ),
            Self::FullAccess => network_sentence(
                network.reach(),
                Some(tr(lang, "permission.netActorsTools")),
                lang,
            ),
        };
        // English joins two sentences with a space; Chinese punctuation carries
        // its own spacing, so the full stops must touch.
        let separator = if lang == Lang::En { " " } else { "" };
        format!("{}{separator}{clause}", tr(lang, head))
    }
}

/// The network reach one row must state.
///
/// Derived from the two facts that set it - the workspace file's tri-state and
/// the preset's resolved `egress` field - because a menu row cannot claim more
/// reach than a turn would get. The vocabulary is the policy's own: absent file
/// leaves egress UNRESTRICTED, a present file with no rule is a DENY ALL, and a
/// present file with rules is the allowlist; a preset's list INTERSECTS the
/// workspace's and never widens it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NetworkReach {
    /// No workspace file and no preset narrowing: nothing constrains egress.
    Unrestricted,
    /// The workspace file declares rules; the preset adds none.
    WorkspaceAllowlist,
    /// The preset declares rules, intersected with the workspace's own.
    PresetAllowlist,
    /// The file exists and declares no rule: a deliberate deny-all.
    DeniedByWorkspace,
    /// The preset declares `egress = []`: a deliberate deny-all.
    DeniedByPreset,
}

impl NetworkReach {
    /// Whether any destination can be reached.
    ///
    /// The tests read it to discriminate a permitted row from a deny-all one;
    /// the sentence itself is chosen by [`Self::phrase_key`].
    #[cfg(test)]
    pub(crate) fn permitted(self) -> bool {
        !matches!(self, Self::DeniedByWorkspace | Self::DeniedByPreset)
    }

    /// The phrase stating this reach ("network available, ..." / "network denied
    /// by ..."), from the policy's own vocabulary.
    fn phrase_key(self) -> &'static str {
        match self {
            Self::Unrestricted => "permission.netReachFree",
            Self::WorkspaceAllowlist => "permission.netReachWorkspace",
            Self::PresetAllowlist => "permission.netReachPreset",
            Self::DeniedByWorkspace => "permission.netDenyWorkspace",
            Self::DeniedByPreset => "permission.netDenyPreset",
        }
    }
}

/// The two facts a mode's network clause is derived from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NetworkFacts<'a> {
    /// The workspace `.bsh/egress-allowlist` file's tri-state.
    pub(crate) workspace: WorkspaceEgress,
    /// The resolved preset's own `egress`: none inherits the workspace, an empty
    /// list denies every destination, entries narrow the workspace's reach.
    pub(crate) preset: Option<&'a [String]>,
}

impl<'a> NetworkFacts<'a> {
    /// The facts for the preset a resolution carries.
    ///
    /// The slice is borrowed on its own (not through the whole resolution) so a
    /// render that has already chosen this chip can still move the resolution's
    /// other fields.
    pub(crate) fn for_preset(state: &'a AppState, preset: Option<&'a [String]>) -> Self {
        Self {
            workspace: workspace_egress(state.egress_allowlist_present, &state.egress_allowlist),
            preset,
        }
    }

    /// The facts for one session: the workspace file's cached state, narrowed by
    /// the preset the turn resolved.
    pub(crate) fn for_session(state: &'a AppState, effective: &'a Effective) -> Self {
        Self::for_preset(state, effective.preset_egress.as_deref())
    }

    /// The workspace floor alone.
    ///
    /// A DEFAULT row (the General pane's mode select) chooses a mode for future
    /// sessions, whose preset is not chosen yet, so only the workspace file is a
    /// fact that row can state.
    pub(crate) fn for_workspace(state: &'a AppState) -> Self {
        Self {
            workspace: workspace_egress(state.egress_allowlist_present, &state.egress_allowlist),
            preset: None,
        }
    }

    /// The reach after the preset intersects the workspace's.
    pub(crate) fn reach(&self) -> NetworkReach {
        match self.preset {
            Some([]) => NetworkReach::DeniedByPreset,
            Some(_) => match self.workspace {
                WorkspaceEgress::Absent | WorkspaceEgress::Rules(_) => {
                    NetworkReach::PresetAllowlist
                }
                WorkspaceEgress::Empty => NetworkReach::DeniedByWorkspace,
            },
            None => match self.workspace {
                WorkspaceEgress::Absent => NetworkReach::Unrestricted,
                WorkspaceEgress::Rules(_) => NetworkReach::WorkspaceAllowlist,
                WorkspaceEgress::Empty => NetworkReach::DeniedByWorkspace,
            },
        }
    }
}

/// The workspace file's state, from the two facts the app already read.
///
/// The app reads the file once per workspace switch and per pane edit
/// (`AppState::refresh_egress_allowlist`), so a render never touches the disk.
/// The entries are classified through the policy's own parser
/// (`agent::policy::EgressRuleSet`), the same one `caps::read_egress_rules`
/// feeds, so a line cannot mean one thing to the label and another to the
/// enforcement.
pub(crate) fn workspace_egress(present: bool, entries: &[String]) -> WorkspaceEgress {
    if !present {
        return WorkspaceEgress::Absent;
    }
    let rules = agent::policy::EgressRuleSet::parse(entries);
    if rules.is_empty() {
        WorkspaceEgress::Empty
    } else {
        WorkspaceEgress::Rules(rules)
    }
}

/// One sentence stating the network reach, in `reach`'s terms.
///
/// `actors` names WHO uses the network when the mode has a class of users worth
/// naming; a row whose shell and tools share one reach passes `None` and states
/// the network itself.
fn network_sentence(reach: NetworkReach, actors: Option<String>, lang: Lang) -> String {
    let phrase = tr(lang, reach.phrase_key());
    match actors {
        Some(actors) => tr_args(
            lang,
            "permission.netActors",
            &[("actors", actors), ("reach", phrase)],
        ),
        None => capitalise(&tr_args(
            lang,
            "permission.netGeneric",
            &[("reach", phrase)],
        )),
    }
}

/// Upper-case the first character of a sentence fragment.
///
/// The generic network sentence starts with its phrase, which the shared keys
/// write in lower case for the actor form; Chinese has no case, so this is a
/// no-op there.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The three rows the permission chip draws for one resolution.
pub(crate) fn rows_for(
    lang: Lang,
    permission: &PermissionResolution,
    sandbox_heel: bool,
    network: &NetworkFacts<'_>,
) -> Vec<ModeChoice> {
    choices_with_sandbox(lang, permission, sandbox_heel, network).to_vec()
}

/// The three rows the permission chip draws for one resolved session.
///
/// The render path and the rendered test call this same function, so the test
/// reads exactly the rows the menu is built from.
pub(crate) fn permission_rows(
    lang: Lang,
    effective: &Effective,
    state: &AppState,
) -> Vec<ModeChoice> {
    let network = NetworkFacts::for_session(state, effective);
    rows_for(
        lang,
        &effective.permission,
        preset_forces_heel(effective),
        &network,
    )
}

/// Whether the PRESET in force would keep the shell confined even at Full access.
///
/// The resolved `sandbox.heel` answers for the mode in force, which is usually
/// Workspace Write - and there heel is true simply because the permission is not
/// full access. A Full access ROW must not read "this preset keeps the shell
/// confined" for a preset that declares nothing, so the row asks the preset's own
/// declaration: a forced heel, or a `sandbox = "none"` the widening guard
/// refused (the shell then stays confined at every tier).
fn preset_forces_heel(effective: &Effective) -> bool {
    effective.sandbox.declared == Some(crate::settings::global::SandboxChoice::Heel)
        || effective.sandbox.refusal.is_some()
}

/// One row of the mode menu: what it says, whether it is the current mode, and
/// whether the preset in force allows it.
#[derive(Clone, Debug)]
pub(crate) struct ModeChoice {
    /// The mode the row selects.
    pub(crate) mode: Mode,
    /// The row's single line: the label and its consequence, or why a preset
    /// keeps the session lower.
    pub(crate) text: String,
    /// Whether this row is the mode in force.
    pub(crate) checked: bool,
    /// Whether picking it can take effect. A preset that caps the session
    /// disables the modes above its ceiling, so a press cannot record a mode
    /// the preset silently overrides.
    pub(crate) enabled: bool,
}

/// The three menu rows for one resolution, without the sandbox refinement.
///
/// The checkmark follows the EFFECTIVE mode - the mode the tools obey - because
/// the chip now shows that mode and two controls must never disagree. A mode
/// above a preset's ceiling is disabled and says which mode the preset keeps,
/// so "presets only narrow" is visible where the choice is made.
///
/// Test-only: the live chip calls [`choices_with_sandbox`] because it knows
/// whether the preset keeps the shell confined; this wrapper is the plain case
/// the row tests compare against.
#[cfg(test)]
pub(crate) fn choices(
    lang: Lang,
    permission: &PermissionResolution,
    network: &NetworkFacts<'_>,
) -> [ModeChoice; 3] {
    choices_with_sandbox(lang, permission, false, network)
}

/// The same rows, told whether the PRESET keeps the shell confined.
///
/// A preset that forces `heel` must not let the Full access row read "no
/// confinement": the shell is confined even though the permission would release
/// the files. `preset_forces_heel` answers that for the row's own mode, so a
/// Workspace Write session does not make the Full access row claim a preset
/// constraint the preset never declared.
pub(crate) fn choices_with_sandbox(
    lang: Lang,
    permission: &PermissionResolution,
    preset_forces_heel: bool,
    network: &NetworkFacts<'_>,
) -> [ModeChoice; 3] {
    let effective = Mode::from_permission(&permission.effective);
    // A mode above the effective one is only unavailable when a PRESET lowered
    // it. Without a ceiling, Full access is a real choice from Workspace Write.
    let capped = permission.reduced_by.is_some();
    MODES.map(|mode| {
        let enabled = !capped || mode.rank() <= effective.rank();
        let label = mode.label(lang);
        let text = if enabled {
            // The row states the network the mode actually gets: the workspace
            // file's tri-state, narrowed by the preset in force.
            let consequence = mode.consequence(lang, preset_forces_heel, network);
            format!("{label} · {consequence}")
        } else {
            format!(
                "{label} · {}",
                tr_args(
                    lang,
                    "permission.keptShort",
                    &[("mode", effective.label(lang))]
                )
            )
        };
        ModeChoice {
            mode,
            text,
            checked: mode == effective,
            enabled,
        }
    })
}

/// The chip's own text: the mode in force, in plain words.
pub(crate) fn chip_text(lang: Lang, permission: &PermissionResolution) -> String {
    Mode::from_permission(&permission.effective).label(lang)
}

/// The preset card's mode line: the grade name and one plain sentence about
/// what the resolved mode permits.
///
/// It reads the RESOLVED permission and sandbox, never the declaration, so the
/// collapsed card cannot claim more access than a turn would get. A preset that
/// caps the session says so; a full-access session whose preset keeps the shell
/// confined says that instead of "no confinement".
pub(crate) fn card_mode_line(
    lang: Lang,
    grade: &str,
    permission: &PermissionResolution,
    sandbox: &SandboxResolution,
) -> String {
    let effective = Mode::from_permission(&permission.effective);
    // The card's line is deliberately short enough for one line at the measured
    // 330px card: the full consequence is in the mode menu, while the collapsed
    // card still answers "what may this turn do" without a paragraph.
    let sentence = if permission.reduced_by.is_some() {
        tr_args(
            lang,
            "permission.cardCapped",
            &[("mode", effective.label(lang))],
        )
    } else if effective == Mode::FullAccess && sandbox.heel {
        tr(lang, "permission.cardFullShell")
    } else {
        tr(
            lang,
            match effective {
                Mode::ReadOnly => "permission.cardReadOnly",
                Mode::WorkspaceWrite => "permission.cardWorkspace",
                Mode::FullAccess => "permission.cardFull",
            },
        )
    };
    format!("{grade} · {sentence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolution(requested: &str, ceiling: Option<&str>) -> PermissionResolution {
        PermissionResolution::resolve(requested, ceiling)
    }

    /// One rule list owned for the duration of a case.
    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|entry| entry.to_string()).collect()
    }

    /// The network facts for one workspace-file state and one preset field.
    fn facts<'a>(
        present: bool,
        entries: &'a [String],
        preset: Option<&'a [String]>,
    ) -> NetworkFacts<'a> {
        NetworkFacts {
            workspace: workspace_egress(present, entries),
            preset,
        }
    }

    /// An ABSENT file, with no preset narrowing: the unrestricted state.
    fn absent<'a>(none: &'a [String]) -> NetworkFacts<'a> {
        facts(false, none, None)
    }

    /// The three modes map onto the three tiers, and nothing else exists.
    #[test]
    fn the_three_modes_are_the_three_tiers() {
        assert_eq!(
            MODES.map(Mode::value),
            [
                crate::caps::READ_ONLY,
                crate::caps::WORKSPACE_WRITE,
                crate::caps::FULL_ACCESS,
            ]
        );
        let none: Vec<String> = Vec::new();
        let open = absent(&none);
        for mode in MODES {
            assert_eq!(Mode::from_permission(mode.value()), mode);
            assert_ne!(mode.label(Lang::En), mode.label_key());
            assert_ne!(
                mode.consequence(Lang::En, false, &open),
                mode.consequence_key()
            );
            assert_ne!(mode.label(Lang::Zh), mode.label_key());
            assert_ne!(
                mode.consequence(Lang::Zh, false, &open),
                mode.consequence_key()
            );
        }
        assert_eq!(
            Mode::from_permission("not-a-mode"),
            Mode::ReadOnly,
            "unknown text fails closed to the tier the resolution normalises to"
        );
    }

    /// The labels are the reference's own, measured from
    /// `dsh-client-ui-permission-presets`: Read Only / Workspace Write /
    /// Full access, 仅可查看 / 工作区内修改 / 完全权限. The consequence line is
    /// bsh's own plain-language addition; the NAME is never invented.
    #[test]
    fn the_mode_labels_are_the_reference_wording() {
        let en = MODES.map(|mode| mode.label(Lang::En));
        assert_eq!(en, ["Read Only", "Workspace Write", "Full access"]);
        let zh = MODES.map(|mode| mode.label(Lang::Zh));
        assert_eq!(zh, ["仅可查看", "工作区内修改", "完全权限"]);
        let none: Vec<String> = Vec::new();
        let open = absent(&none);
        for mode in MODES {
            assert!(!mode.consequence(Lang::En, false, &open).is_empty());
            assert!(!mode.consequence(Lang::Zh, false, &open).is_empty());
        }
    }

    /// The workspace file is classified in the three states the policy reads,
    /// through the policy's own parser: absent is nothing, present-with-no-rule
    /// is the deny-all floor, and present-with-rules is the allowlist.
    #[test]
    fn the_workspace_file_is_classified_in_three_states() {
        let none: Vec<String> = Vec::new();
        let comments = entries(&["# a comment only", ""]);
        let rules = entries(&["203.0.113.7"]);
        assert_eq!(workspace_egress(false, &none), WorkspaceEgress::Absent);
        assert_eq!(
            workspace_egress(true, &none),
            WorkspaceEgress::Empty,
            "a file that declares no rule is the deliberate deny-all"
        );
        assert_eq!(
            workspace_egress(true, &comments),
            WorkspaceEgress::Empty,
            "comments are not rules: the file still denies every destination"
        );
        assert!(matches!(
            workspace_egress(true, &rules),
            WorkspaceEgress::Rules(_)
        ));
    }

    /// EVERY WRITABLE row states the network, in both locales, and its clause
    /// is the resolved state: an absent file is unrestricted, a present file
    /// with no rule denies all, and a present file with rules is the workspace
    /// allowlist. A hardcoded "allowlist" passes one case and lies in the other
    /// two, which is what this discriminates against.
    ///
    /// Read Only is excluded on purpose: its row is NOT derived from the file
    /// (the session tier denies every local egress there), and its own test
    /// below pins that independent sentence.
    #[test]
    fn every_row_states_the_network_truth_for_the_three_file_states() {
        let none: Vec<String> = Vec::new();
        let rules = entries(&["203.0.113.7"]);
        // present, entries, EN fragment, ZH fragment, is the reach permitted.
        let cases: [(bool, &[String], &str, &str, bool); 3] = [
            (
                false,
                &none,
                "network available, unrestricted",
                "网络可用，不受限",
                true,
            ),
            (
                true,
                &none,
                "network denied by the workspace allowlist",
                "网络被工作区白名单拒绝",
                false,
            ),
            (
                true,
                &rules,
                "network available, bounded by the workspace allowlist",
                "网络可用，受工作区白名单约束",
                true,
            ),
        ];
        for mode in [Mode::WorkspaceWrite, Mode::FullAccess] {
            for lang in [Lang::En, Lang::Zh] {
                // The word for an unrestricted network: a deny-all state may
                // not use it, which is what the negative half discriminates.
                let (word, free, denied) = match lang {
                    Lang::En => ("network", "unrestricted", "network denied"),
                    Lang::Zh => ("网络", "不受限", "网络被"),
                };
                for (present, list, en, zh, permitted) in cases {
                    let facts = facts(present, list, None);
                    let line = mode.consequence(lang, false, &facts);
                    let expected = if lang == Lang::En { en } else { zh };
                    assert!(
                        line.contains(word),
                        "{mode:?} in {lang:?} says nothing about the network: {line}"
                    );
                    assert!(
                        line.contains(expected),
                        "{mode:?} in {lang:?} must say {expected:?}: {line}"
                    );
                    assert_eq!(
                        facts.reach().permitted(),
                        permitted,
                        "{mode:?} resolved the wrong reach"
                    );
                    if permitted {
                        assert!(
                            !line.contains(denied),
                            "{mode:?} in {lang:?} claims a denial the state does not have: {line}"
                        );
                    } else {
                        assert!(
                            !line.contains(free),
                            "{mode:?} in {lang:?} claims an open network a deny-all file forbids: {line}"
                        );
                    }
                }
            }
        }
    }

    /// NEGATIVE: a WRITABLE row governed by a deny-all preset neither reads as
    /// an open network nor hides the preset behind the workspace file. Both
    /// workspace states are exercised, because the preset wins either way.
    /// Read Only is exempt because it never states a derived reach: its row
    /// refuses the local network itself, which is pinned separately.
    #[test]
    fn a_deny_all_preset_is_named_on_every_row() {
        let none: Vec<String> = Vec::new();
        let rules = entries(&["203.0.113.7"]);
        let deny: Vec<String> = Vec::new();
        for lang in [Lang::En, Lang::Zh] {
            let (free, denied, denier) = match lang {
                Lang::En => (
                    "unrestricted",
                    "network denied",
                    "network denied by the preset",
                ),
                Lang::Zh => ("不受限", "网络被", "网络被预设拒绝"),
            };
            for (present, list) in [(false, &none), (true, &rules)] {
                let facts = facts(present, list, Some(&deny));
                for mode in [Mode::WorkspaceWrite, Mode::FullAccess] {
                    let line = mode.consequence(lang, false, &facts);
                    assert!(
                        line.contains(denied),
                        "{mode:?} in {lang:?} must state the deny: {line}"
                    );
                    assert!(
                        line.contains(denier),
                        "{mode:?} in {lang:?} must name the preset as the denier: {line}"
                    );
                    assert!(
                        !line.contains(free),
                        "{mode:?} in {lang:?} claims an unrestricted network a deny-all preset forbids: {line}"
                    );
                }
            }
        }
    }

    /// A preset's own list is the bound on every WRITABLE row, intersected with
    /// the workspace's: entries narrow, an empty workspace file still wins.
    #[test]
    fn a_preset_list_bounds_every_row_and_intersects_the_workspace() {
        let none: Vec<String> = Vec::new();
        let rules = entries(&["203.0.113.7"]);
        let preset = entries(&["api.example.com"]);
        for (present, list) in [(false, &none), (true, &rules)] {
            let facts = facts(present, list, Some(&preset));
            assert_eq!(facts.reach(), NetworkReach::PresetAllowlist);
            for lang in [Lang::En, Lang::Zh] {
                let expected = if lang == Lang::En {
                    "network available, bounded by the preset allowlist"
                } else {
                    "网络可用，受预设白名单约束"
                };
                for mode in [Mode::WorkspaceWrite, Mode::FullAccess] {
                    let line = mode.consequence(lang, false, &facts);
                    assert!(
                        line.contains(expected),
                        "{mode:?} in {lang:?} must say {expected:?}: {line}"
                    );
                }
            }
        }
        let denied_by_file = facts(true, &none, Some(&preset));
        assert_eq!(
            denied_by_file.reach(),
            NetworkReach::DeniedByWorkspace,
            "a workspace deny-all wins over a preset list: egress is intersected, never unioned"
        );
    }

    /// The Read Only row states the SESSION tier's refusal, not a reach.
    ///
    /// `session-read-only-egress` (`SESSION_PERMISSION_REGO` in
    /// `agent::policy`) refuses every local egress under read-only, so the
    /// browser's navigation, page fetches and MCP HTTP open no socket. Hosted
    /// web search runs on the model provider and touches no local network, so
    /// it is the one thing still available - and the row must say exactly that.
    ///
    /// This pins the pair: hosted search named as available, the local network
    /// named as refused - and the negative that the row carries NO workspace
    /// allowlist phrase, because that phrase describes the reach a WRITABLE
    /// tier gets. It also pins non-derivation: the same row in every workspace
    /// and preset egress state, since neither can widen a read-only session.
    #[test]
    fn the_read_only_row_states_the_session_refusal_not_an_allowlist() {
        let none: Vec<String> = Vec::new();
        let rules = entries(&["203.0.113.7"]);
        let preset = entries(&["api.example.com"]);
        for lang in [Lang::En, Lang::Zh] {
            let (hosted, refused, browser, pages, mcp, allowlist, open, actor_form) = match lang {
                Lang::En => (
                    "Hosted web search works",
                    "is refused by Read Only",
                    "the browser",
                    "page fetches",
                    "MCP HTTP",
                    "workspace allowlist",
                    "network available",
                    "browser: network",
                ),
                Lang::Zh => (
                    "托管网页搜索可用",
                    "仅可查看拒绝本地网络",
                    "浏览器",
                    "页面抓取",
                    "MCP HTTP",
                    "工作区白名单",
                    "网络可用",
                    "浏览器：网络",
                ),
            };
            let resolved = resolution(crate::caps::READ_ONLY, None);
            let mut lines: Vec<String> = Vec::new();
            for (present, list, preset_list) in [
                (false, &none, None),
                (true, &none, None),
                (true, &rules, None),
                (false, &none, Some(preset.as_slice())),
                (true, &rules, Some(preset.as_slice())),
            ] {
                let facts = facts(present, list, preset_list);
                let rows = choices_with_sandbox(lang, &resolved, false, &facts);
                let row = rows
                    .iter()
                    .find(|row| row.mode == Mode::ReadOnly)
                    .expect("the Read Only row");
                lines.push(row.text.clone());
            }
            let line = &lines[0];
            for other in &lines[1..] {
                assert_eq!(
                    other, line,
                    "Read Only must not derive its reach from the workspace or preset: {other}"
                );
            }
            assert!(
                line.starts_with(&Mode::ReadOnly.label(lang)),
                "the head phrase keeps the mode label: {line}"
            );
            assert!(
                line.contains(&tr(lang, "permission.consequenceReadOnly")),
                "and the existing head sentence: {line}"
            );
            assert!(
                line.contains(hosted),
                "{lang:?} must name hosted search as available: {line}"
            );
            assert!(
                line.contains(refused),
                "{lang:?} must name Read Only as refusing the local network: {line}"
            );
            for named in [browser, pages, mcp] {
                assert!(
                    line.contains(named),
                    "{lang:?} must name {named:?} as refused, not omit it: {line}"
                );
            }
            assert!(
                !line.contains(allowlist),
                "{lang:?} must not carry the workspace-allowlist phrase for local traffic: {line}"
            );
            assert!(
                !line.contains(open),
                "{lang:?} must not claim the local network is available: {line}"
            );
            assert!(
                !line.contains(actor_form),
                "{lang:?} must not keep the old actor clause that claimed reach: {line}"
            );
        }
    }

    /// Every row carries a one-line consequence, and the current row is the
    /// effective mode rather than a request a preset already narrowed.
    #[test]
    fn the_rows_mark_the_effective_mode() {
        let none: Vec<String> = Vec::new();
        let open = absent(&none);
        let plain = choices(
            Lang::En,
            &resolution(crate::caps::WORKSPACE_WRITE, None),
            &open,
        );
        assert_eq!(plain.len(), 3);
        let checked: Vec<&str> = plain
            .iter()
            .filter(|choice| choice.checked)
            .map(|choice| choice.mode.value())
            .collect();
        assert_eq!(checked, [crate::caps::WORKSPACE_WRITE]);
        for choice in &plain {
            assert!(
                choice.enabled,
                "without a ceiling every mode is available: {choice:?}"
            );
            assert!(
                choice.text.contains('·'),
                "each row pairs the label with a consequence: {}",
                choice.text
            );
            assert!(
                choice.text.contains("network"),
                "and each row states the network: {}",
                choice.text
            );
        }

        let capped = choices(
            Lang::En,
            &resolution(crate::caps::FULL_ACCESS, Some(crate::caps::READ_ONLY)),
            &open,
        );
        let marked: Vec<&str> = capped
            .iter()
            .filter(|choice| choice.checked)
            .map(|choice| choice.mode.value())
            .collect();
        assert_eq!(
            marked,
            [crate::caps::READ_ONLY],
            "a capped chat marks the mode the tools obey, not the request"
        );
        assert!(
            !capped[1].enabled,
            "a preset ceiling disables the rows above it"
        );
        assert!(!capped[2].enabled);
        assert!(
            capped[2].text.contains("Read Only"),
            "the disabled row says which mode the preset keeps: {}",
            capped[2].text
        );
    }

    /// A preset can only narrow: the effective mode never ranks above the
    /// requested one, for every ceiling the settings accept.
    #[test]
    fn a_preset_never_raises_the_mode() {
        for requested in MODES {
            for ceiling in [
                None,
                Some(crate::caps::READ_ONLY),
                Some(crate::caps::WORKSPACE_WRITE),
                Some(crate::caps::FULL_ACCESS),
            ] {
                let resolved = resolution(requested.value(), ceiling);
                assert!(
                    Mode::from_permission(&resolved.effective).rank() <= requested.rank(),
                    "{requested:?} with ceiling {ceiling:?} resolved to {}",
                    resolved.effective
                );
            }
        }
    }

    /// The card's line reads the resolved facts: a capped session says so, and
    /// a full-access session whose preset keeps the shell confined does not
    /// claim "no confinement" - and its menu row names both the shell and the
    /// network reach.
    #[test]
    fn the_card_line_is_true_for_the_resolved_scope() {
        let workspace = resolution(crate::caps::WORKSPACE_WRITE, None);
        let sandbox = SandboxResolution::resolve(None, None, &workspace.effective);
        let line = card_mode_line(Lang::En, "Standard", &workspace, &sandbox);
        assert_eq!(
            line, "Standard · Confined to this workspace.",
            "the mode line states the resolved mode"
        );

        let full = resolution(crate::caps::FULL_ACCESS, None);
        let heel = SandboxResolution::resolve(
            Some(crate::settings::global::SandboxChoice::Heel),
            None,
            &full.effective,
        );
        let line = card_mode_line(Lang::En, "Elevated", &full, &heel);
        assert_eq!(
            line, "Elevated · Files open; shell confined.",
            "a forced heel is named instead of claiming no confinement: {line}"
        );
        let none: Vec<String> = Vec::new();
        let open = absent(&none);
        let rows = choices_with_sandbox(Lang::En, &full, true, &open);
        let full_row = rows
            .iter()
            .find(|row| row.mode == Mode::FullAccess)
            .expect("the Full access row");
        assert!(
            full_row.text.contains("keeps the shell confined"),
            "the menu row says the same thing as the card: {}",
            full_row.text
        );
        assert!(
            full_row.text.contains("Network available, unrestricted."),
            "a confined full-access shell gets the resolved reach: {}",
            full_row.text
        );

        let capped = resolution(crate::caps::FULL_ACCESS, Some(crate::caps::WORKSPACE_WRITE));
        let sandbox = SandboxResolution::resolve(None, None, &capped.effective);
        let line = card_mode_line(Lang::En, "Standard", &capped, &sandbox);
        assert!(
            line.contains("Preset keeps Workspace Write"),
            "the cap is on the collapsed card too: {line}"
        );
    }
}
