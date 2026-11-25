//! A list the reader picks from, for the slash commands that name a choice.
//!
//! The desktop answers `/model` with a picker: a titled list, one row per
//! choice, a search box, and Enter to commit. A terminal's `/model` used to
//! answer with one status line of names, which a reader then had to retype
//! exactly - and a name retyped from a line that has scrolled away is the kind
//! of work a list should do.
//!
//! So this is the same picker in the form a terminal can draw: the rows the
//! Host's catalogue already offers, a fuzzy filter typed at the keyboard, the
//! highlight moved with the arrow keys, the row in force marked, and Enter to
//! commit. It is client state only - the rows come from a snapshot this client
//! already holds, and committing one sends the same `SessionConfigChange` the
//! typed command sent, so a pick and a typed name land on the same Host call.

use crate::fuzzy;
use crate::runtime::contract::{CatalogueView, PERMISSION_MODES, SessionConfig};

/// How many choices one frame draws.
///
/// A real catalogue carries a few dozen models and the body has one column, so
/// the list is a window that follows the highlight rather than a frame that
/// pushes the transcript off the screen. The desktop's card caps its height for
/// the same reason; eight rows is a terminal's share of it.
pub(crate) const VISIBLE_ROWS: usize = 8;

/// Which list a command asked for.
///
/// The three commands whose argument is one of a fixed set: each takes a bare
/// name from the Host's own catalogue, so the list can always be drawn from
/// what the client already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `/model`: the models, the provider beside each.
    Model,
    /// `/preset`: the presets this workspace may run.
    Preset,
    /// `/permission`: the approval modes the contract names.
    Permission,
    /// `/sessions`: the conversations this workspace holds.
    ///
    /// The rows are the client's own snapshot rather than the catalogue's:
    /// which chats exist is a fact about this workspace and this moment, so
    /// they come from [`session_items`], not from [`Kind::items`].
    Session,
    /// `/trash`: the bin this workspace holds.
    ///
    /// A snapshot too: what is parked is a fact about this workspace and this
    /// moment, and the number a row carries is the one `/restore` takes.
    Trash,
    /// `/sessions archived`: the chats this workspace has hidden.
    ///
    /// Here Enter is the Unarchive row of the desktop's session menu, so the
    /// numbers are the ones `/sessions unarchive` addresses.
    Archived,
    /// `/find`: the rows of the open conversation that contain the text.
    ///
    /// A snapshot too: the rows are the transcript this client paints, so the
    /// list and the screen cannot disagree about what the conversation says.
    /// Enter scrolls the transcript to the row, because a hit is only useful
    /// once the reader is reading it in its place.
    Find,
}

impl Kind {
    /// The line above the rows: what this list chooses.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Model => "choose a model",
            Self::Preset => "choose a preset",
            Self::Permission => "choose the approval mode",
            Self::Session => "open a session",
            Self::Trash => "restore a parked session",
            Self::Archived => "unarchive an archived session",
            Self::Find => "reveal a row of this conversation",
        }
    }

    /// What the list says when the catalogue has nothing to offer.
    ///
    /// A sentence, not an empty box: a list with no rows asks the reader to
    /// walk choices that are not there, and the reason a catalogue is empty -
    /// settings, not this client - is what they need told.
    pub(crate) fn empty(self) -> &'static str {
        match self {
            Self::Model => "no models are configured in bsh settings",
            Self::Preset => "no presets are configured in bsh settings",
            Self::Permission => "this build names no approval modes",
            Self::Session => "no sessions in this workspace · Ctrl-T creates one",
            Self::Trash => "the bin is empty · /trash <number> parks the row /sessions numbers",
            Self::Archived => "no archived sessions · /sessions archive <number> hides one",
            Self::Find => "nothing in this conversation matches",
        }
    }

    /// Every row this list offers from the catalogue the client holds.
    ///
    /// The three snapshot kinds have no rows here: which chats exist, which
    /// are parked and which are archived are the client's own snapshot, so the
    /// app builds those rows and hands them to [`Picker::new`] the way it
    /// builds them for the printed listing.
    pub(crate) fn items(self, catalogue: &CatalogueView) -> Vec<Item> {
        match self {
            Self::Model => model_items(catalogue),
            Self::Preset => preset_items(catalogue),
            Self::Permission => permission_items(),
            Self::Session | Self::Trash | Self::Archived | Self::Find => Vec::new(),
        }
    }

    /// Mark the row whose choice the session already runs.
    ///
    /// The desktop ticks the active model in its picker, so a reader can tell
    /// "this is what I have" from "this is what I might pick" without closing
    /// the list and reading the header. A session that names no model follows
    /// the machine default, so no row is ticked and none is claimed.
    pub(crate) fn mark(self, items: &mut [Item], config: &SessionConfig) {
        for item in items {
            item.active = match (&item.choice, self) {
                (Choice::Model { provider, model }, Self::Model) => {
                    config.model_ref().as_ref() == Some(&(provider.clone(), model.clone()))
                }
                (Choice::Preset(preset), Self::Preset) => {
                    config.preset.as_deref() == Some(preset.as_str())
                }
                (Choice::Permission(permission), Self::Permission) => {
                    config.permission == *permission
                }
                _ => false,
            };
        }
    }
}

/// What committing a row asks the client to do.
///
/// These are the choices the desktop's pickers commit, and each one resolves to
/// the same command the composer accepts when it is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Set the session's provider and model, as `/model <provider> <model>`.
    Model { provider: String, model: String },
    /// Set the session's preset, as `/preset <name>`.
    Preset(String),
    /// Set the session's approval mode, as `/permission <mode>`.
    Permission(String),
    /// Open the `index`-th session of the list the picker was built from, as
    /// `/sessions <index + 1>`: the number the printed listing shows.
    Session(usize),
    /// Take the `index`-th parked session back, as `/restore <index + 1>`: the
    /// number `/trash` prints in front of the row.
    Trash(usize),
    /// Bring the `index`-th archived session back, as
    /// `/sessions unarchive <index + 1>`.
    Archived(usize),
    /// Scroll the transcript to the `index`-th row of the timeline, which is
    /// the row a matched line of `/find` was found on.
    Find(usize),
}

/// One row a reader can walk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    /// The row's own name, drawn first and ranked by the filter.
    pub(crate) label: String,
    /// The fact beside it - the provider, what the mode allows - drawn last.
    pub(crate) detail: String,
    /// What accepting the row commits.
    pub(crate) choice: Choice,
    /// Whether the session already runs this choice; drawn as a tick.
    pub(crate) active: bool,
}

impl Item {
    /// One row, with the text the filter ranks: the label and the detail
    /// together, so a reader can narrow the list by either.
    fn ranked(&self) -> String {
        if self.detail.is_empty() {
            self.label.clone()
        } else {
            format!("{} {}", self.label, self.detail)
        }
    }
}

/// The picker's whole state: what it offers, what has been typed, and where the
/// highlight is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picker {
    /// The row above the list: what this list chooses.
    title: String,
    items: Vec<Item>,
    /// What the reader typed to narrow the list; empty shows every row.
    query: String,
    /// Index into the rows the query leaves, not into `items`: with no query
    /// the two are the same, and with one the highlight belongs to what the
    /// reader can see.
    selected: usize,
}

impl Picker {
    /// A picker over `items`, highlighting the first row.
    pub(crate) fn new(title: impl Into<String>, items: Vec<Item>) -> Self {
        Self {
            title: title.into(),
            items,
            query: String::new(),
            selected: 0,
        }
    }

    /// The line above the rows.
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// Every row, before the query narrows them.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// What the reader has typed to narrow the list.
    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    /// Whether any row carries a tick, whatever the filter hides.
    ///
    /// The footer names the tick only on a list that has one, and a filter that
    /// happens to hide the ticked row must not make the key flicker away: the
    /// list still marks the row the session runs.
    pub(crate) fn any_active(&self) -> bool {
        self.items.iter().any(|item| item.active)
    }

    /// The highlighted row, as an index into the rows [`Picker::rows`] returns.
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// The rows the query leaves, best match first.
    ///
    /// An empty query keeps the caller's order: a blank filter is not a match
    /// to rank, and the list the reader opened is the order they expect.
    pub(crate) fn rows(&self) -> Vec<&Item> {
        if self.query.trim().is_empty() {
            return self.items.iter().collect();
        }
        let ranked: Vec<String> = self.items.iter().map(Item::ranked).collect();
        fuzzy::rank(&self.query, ranked.iter().map(String::as_str))
            .into_iter()
            .map(|hit| &self.items[hit.index])
            .collect()
    }

    /// The row under the highlight.
    pub(crate) fn highlighted(&self) -> Option<&Item> {
        self.rows().get(self.selected).copied()
    }

    /// The choice the highlight names, for a reader who pressed Enter.
    pub(crate) fn choice(&self) -> Option<Choice> {
        self.highlighted().map(|item| item.choice.clone())
    }

    /// The window of rows one frame draws: where it starts, and how many of
    /// the matching rows it holds.
    ///
    /// The frame and a click read the same arithmetic: a pointer names a screen
    /// row, and the row it names is only a choice because this window put it
    /// there. So the window is computed here, once, instead of twice.
    pub(crate) fn window(&self) -> (usize, usize) {
        let shown = self.rows().len();
        let visible = shown.min(VISIBLE_ROWS);
        let start = self.selected.saturating_sub(visible.saturating_sub(1));
        (start, visible)
    }

    /// Point the highlight at one row of the matching list, as a click does.
    ///
    /// A row past the ends is clamped rather than ignored: a pointer that
    /// names the last visible row while the list is shorter still means the
    /// last choice, not nothing.
    pub(crate) fn select(&mut self, index: usize) -> bool {
        let count = self.rows().len();
        if count == 0 {
            return false;
        }
        let at = index.min(count - 1);
        let moved = at != self.selected;
        self.selected = at;
        moved
    }

    /// Move the highlight by `delta` rows, stopping at the ends the way a list
    /// does rather than wrapping past the row the reader was looking at.
    pub(crate) fn move_by(&mut self, delta: isize) -> bool {
        let count = self.rows().len();
        if count == 0 {
            return false;
        }
        let at = (self.selected as isize + delta).clamp(0, count as isize - 1) as usize;
        let moved = at != self.selected;
        self.selected = at;
        moved
    }

    /// Narrow the list with one more character.
    pub(crate) fn typed(&mut self, character: char) {
        self.query.push(character);
        // The old highlight named a row of a longer list, so it names nothing
        // now: the reader starts again at the best match.
        self.selected = 0;
    }

    /// Take the last typed character back, and report whether there was one.
    pub(crate) fn erase(&mut self) -> bool {
        if self.query.pop().is_none() {
            return false;
        }
        self.selected = 0;
        true
    }
}

/// The rows a `/model` picker offers: every model the Host's catalogue names,
/// with the provider beside it.
///
/// The label is the model and the detail is the provider, because a model name
/// is what a reader picks by and the provider is the fact that separates two
/// rows with the same name. Both are ranked, so either narrows the list.
pub(crate) fn model_items(catalogue: &CatalogueView) -> Vec<Item> {
    catalogue
        .providers
        .iter()
        .flat_map(|provider| {
            provider.models.iter().map(|model| Item {
                label: model.clone(),
                detail: provider.provider.clone(),
                choice: Choice::Model {
                    provider: provider.provider.clone(),
                    model: model.clone(),
                },
                active: false,
            })
        })
        .collect()
}

/// The rows a `/preset` picker offers, in the order the Host lists them.
pub(crate) fn preset_items(catalogue: &CatalogueView) -> Vec<Item> {
    catalogue
        .presets
        .iter()
        .map(|preset| Item {
            label: preset.clone(),
            detail: preset_detail(preset).to_string(),
            choice: Choice::Preset(preset.clone()),
            active: false,
        })
        .collect()
}

/// The one fact a preset row carries beside its id.
///
/// The desktop's preset card states what the preset is for; a terminal has one
/// line, so a known preset names its purpose and an id this build has no
/// sentence for is drawn as the plain preset it is rather than guessed at.
fn preset_detail(preset: &str) -> &'static str {
    match preset {
        "default" => "the machine's own agent",
        "review" => "reads and reports, changes nothing",
        _ => "preset",
    }
}

/// The rows a `/permission` picker offers: the modes the contract names, each
/// with the one line the desktop's chip menu carries beside it.
///
/// The order is the contract's, which is the order the reference draws: least
/// reach first, so a reader walking down the list is walking outwards.
pub(crate) fn permission_items() -> Vec<Item> {
    PERMISSION_MODES
        .iter()
        .map(|mode| Item {
            label: (*mode).to_string(),
            detail: permission_detail(mode).to_string(),
            choice: Choice::Permission((*mode).to_string()),
            active: false,
        })
        .collect()
}

/// What each mode permits, in the words the desktop's chip menu uses.
fn permission_detail(mode: &str) -> &'static str {
    match mode {
        crate::caps::READ_ONLY => "reads only, no writes and no commands",
        crate::caps::WORKSPACE_WRITE => "writes and commands stay in this workspace",
        crate::caps::FULL_ACCESS => "no confinement, anywhere on the machine",
        _ => "approval mode",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::contract::ProviderModels;

    fn sample() -> Picker {
        Picker::new(
            "choose a model",
            vec![
                Item {
                    label: "deepseek-flash".to_string(),
                    detail: "deepseek".to_string(),
                    choice: Choice::Model {
                        provider: "deepseek".to_string(),
                        model: "deepseek-flash".to_string(),
                    },
                    active: false,
                },
                Item {
                    label: "glm-5.3".to_string(),
                    detail: "openai".to_string(),
                    choice: Choice::Model {
                        provider: "openai".to_string(),
                        model: "glm-5.3".to_string(),
                    },
                    active: false,
                },
                Item {
                    label: "qwen3.8-flash".to_string(),
                    detail: "openai".to_string(),
                    choice: Choice::Model {
                        provider: "openai".to_string(),
                        model: "qwen3.8-flash".to_string(),
                    },
                    active: false,
                },
            ],
        )
    }

    #[test]
    fn enter_commits_the_row_under_the_highlight() {
        let mut picker = sample();
        assert_eq!(picker.title(), "choose a model");
        assert_eq!(picker.rows().len(), 3);
        assert_eq!(
            picker.choice(),
            Some(Choice::Model {
                provider: "deepseek".to_string(),
                model: "deepseek-flash".to_string(),
            })
        );
        assert!(picker.move_by(1));
        assert_eq!(
            picker.choice(),
            Some(Choice::Model {
                provider: "openai".to_string(),
                model: "glm-5.3".to_string(),
            })
        );
        // The ends hold: a list does not wrap past the row being read.
        assert!(picker.move_by(9), "a page lands on the last row");
        assert_eq!(picker.selected(), 2);
        assert!(!picker.move_by(9), "and stays there");
        assert!(picker.move_by(-9));
        assert_eq!(picker.selected(), 0);
    }

    #[test]
    fn a_typed_filter_narrows_the_rows_and_the_highlight_starts_over() {
        let mut picker = sample();
        picker.move_by(2);
        for character in "qwen".chars() {
            picker.typed(character);
        }
        let labels: Vec<&str> = picker
            .rows()
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(labels, vec!["qwen3.8-flash"]);
        assert_eq!(
            picker.selected(),
            0,
            "a highlight from the longer list names nothing in the shorter one"
        );
        assert_eq!(picker.query(), "qwen");
        // The provider ranks too, so a reader can narrow by who serves it.
        let mut picker = sample();
        for character in "openai".chars() {
            picker.typed(character);
        }
        assert_eq!(picker.rows().len(), 2);
        assert!(picker.erase());
        assert_eq!(picker.rows().len(), 2, "one character back still matches");
        for _ in 0.."openai".len() {
            picker.erase();
        }
        assert_eq!(
            picker.rows().len(),
            3,
            "an empty filter shows the list again"
        );
        assert!(!picker.erase(), "nothing left to take back");
    }

    #[test]
    fn a_filter_that_matches_nothing_commits_nothing() {
        let mut picker = sample();
        for character in "zzz".chars() {
            picker.typed(character);
        }
        assert!(picker.rows().is_empty());
        assert_eq!(picker.choice(), None);
        assert!(!picker.move_by(1), "an empty list has nowhere to walk");
        assert_eq!(picker.selected(), 0);
    }

    #[test]
    fn the_rows_are_the_catalogues_own_with_the_provider_beside_them() {
        let catalogue = CatalogueView {
            providers: vec![ProviderModels {
                provider: "deepseek".to_string(),
                models: vec!["deepseek-flash".to_string(), "deepseek-v4-pro".to_string()],
            }],
            presets: vec!["review".to_string(), "fast".to_string()],
            ..CatalogueView::default()
        };
        let models = model_items(&catalogue);
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].label, "deepseek-v4-pro");
        assert_eq!(models[1].detail, "deepseek");
        assert_eq!(
            models[1].choice,
            Choice::Model {
                provider: "deepseek".to_string(),
                model: "deepseek-v4-pro".to_string(),
            }
        );
        let presets = preset_items(&catalogue);
        assert_eq!(presets[0].label, "review");
        assert_eq!(presets[0].detail, "reads and reports, changes nothing");
        assert_eq!(presets[1].choice, Choice::Preset("fast".to_string()));
        let modes = permission_items();
        assert_eq!(modes.len(), PERMISSION_MODES.len());
        assert_eq!(modes[0].choice, Choice::Permission("read-only".to_string()));
        assert_eq!(modes[0].detail, "reads only, no writes and no commands");
    }

    #[test]
    fn the_row_the_session_runs_is_marked_and_nothing_else_is() {
        let catalogue = CatalogueView {
            providers: vec![ProviderModels {
                provider: "openai".to_string(),
                models: vec!["glm-5.3".to_string(), "qwen3.8-flash".to_string()],
            }],
            presets: vec!["review".to_string(), "fast".to_string()],
            ..CatalogueView::default()
        };
        let mut items = Kind::Model.items(&catalogue);
        let config = SessionConfig {
            provider: Some("openai".to_string()),
            model: Some("qwen3.8-flash".to_string()),
            preset: Some("fast".to_string()),
            permission: "workspace-write".to_string(),
            plan_mode: false,
        };
        Kind::Model.mark(&mut items, &config);
        let marked: Vec<&str> = items
            .iter()
            .filter(|item| item.active)
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(marked, vec!["qwen3.8-flash"]);

        let mut presets = Kind::Preset.items(&catalogue);
        Kind::Preset.mark(&mut presets, &config);
        let marked: Vec<&str> = presets
            .iter()
            .filter(|item| item.active)
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(marked, vec!["fast"]);

        let mut modes = Kind::Permission.items(&catalogue);
        Kind::Permission.mark(&mut modes, &config);
        let marked: Vec<&str> = modes
            .iter()
            .filter(|item| item.active)
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(marked, vec!["workspace-write"]);

        // A session on the machine default names no model: no row claims to be
        // the one in force, because none of them is.
        let mut items = Kind::Model.items(&catalogue);
        Kind::Model.mark(&mut items, &SessionConfig::default());
        assert!(items.iter().all(|item| !item.active));
    }

    #[test]
    fn a_kind_with_nothing_to_offer_says_why_instead_of_drawing_a_box() {
        let empty = CatalogueView::default();
        assert!(Kind::Model.items(&empty).is_empty());
        assert!(Kind::Preset.items(&empty).is_empty());
        assert!(
            !Kind::Permission.items(&empty).is_empty(),
            "the contract names modes in any build"
        );
        assert!(Kind::Model.empty().contains("settings"));
        assert!(Kind::Model.title().contains("model"));
    }
}
