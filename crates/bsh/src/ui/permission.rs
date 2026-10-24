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
//! # Two lengths of the same truth
//!
//! The composer MENU stays scannable: one row per mode, the reference's label
//! plus a deliberately short qualifier ("Writes stay in this workspace."), no
//! second line and no parenthetical. [`Mode::qualifier`] is that copy.
//!
//! The DETAILED sentence lives where the reader is choosing a default or has
//! already opened a preset: [`Mode::consequence`] feeds the Settings pane's
//! mode select, and the preset card carries its own short form. That sentence
//! is DERIVED, because a mode line that described files and commands but said
//! nothing about the network would be false by omission: Workspace Write can
//! reach the network, while Read Only cannot write and its session tier refuses
//! local egress. The clause for a WRITABLE mode is built from the resolved
//! egress state at the moment the sentence is drawn:
//!
//! - the workspace's `.bsh/egress-allowlist` is TRI-STATE - ABSENT leaves the
//!   network unrestricted, PRESENT with no rule is a deliberate deny-all, and
//!   PRESENT with rules is the allowlist;
//! - a preset's own `egress` field intersects (never unions) with that state;
//!   `egress = []` denies every destination, entries narrow the reach.
//!
//! The classification goes through the policy's own parser, so a sentence
//! cannot claim an "unrestricted" network a deny-all preset forbids, nor a
//! "bounded" one when no file and no preset narrows anything.
//!
//! Read Only is the ONE exception, and it is deliberately not derived: the
//! session tier (`session-read-only-egress` in
//! `agent::policy::SESSION_PERMISSION_REGO`) refuses every local egress at that
//! mode, so no workspace allowlist can make a local socket reachable. Its
//! detailed sentence states that refusal and names hosted web search - which
//! runs on the model provider and opens no local socket - as the one thing
//! still available.
//!
//! The sandbox state is deliberately absent from the menu rows: it is already
//! visible in the Settings switch row, the terminal status line and each call's
//! audit, and a row cannot mention it without outgrowing the scannable shape.

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

    /// The locale key of the SHORT qualifier a menu row carries.
    ///
    /// Read Only's qualifier is the same sentence its detailed form starts
    /// with, so it reuses that key; the two writable modes get a dedicated
    /// one-liner. None names the network, the sandbox, the proxy or the
    /// browser: those facts belong to the detailed sentence and to the
    /// surfaces that already show them, and a row that repeated them stopped
    /// being scannable.
    pub(crate) fn qualifier_key(self) -> &'static str {
        match self {
            Self::ReadOnly => "permission.consequenceReadOnly",
            Self::WorkspaceWrite => "permission.shortWorkspace",
            Self::FullAccess => "permission.shortFull",
        }
    }

    /// The label in the active language.
    pub(crate) fn label(self, lang: Lang) -> String {
        tr(lang, self.label_key())
    }

    /// The short qualifier in the active language.
    pub(crate) fn qualifier(self, lang: Lang) -> String {
        tr(lang, self.qualifier_key())
    }

    /// The DETAILED consequence in the active language, network included.
    ///
    /// This is the Settings pane's mode select and the preset card's sentence,
    /// not the composer menu row: the menu shows [`Self::qualifier`], because a
    /// row that repeated this sentence stopped being scannable.
    ///
    /// The filesystem and command half is the mode's own sentence; the network
    /// half of a WRITABLE mode is built from `network` so the sentence cannot
    /// state a reach the turn does not get. Read Only is deliberately NOT
    /// derived:
    /// its local egress is refused by the session tier, so no workspace
    /// allowlist can make the browser, a page fetch or MCP HTTP reachable.
    /// Under Full access a preset that keeps the shell confined swaps in the
    /// sentence that names that constraint, because it is the fact the reader
    /// cannot otherwise see.
    ///
    /// `release` is the SAME resolved sandbox fact the preset cards read. It
    /// changes the writable sentences because a released shell is not confined
    /// by the workspace and is not behind the egress proxy either: a sentence
    /// that still said "writes and commands stay in this workspace" or that
    /// named the allowlist as the bound would describe a confinement half the
    /// turn does not have.
    pub(crate) fn consequence(
        self,
        lang: Lang,
        preset_forces_heel: bool,
        release: ShellRelease,
        network: &NetworkFacts<'_>,
    ) -> String {
        let released = release != ShellRelease::None;
        let (head, clause) = match self {
            // No shell exists at this mode, so a release changes nothing and
            // the sentence keeps stating the session tier's own egress refusal.
            Self::ReadOnly => (self.consequence_key(), tr(lang, "permission.netReadOnly")),
            // The released arm: the files half is still true (the tool boundary
            // and its policy remain), the command half is not, and no proxy
            // consulted the workspace allowlist.
            Self::WorkspaceWrite if released => (
                "permission.consequenceWorkspaceSandboxOff",
                tr(lang, "permission.netCommandsUnconfined"),
            ),
            // A full-access shell that a preset keeps confined names the reach
            // the shell and the tools share.
            Self::FullAccess if preset_forces_heel => (
                "permission.consequenceFullShell",
                network_sentence(network.reach(), None, lang),
            ),
            // An unconfined full-access shell: "bypasses the allowlist" was the
            // old copy, but with no proxy the allowlist was never consulted.
            Self::FullAccess if released => (
                "permission.consequenceFullSandboxOff",
                tr(lang, "permission.netCommandsUnconfined"),
            ),
            Self::FullAccess => (
                self.consequence_key(),
                network_sentence(
                    network.reach(),
                    Some(tr(lang, "permission.netActorsTools")),
                    lang,
                ),
            ),
            Self::WorkspaceWrite => (
                self.consequence_key(),
                network_sentence(
                    network.reach(),
                    Some(tr(lang, "permission.netActorsCommands")),
                    lang,
                ),
            ),
        };
        // English joins two sentences with a space; Chinese punctuation carries
        // its own spacing, so the full stops must touch.
        let separator = if lang == Lang::En { " " } else { "" };
        format!("{}{separator}{clause}", tr(lang, head))
    }
}

/// Whether the settings master switch released the shell.
///
/// The permission's own release is NOT a variant: at full access with no policy
/// asking for heel the shell is released by the tier, which is the ordinary
/// full-access state rather than a policy choice. The DETAILED sentence only
/// needs the settings arm; the cards read the resolved sandbox fields directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellRelease {
    /// Confinement is not released by a policy.
    None,
    /// The settings master switch turned heel off.
    Settings,
}

/// The network reach the DETAILED sentence must state.
///
/// Derived from the two facts that set it - the workspace file's tri-state and
/// the preset's resolved `egress` field - because the sentence cannot claim
/// more reach than a turn would get. The vocabulary is the policy's own: absent
/// file
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
    /// The tests read it to discriminate a permitted state from a deny-all one;
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

/// The three rows the permission chip draws for one resolved session.
///
/// The render path and the rendered test call this same function, so the test
/// reads exactly the rows the menu is built from. Only the RESOLVED permission
/// is read: the menu makes no egress or sandbox claim, so a frame no longer
/// classifies the workspace allowlist for copy it does not show.
pub(crate) fn permission_rows(lang: Lang, effective: &Effective) -> Vec<ModeChoice> {
    rows(lang, &effective.permission).to_vec()
}

/// One row of the mode menu: what it says, whether it is the current mode, and
/// whether the preset in force allows it.
#[derive(Clone, Debug)]
pub(crate) struct ModeChoice {
    /// The mode the row selects.
    pub(crate) mode: Mode,
    /// The row's single line: the label and a qualifier short enough that the
    /// three modes can be compared at a glance, or why a preset keeps the
    /// session lower. The detailed sentence is NOT here; see
    /// [`Mode::qualifier`] and [`Mode::consequence`].
    pub(crate) text: String,
    /// Whether this row is the mode in force.
    pub(crate) checked: bool,
    /// Whether picking it can take effect. A preset that caps the session
    /// disables the modes above its ceiling, so a press cannot record a mode
    /// the preset silently overrides.
    pub(crate) enabled: bool,
}

/// The three menu rows: label plus SHORT qualifier, current mode marked.
///
/// The checkmark follows the EFFECTIVE mode - the mode the tools obey - because
/// the chip shows that mode and two controls must never disagree. A mode above
/// a preset's ceiling is disabled and states which mode the preset keeps, so
/// "presets only narrow" is visible where the choice is made; that tail stays
/// short for the same reason the enabled rows do.
///
/// The menu deliberately takes no sandbox or egress facts: the rows it draws
/// cannot mention either, and the DETAILED sentence that does
/// ([`Mode::consequence`]) is read in the Settings pane and on the card.
pub(crate) fn rows(lang: Lang, permission: &PermissionResolution) -> [ModeChoice; 3] {
    let effective = Mode::from_permission(&permission.effective);
    // A mode above the effective one is only unavailable when a PRESET lowered
    // it. Without a ceiling, Full access is a real choice from Workspace Write.
    let capped = permission.reduced_by.is_some();
    MODES.map(|mode| {
        let enabled = !capped || mode.rank() <= effective.rank();
        let qualifier = if enabled {
            mode.qualifier(lang)
        } else {
            tr_args(
                lang,
                "permission.keptShort",
                &[("mode", effective.label(lang))],
            )
        };
        ModeChoice {
            mode,
            text: format!("{} · {qualifier}", mode.label(lang)),
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
    //
    // While the master switch is off, EVERY card carries the released-shell
    // clause. A preset that declares no sandbox overrides nothing, so its card
    // shows no override line - and without this clause it would say nothing at
    // all about the shell, which is exactly the card that implies a confinement
    // that is not applied. Read Only says it has no shell to release; the
    // writable cards name the half that survived and the half that did not.
    let sentence = if permission.reduced_by.is_some() {
        if sandbox.switch_off {
            if effective == Mode::ReadOnly {
                tr_args(
                    lang,
                    "permission.cardCappedSandboxOffNoShell",
                    &[("mode", effective.label(lang))],
                )
            } else {
                tr_args(
                    lang,
                    "permission.cardCappedSandboxOff",
                    &[("mode", effective.label(lang))],
                )
            }
        } else {
            tr_args(
                lang,
                "permission.cardCapped",
                &[("mode", effective.label(lang))],
            )
        }
    } else if sandbox.switch_off {
        match effective {
            Mode::ReadOnly => tr(lang, "permission.cardReadOnlySandboxOff"),
            Mode::WorkspaceWrite => tr(lang, "permission.cardWorkspaceSandboxOff"),
            // Three different controls can leave a full-access shell
            // unconfined; the card names the one that applies rather than an
            // anonymous "sandbox off".
            Mode::FullAccess if sandbox.off_by_preset => tr(lang, "permission.cardFullPresetOff"),
            Mode::FullAccess if sandbox.off_by_settings => {
                tr(lang, "permission.cardFullSettingsOff")
            }
            Mode::FullAccess => tr(lang, "permission.cardFullReleased"),
        }
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
                mode.consequence(Lang::En, false, ShellRelease::None, &open),
                mode.consequence_key()
            );
            assert_ne!(mode.label(Lang::Zh), mode.label_key());
            assert_ne!(
                mode.consequence(Lang::Zh, false, ShellRelease::None, &open),
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
            assert!(!mode
                .consequence(Lang::En, false, ShellRelease::None, &open)
                .is_empty());
            assert!(!mode
                .consequence(Lang::Zh, false, ShellRelease::None, &open)
                .is_empty());
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

    /// EVERY WRITABLE detailed sentence states the network, in both locales, and
    /// its clause is the resolved state: an absent file is unrestricted, a
    /// present file with no rule denies all, and a present file with rules is
    /// the workspace allowlist. A hardcoded "allowlist" passes one case and lies
    /// in the other two, which is what this discriminates against.
    ///
    /// This is the sentence the Settings pane's mode select reads; the composer
    /// MENU row deliberately carries none of it (see the short-row test below).
    /// Read Only is excluded on purpose: its sentence is NOT derived from the
    /// file (the session tier denies every local egress there), and its own test
    /// below pins that independent sentence.
    #[test]
    fn the_detailed_sentence_states_the_network_truth_for_the_three_file_states() {
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
                    let line = mode.consequence(lang, false, ShellRelease::None, &facts);
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

    /// NEGATIVE: a WRITABLE detailed sentence governed by a deny-all preset
    /// neither reads as an open network nor hides the preset behind the
    /// workspace file. Both workspace states are exercised, because the preset
    /// wins either way. Read Only is exempt because it never states a derived
    /// reach: its sentence refuses the local network itself, which is pinned
    /// separately.
    #[test]
    fn a_deny_all_preset_is_named_in_the_detailed_sentence() {
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
                    let line = mode.consequence(lang, false, ShellRelease::None, &facts);
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

    /// A preset's own list is the bound on every WRITABLE detailed sentence,
    /// intersected with the workspace's: entries narrow, an empty workspace
    /// file still wins.
    #[test]
    fn a_preset_list_bounds_the_detailed_sentence_and_intersects_the_workspace() {
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
                    let line = mode.consequence(lang, false, ShellRelease::None, &facts);
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

    /// The Read Only DETAILED sentence states the SESSION tier's refusal, not a
    /// reach; the MENU row is the short qualifier and says none of it.
    ///
    /// `session-read-only-egress` (`SESSION_PERMISSION_REGO` in
    /// `agent::policy`) refuses every local egress under read-only, so the
    /// browser's navigation, page fetches, MCP HTTP and the local web-search
    /// tool all open no socket. The Settings sentence names search as refused
    /// too, because search is local access since the tool moved in-process.
    ///
    /// This pins the sentence the Settings pane reads, and pins non-derivation:
    /// the same sentence in every workspace and preset egress state, since
    /// neither can widen a read-only session. The menu row is asserted to stay
    /// short in both locales, in the same states.
    #[test]
    fn the_read_only_consequence_states_the_session_refusal_not_an_allowlist() {
        let none: Vec<String> = Vec::new();
        let rules = entries(&["203.0.113.7"]);
        let preset = entries(&["api.example.com"]);
        for lang in [Lang::En, Lang::Zh] {
            let (hosted, refused, browser, pages, mcp, allowlist, open, actor_form) = match lang {
                Lang::En => (
                    "web search",
                    "is refused by Read Only",
                    "the browser",
                    "page fetches",
                    "MCP HTTP",
                    "workspace allowlist",
                    "network available",
                    "browser: network",
                ),
                Lang::Zh => (
                    "网页搜索",
                    "拒绝本地网络",
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
                lines.push(Mode::ReadOnly.consequence(lang, false, ShellRelease::None, &facts));
            }
            let line = &lines[0];
            for other in &lines[1..] {
                assert_eq!(
                    other, line,
                    "Read Only must not derive its reach from the workspace or preset: {other}"
                );
            }
            assert!(
                line.starts_with(&tr(lang, "permission.consequenceReadOnly")),
                "the detailed sentence starts with the short qualifier: {line}"
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

            // The menu row is the short qualifier in every one of those states.
            let menu = rows(lang, &resolved);
            let row = menu
                .iter()
                .find(|row| row.mode == Mode::ReadOnly)
                .expect("the Read Only row");
            assert_eq!(
                row.text,
                format!(
                    "{} · {}",
                    Mode::ReadOnly.label(lang),
                    Mode::ReadOnly.qualifier(lang)
                ),
                "{lang:?} menu row stays scannable"
            );
            assert!(
                !row.text.contains(refused) && !row.text.contains(hosted),
                "{lang:?} menu row must not carry the detailed refusal: {}",
                row.text
            );
        }
    }

    /// Every row carries a label and its SHORT qualifier, and the current row is
    /// the effective mode rather than a request a preset already narrowed.
    #[test]
    fn the_rows_mark_the_effective_mode() {
        let plain = rows(Lang::En, &resolution(crate::caps::WORKSPACE_WRITE, None));
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
                "each row pairs the label with a qualifier: {}",
                choice.text
            );
            assert!(
                choice.text.contains(&choice.mode.qualifier(Lang::En)),
                "and the qualifier is the short one: {}",
                choice.text
            );
            assert!(
                !choice.text.contains("network"),
                "the menu states no network reach: {}",
                choice.text
            );
        }

        let capped = rows(
            Lang::En,
            &resolution(crate::caps::FULL_ACCESS, Some(crate::caps::READ_ONLY)),
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
    /// claim "no confinement". The companion menu row stays the short qualifier
    /// even when the card has a confinement story to tell.
    #[test]
    fn the_card_line_is_true_for_the_resolved_scope() {
        let workspace = resolution(crate::caps::WORKSPACE_WRITE, None);
        let sandbox = SandboxResolution::resolve(true, None, None, &workspace.effective);
        let line = card_mode_line(Lang::En, "Standard", &workspace, &sandbox);
        assert_eq!(
            line, "Standard · Confined to this workspace.",
            "the mode line states the resolved mode"
        );

        let full = resolution(crate::caps::FULL_ACCESS, None);
        let heel = SandboxResolution::resolve(
            true,
            Some(crate::settings::global::SandboxChoice::Heel),
            None,
            &full.effective,
        );
        let line = card_mode_line(Lang::En, "Elevated", &full, &heel);
        assert_eq!(
            line, "Elevated · Files open; shell confined.",
            "a forced heel is named instead of claiming no confinement: {line}"
        );
        let menu = rows(Lang::En, &full);
        let full_row = menu
            .iter()
            .find(|row| row.mode == Mode::FullAccess)
            .expect("the Full access row");
        assert_eq!(
            full_row.text, "Full access · Writes anywhere.",
            "the menu row stays scannable; the card above carries the confinement"
        );

        let capped = resolution(crate::caps::FULL_ACCESS, Some(crate::caps::WORKSPACE_WRITE));
        let sandbox = SandboxResolution::resolve(true, None, None, &capped.effective);
        let line = card_mode_line(Lang::En, "Standard", &capped, &sandbox);
        assert!(
            line.contains("Preset keeps Workspace Write"),
            "the cap is on the collapsed card too: {line}"
        );
    }

    /// The master switch changes what the card may claim: a Workspace Write
    /// card under it must not say the COMMANDS stay in the workspace, because
    /// heel is what confined them and the switch released it. The files half is
    /// unchanged, so the sentence keeps it.
    #[test]
    fn the_card_line_states_a_released_shell_when_the_switch_is_off() {
        let workspace = resolution(crate::caps::WORKSPACE_WRITE, None);
        let on = SandboxResolution::resolve(true, None, None, &workspace.effective);
        assert_eq!(
            card_mode_line(Lang::En, "Standard", &workspace, &on),
            "Standard · Confined to this workspace."
        );
        let off = SandboxResolution::resolve(false, None, None, &workspace.effective);
        let line = card_mode_line(Lang::En, "Standard", &workspace, &off);
        assert!(
            line.contains("Writes confined to this workspace"),
            "the files half is still stated: {line}"
        );
        assert!(
            line.contains("shell is unconfined") && line.contains("settings"),
            "and the half it lost is named, not omitted: {line}"
        );
        assert!(
            !line.contains("Confined to this workspace."),
            "the old all-confined sentence would be a false claim: {line}"
        );
        assert_ne!(
            card_mode_line(Lang::Zh, "Standard", &workspace, &off),
            line,
            "the sentence is localized"
        );
        // Read Only registers no shell, but its card must still say so rather
        // than fall silent while the switch is off: silence is the card that
        // implies a confinement that is not applied.
        let ro = resolution(crate::caps::READ_ONLY, None);
        let ro_off = SandboxResolution::resolve(false, None, None, &ro.effective);
        let ro_line = card_mode_line(Lang::En, "Read Only", &ro, &ro_off);
        assert!(
            ro_line.contains("no shell"),
            "Read Only names the half there is nothing to release: {ro_line}"
        );
        assert!(
            ro_line.contains("terminal is separate"),
            "and says the reader's terminal pane is a different seam: {ro_line}"
        );
        assert_ne!(
            ro_line,
            card_mode_line(
                Lang::En,
                "Read Only",
                &ro,
                &SandboxResolution::resolve(true, None, None, &ro.effective)
            ),
            "and it is not the switch-on line"
        );

        // Full access attributes its release to the control that made it. The
        // three arms are separate sentences because each is reversed by a
        // different control.
        let full = resolution(crate::caps::FULL_ACCESS, None);
        let released = SandboxResolution::resolve(false, None, None, &full.effective);
        let released_line = card_mode_line(Lang::En, "Elevated", &full, &released);
        assert!(
            released_line.contains("full access released the shell"),
            "the switch changed nothing, so the release keeps its own attribution: {released_line}"
        );
        let preset_off = SandboxResolution::resolve(
            false,
            Some(crate::settings::global::SandboxChoice::Off),
            Some(crate::caps::FULL_ACCESS),
            &full.effective,
        );
        let preset_line = card_mode_line(Lang::En, "Elevated", &full, &preset_off);
        assert!(
            preset_line.contains("this preset turned the sandbox off"),
            "a preset's own widening is named as its own: {preset_line}"
        );
        let heel = SandboxResolution::resolve(
            true,
            Some(crate::settings::global::SandboxChoice::Heel),
            Some(crate::caps::FULL_ACCESS),
            &full.effective,
        );
        let overridden = SandboxResolution::resolve(
            false,
            Some(crate::settings::global::SandboxChoice::Heel),
            Some(crate::caps::FULL_ACCESS),
            &full.effective,
        );
        assert!(heel.heel && overridden.overrides_preset());
        let settings_line = card_mode_line(Lang::En, "Elevated", &full, &overridden);
        assert!(
            settings_line.contains("settings"),
            "the switch overriding a heel preset claims the switch: {settings_line}"
        );
        for (line, other) in [
            (&released_line, &preset_line),
            (&preset_line, &settings_line),
            (&settings_line, &released_line),
        ] {
            assert_ne!(line, other, "two release arms read alike: {line} / {other}");
        }
    }

    /// The MENU is a choice, not a briefing: each row is the reference's label
    /// and one very short qualifier, in both locales, with no network, egress,
    /// sandbox, proxy, MCP, browser, shell or terminal clause and no second line.
    ///
    /// The 48-character bound is the owner's "label and a very short qualifier"
    /// made testable: the longest target row ("Workspace Write · Writes stay in
    /// this workspace.") is exactly 48, while the rows that shipped before this
    /// copy round ran past 100. A row that would need a qualifier to stay
    /// truthful drops the qualifier instead of growing: the truth is in the
    /// Settings pane and on the preset card.
    #[test]
    fn the_permission_rows_stay_a_label_and_a_short_qualifier() {
        const ROW_MAX_CHARS: usize = 48;
        for (lang, expected) in [
            (
                Lang::En,
                [
                    "Read Only · Nothing written, no commands.",
                    "Workspace Write · Writes stay in this workspace.",
                    "Full access · Writes anywhere.",
                ],
            ),
            (
                Lang::Zh,
                [
                    "仅可查看 · 不写入，不执行命令。",
                    "工作区内修改 · 写入限于此工作区。",
                    "完全权限 · 可写入任意位置。",
                ],
            ),
        ] {
            let resolved = resolution(crate::caps::WORKSPACE_WRITE, None);
            let menu = rows(lang, &resolved);
            let texts: Vec<&str> = menu.iter().map(|row| row.text.as_str()).collect();
            assert_eq!(
                texts, expected,
                "{lang:?} rows are the label and the short qualifier"
            );
            for row in &menu {
                assert!(
                    row.text.chars().count() <= ROW_MAX_CHARS,
                    "{lang:?} row is longer than {ROW_MAX_CHARS}: {}",
                    row.text
                );
                assert!(!row.text.contains('\n'), "one line: {}", row.text);
                assert_eq!(
                    row.text.matches(" · ").count(),
                    1,
                    "one label separator: {}",
                    row.text
                );
                let lowered = row.text.to_lowercase();
                for banned in [
                    "network",
                    "egress",
                    "sandbox",
                    "proxy",
                    "mcp",
                    "browser",
                    "allowlist",
                    "shell",
                    "terminal",
                    "网络",
                    "白名单",
                    "沙箱",
                    "代理",
                    "浏览器",
                    "终端",
                ] {
                    assert!(
                        !lowered.contains(banned),
                        "{lang:?} row names {banned:?}: {}",
                        row.text
                    );
                }
            }
        }

        // A capped row keeps the same shape: the tail says which mode the
        // preset keeps, without turning into a sentence.
        let capped = rows(
            Lang::En,
            &resolution(crate::caps::FULL_ACCESS, Some(crate::caps::READ_ONLY)),
        );
        for row in &capped {
            assert!(
                row.text.chars().count() <= ROW_MAX_CHARS,
                "capped row is longer than {ROW_MAX_CHARS}: {}",
                row.text
            );
        }
    }

    /// The released-shell truth survived the copy round, in the DETAILED
    /// sentence the Settings pane reads; the MENU row stays short.
    ///
    /// With the master switch off, a Workspace Write session is still confined
    /// for files but not for commands, and no proxy consulted the allowlist.
    /// That is the sentence `permission_option_text` draws in the pane; it must
    /// not creep back into the menu rows.
    #[test]
    fn the_released_shell_lives_in_the_detailed_sentence_not_the_menu() {
        let workspace = resolution(crate::caps::WORKSPACE_WRITE, None);
        let none: Vec<String> = Vec::new();
        let open = absent(&none);

        let detailed =
            Mode::WorkspaceWrite.consequence(Lang::En, false, ShellRelease::Settings, &open);
        assert!(
            detailed.contains("Writes stay in this workspace"),
            "the files half is still stated: {detailed}"
        );
        assert!(
            detailed.contains("shell and terminal run unconfined"),
            "and the released half is named, not omitted: {detailed}"
        );
        assert!(
            detailed.contains("not bounded by the workspace egress allowlist"),
            "the network clause stops claiming a proxy that is not there: {detailed}"
        );
        let zh = Mode::WorkspaceWrite.consequence(Lang::Zh, false, ShellRelease::Settings, &open);
        assert!(
            zh.contains("不受沙箱限制"),
            "the Chinese sentence carries the same fact: {zh}"
        );
        assert_ne!(zh, detailed, "the sentence is localized");

        // The menu carries none of it, in either locale.
        for lang in [Lang::En, Lang::Zh] {
            let menu = rows(lang, &workspace);
            assert_eq!(menu.len(), 3, "still one row per mode");
            for row in &menu {
                let lowered = row.text.to_lowercase();
                assert!(
                    !lowered.contains("sandbox")
                        && !lowered.contains("unconfined")
                        && !lowered.contains("沙箱")
                        && !lowered.contains("不受"),
                    "the menu row must not carry the release: {}",
                    row.text
                );
            }
        }
    }
}
