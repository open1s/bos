//! The GPUI renderer for declarative extension panels.
//!
//! This view owns no extension state beyond the selected revision. It asks the
//! owning Runtime for a UiDocument, validates it again at the host boundary,
//! renders the bounded tree with GPUI widgets, and routes every action back with
//! the revision it was rendered from. A stale action is refused, so a button
//! from an old frame cannot mutate a newer one.
//!
//! The chooser above the document is one searchable dropdown over the panels
//! installed for this mount, so the surface keeps its shape whether one package
//! ships a panel or a dozen do.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use gpui_kit::component::button::Button;
use gpui_kit::component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::IndexPath;
use gpui_kit::*;

use crate::extension::host::PanelSurface;
use crate::i18n::{tr, tr_args, Lang};
use crate::theme;
use ferris_sdk::host_runtime::CallContext;
use ferris_sdk::types::ExtensionError;
use ferris_sdk::ui::{
    ButtonStyle, NotificationLevel, PanelLocation, ThemePalette, Tone, UiDocument, UiNode,
};

use super::{hairline_bottom, icon, txt, IconName};

/// The host palette with a panel's theme tokens layered over it.
struct Palette {
    page: u32,
    text: u32,
    muted: u32,
    faint: u32,
    border: u32,
    border_soft: u32,
    accent: u32,
    error: u32,
    success: u32,
    warn: u32,
    new_session_fill: u32,
    hover: u32,
}

impl Palette {
    fn new(theme: Option<&ThemePalette>) -> Self {
        let mut palette = Self {
            page: theme::page(),
            text: theme::text(),
            muted: theme::muted(),
            faint: theme::faint(),
            border: theme::border(),
            border_soft: theme::border_soft(),
            accent: theme::accent(),
            error: theme::error(),
            success: theme::success(),
            warn: theme::warn(),
            new_session_fill: theme::new_session_fill(),
            hover: theme::hover(),
        };
        if let Some(theme) = theme {
            for (token, slot) in [
                ("page", &mut palette.page),
                ("text", &mut palette.text),
                ("muted", &mut palette.muted),
                ("faint", &mut palette.faint),
                ("border", &mut palette.border),
                ("border-soft", &mut palette.border_soft),
                ("accent", &mut palette.accent),
                ("error", &mut palette.error),
                ("success", &mut palette.success),
                ("warn", &mut palette.warn),
                ("new-session-fill", &mut palette.new_session_fill),
                ("hover", &mut palette.hover),
            ] {
                if let Some(color) = theme.color(token) {
                    *slot = color;
                }
            }
        }
        palette
    }
}

/// One row of the installed-panel picker.
///
/// The row label is the text the picker searches, so it carries the location
/// too: two packages can ship a panel with the same title, and a reader looking
/// for the right one needs to tell them apart.
#[derive(Clone, Debug)]
struct PanelItem {
    /// Index into the view's `panels`, not into the filtered list.
    index: usize,
    label: String,
}

impl SearchableListItem for PanelItem {
    type Value = usize;

    fn title(&self) -> SharedString {
        SharedString::from(self.label.clone())
    }

    fn value(&self) -> &usize {
        &self.index
    }
}

/// Looks up a panel's surface in the current host generation.
///
/// A panel is identified by its qualified id, and the answer is the surface the
/// live host offers right now — never a stored snapshot.
type SurfaceResolver = Arc<dyn Fn(&str) -> Option<PanelSurface> + Send + Sync>;

/// Where a panel's rendered document and revision come from.
///
/// The production store is the surface host: the renderer draws the document
/// the host's engine validated, and the action check compares against the
/// engine's revision. A view built without a host (a unit test) falls back to
/// the panel's declared document, and its own revision gate.
trait SurfaceStore: Send + Sync {
    /// The document and revision to draw.
    fn view(&self, panel_id: &str) -> Option<(UiDocument, u64)>;
    /// Apply a live card as the next frame.
    fn apply(&self, panel_id: &str, document: &UiDocument) -> Result<(UiDocument, u64), String>;
    /// Refuse an action stamped with a revision the surface moved past.
    fn accept(&self, panel_id: &str, revision: u64) -> Result<(), String>;
    /// The binding a control names, when the panel ships a binding table.
    fn binding(&self, panel_id: &str, key: &str) -> Option<ferris_sdk::surface::ActionBinding>;
    /// Replace the provider generation after a runtime rebind.
    fn renew(&self, panel_id: &str) -> Result<(), String>;
}

/// The production store, backed by the open extension host.
struct HostSurfaceStore;

impl SurfaceStore for HostSurfaceStore {
    fn view(&self, panel_id: &str) -> Option<(UiDocument, u64)> {
        crate::extension::host::surface_view(panel_id)
    }

    fn apply(&self, panel_id: &str, document: &UiDocument) -> Result<(UiDocument, u64), String> {
        crate::extension::host::surface_apply(panel_id, document)
    }

    fn accept(&self, panel_id: &str, revision: u64) -> Result<(), String> {
        crate::extension::host::surface_accept(panel_id, revision)
    }

    fn binding(&self, panel_id: &str, key: &str) -> Option<ferris_sdk::surface::ActionBinding> {
        crate::extension::host::surface_binding(panel_id, key)
    }

    fn renew(&self, panel_id: &str) -> Result<(), String> {
        crate::extension::host::surface_renew(panel_id)
    }
}

/// One extension panel surface in the settings dialog.
pub(crate) struct ExtensionPanelView {
    panels: Vec<PanelSurface>,
    selected: usize,
    /// The revision of the document on screen.
    revision: u64,
    /// Whether the document came from the surface host or a local fallback.
    host_backed: bool,
    /// Where the document and its revision come from.
    store: Arc<dyn SurfaceStore>,
    document: Option<UiDocument>,
    loading: bool,
    error: Option<String>,
    message: Option<String>,
    palette: Palette,
    /// Selected index per tabs path, so switching a tab is host state.
    tabs: std::collections::BTreeMap<String, usize>,
    /// The selected panel's package root, for `asset/…` images and icons.
    panel_root: Option<std::path::PathBuf>,
    /// The revision the controls currently on screen were rendered from.
    ///
    /// The action request carries this, not the live session revision: stamping
    /// it at dispatch time made the staleness check unable to fail.
    render_revision: u64,
    /// The language the picker's own labels are written in.
    lang: Lang,
    /// The installed-panel picker.
    ///
    /// Its state needs a `Window`, so it is built on the first frame rather than
    /// in `new`, and kept afterwards so a re-render does not rebuild it (which
    /// would drop the reader's search query and the open popover).
    picker: Option<Entity<ComboboxState<SearchableVec<PanelItem>>>>,
    /// Dropping this unsubscribes the picker, and a chosen row would then stop
    /// switching panels, so the view holds it for its whole life.
    picker_subscription: Option<Subscription>,
    /// Whether the runtime's live card has been requested for this selection.
    ///
    /// A panel that ships a declared view shows it on the first frame, but that
    /// view is a placeholder: the runtime may serve a live card for the same
    /// panel, and it is fetched once per selection (and on Refresh).
    attempted: bool,
    /// Editable input state, keyed by node path.
    ///
    /// A declared input carries a starting value, not the field's life: the
    /// state entity is kept per path so typing survives a re-render, and the
    /// Apply action reads it back.
    inputs: RefCell<BTreeMap<String, (String, Entity<InputState>)>>,
    /// Section open state, keyed by node path.
    sections: RefCell<BTreeMap<String, bool>>,
    /// The panel's own light palette, when the package named a theme.
    panel_theme: Option<ThemePalette>,
    /// The panel's own dark palette, when the package named one.
    panel_theme_dark: Option<ThemePalette>,
    /// How often the live card should be re-read, when the package asks.
    refresh_ms: Option<u64>,
    /// Whether a refresh timer is already scheduled for this view.
    refresh_pending: bool,
    /// How the view finds a panel's surface in the current host generation.
    ///
    /// The surfaces a view is built with name the runtimes of the generation
    /// that was live when it was made, and a rebuild shuts those down. This
    /// resolves the replacement. It is a field, not a direct call, so the rebind
    /// path can be tested without the process-global host.
    surface_resolver: SurfaceResolver,
}

impl ExtensionPanelView {
    /// Build a view over the panels the host currently offers.
    pub(crate) fn new(panels: Vec<PanelSurface>, lang: Lang) -> Self {
        let selected = panels
            .iter()
            .position(|panel| matches!(panel.visibility, ferris_sdk::ui::PanelVisibility::Visible))
            .unwrap_or(0);
        let panel_theme = panels.get(selected).and_then(|panel| panel.theme.clone());
        let panel_theme_dark = panels
            .get(selected)
            .and_then(|panel| panel.theme_dark.clone());
        let palette = Palette::new(panel_palette(&panel_theme, &panel_theme_dark));
        let panel_root = panels.get(selected).map(|panel| panel.root.clone());
        let refresh_ms = panels.get(selected).and_then(|panel| panel.refresh_ms);
        let mut view = Self {
            panels,
            selected,
            revision: 0,
            host_backed: false,
            store: Arc::new(HostSurfaceStore),
            document: None,
            loading: false,
            error: None,
            message: None,
            palette,
            tabs: std::collections::BTreeMap::new(),
            panel_root,
            render_revision: 0,
            lang,
            picker: None,
            picker_subscription: None,
            attempted: false,
            inputs: RefCell::new(BTreeMap::new()),
            sections: RefCell::new(BTreeMap::new()),
            panel_theme,
            panel_theme_dark,
            refresh_ms,
            refresh_pending: false,
            surface_resolver: Arc::new(crate::extension::live_panel),
        };
        if let Some(panel) = view.panels.get(selected).cloned() {
            if let Some(document) = panel.initial.clone() {
                view.seed(&panel, &document);
            }
        }
        view
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.panels.len() || index == self.selected {
            return;
        }
        self.selected = index;
        self.revision = 0;
        self.host_backed = false;
        self.document = None;
        self.error = None;
        self.message = None;
        self.loading = false;
        self.attempted = false;
        self.refresh_ms = self.panels[index].refresh_ms;
        self.panel_root = Some(self.panels[index].root.clone());
        self.panel_theme = self.panels[index].theme.clone();
        self.panel_theme_dark = self.panels[index].theme_dark.clone();
        self.palette = Palette::new(panel_palette(&self.panel_theme, &self.panel_theme_dark));
        let panel = self.panels[index].clone();
        if let Some(document) = panel.initial.clone() {
            self.seed(&panel, &document);
        } else {
            self.load(cx);
        }
        cx.notify();
    }

    /// The picker's rows, in the order the view holds the panels.
    fn picker_items(panels: &[PanelSurface]) -> Vec<PanelItem> {
        panels
            .iter()
            .enumerate()
            .map(|(index, panel)| PanelItem {
                index,
                label: format!("{} · {}", panel.title, location_label(panel.location)),
            })
            .collect()
    }

    /// Build the picker on the first frame.
    ///
    /// `ComboboxState` needs a `Window`, and the view is constructed before one
    /// is available, so this is the earliest point that can build it.
    fn ensure_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() || self.panels.is_empty() {
            return;
        }
        let items = SearchableVec::new(Self::picker_items(&self.panels));
        let selected = vec![IndexPath::default().row(self.selected)];
        let picker = cx.new(|cx| {
            ComboboxState::new(items, selected, window, cx)
                .searchable(true)
                .multiple(false)
        });
        // The row carries the panel's own index, so a filtered list's row
        // number can never be mistaken for a panel one.
        let subscription = cx.subscribe(
            &picker,
            |this, _picker, event: &ComboboxEvent<SearchableVec<PanelItem>>, cx| {
                let values = match event {
                    ComboboxEvent::Change(values) | ComboboxEvent::Confirm(values) => values,
                };
                if let Some(index) = values.first().copied() {
                    this.select(index, cx);
                }
            },
        );
        self.picker = Some(picker);
        self.picker_subscription = Some(subscription);
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.should_fetch() {
            self.load(cx);
        }
    }

    /// Whether this frame should request the runtime's card.
    ///
    /// A declared view counts as a placeholder, not an answer: the live card is
    /// still fetched once, so a package that ships both does not freeze on the
    /// static copy.
    fn should_fetch(&self) -> bool {
        !self.attempted && !self.loading
    }

    /// Fetch the runtime's live card again.
    ///
    /// A panel's data (a capture store, a job list) moves after the first
    /// frame; without this the only way to see new rows was to switch panels or
    /// reopen the pane.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.attempted = false;
        self.error = None;
        self.message = None;
        self.load(cx);
    }

    /// Arm the next live re-read, when the package declared an interval.
    ///
    /// The timer is armed from a render, so a hidden pane stops refreshing by
    /// itself: no render, no new timer. A visible pane re-arms after each tick.
    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(ms) = self.refresh_ms else {
            return;
        };
        if self.refresh_pending {
            return;
        }
        self.refresh_pending = true;
        let interval = std::time::Duration::from_millis(ms.clamp(250, 60_000));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(interval).await;
            let _ = this.update(cx, |this, cx| {
                this.refresh_pending = false;
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// The editable state for one input node.
    fn input_for(
        &self,
        path: &str,
        value: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let existing = self.inputs.borrow().get(path).cloned();
        if let Some((declared, entity)) = existing {
            if declared != value {
                // Never overwrite the field the reader is typing in: a live
                // card that re-sends an older value would otherwise clobber
                // the caret and the input method's composition buffer. The
                // cache stays at the declared value, so the update lands on
                // the first frame after focus leaves.
                let focused = entity.read(cx).focus_handle(cx).is_focused(window);
                if focused {
                    return entity;
                }
                entity.update(cx, |state, cx| state.set_value(value, window, cx));
                self.inputs
                    .borrow_mut()
                    .insert(path.to_string(), (value.to_string(), entity.clone()));
            }
            return entity;
        }
        let entity = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_string()));
        if !value.is_empty() {
            entity.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        self.inputs
            .borrow_mut()
            .insert(path.to_string(), (value.to_string(), entity.clone()));
        entity
    }

    /// The text a panel input currently holds.
    fn input_value(&self, path: &str, cx: &Context<Self>) -> String {
        self.inputs
            .borrow()
            .get(path)
            .map(|(_, entity)| entity.read(cx).value().to_string())
            .unwrap_or_default()
    }

    /// A semantic tone from the active panel palette.
    ///
    /// A component names a role, never a colour, so every node follows the
    /// package theme (when one is named) and the reader's light/dark choice.
    fn tone_color(&self, tone: Tone) -> u32 {
        match tone {
            Tone::Neutral => self.palette.text,
            Tone::Muted => self.palette.muted,
            Tone::Faint => self.palette.faint,
            Tone::Accent => self.palette.accent,
            Tone::Success => self.palette.success,
            Tone::Warning => self.palette.warn,
            Tone::Error => self.palette.error,
        }
    }

    /// Report a failed live fetch.
    ///
    /// A panel that ships a declared view keeps it on screen: a runtime without
    /// a live card is a note under the declared view, not an empty pane. A
    /// panel with nothing to show still reports the error where the card would
    /// be.
    fn live_failure(&mut self, error: String) {
        if self.document.is_some() {
            self.message = Some(tr_args(
                self.lang,
                "right.extensions.liveFailed",
                &[("error", error)],
            ));
            self.error = None;
        } else {
            self.error = Some(error);
        }
    }

    /// Seed the view from the surface host, or from the declared view when no
    /// host is open.
    fn seed(&mut self, panel: &PanelSurface, document: &UiDocument) {
        self.revision = 0;
        self.host_backed = false;
        if let Some((document, revision)) = self.store.view(&panel.panel_id) {
            self.document = Some(document);
            self.revision = revision;
            self.host_backed = true;
            self.error = None;
            return;
        }
        let report = document.validate(&panel.owner);
        if report.is_valid() {
            self.document = Some(document.clone());
            self.revision = 1;
            self.error = None;
        } else {
            self.error = Some(
                report
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
    }

    /// Apply a live card through the surface host, falling back to a local
    /// validation when the host is not open.
    fn apply_live(&mut self, panel: &PanelSurface, document: UiDocument) {
        match self.store.apply(&panel.panel_id, &document) {
            Ok((document, revision)) => {
                self.document = Some(document);
                self.revision = revision;
                self.host_backed = true;
                self.error = None;
                self.message = None;
            }
            Err(error) => {
                if !self.host_backed {
                    let report = document.validate(&panel.owner);
                    if report.is_valid() {
                        self.document = Some(document);
                        self.revision += 1;
                        self.error = None;
                        self.message = None;
                        return;
                    }
                }
                self.live_failure(error);
            }
        }
    }

    /// Point the view's copy of a panel at the runtime that just answered.
    ///
    /// A panel is looked up by id, not by its row index, because the reader can
    /// switch panels while the call is in flight.
    fn rebind(&mut self, surface: &PanelSurface) {
        if let Some(index) = self
            .panels
            .iter()
            .position(|panel| panel.panel_id == surface.panel_id)
        {
            self.panels[index] = surface.clone();
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.panels.get(self.selected).cloned() else {
            return;
        };
        self.loading = true;
        self.attempted = true;
        self.error = None;
        self.message = None;
        let resolve = Arc::clone(&self.surface_resolver);
        cx.spawn(async move |this, cx| {
            // A rebuild can land between the view being built and the first
            // frame that needs the document, so resolve the generation here.
            let resolved = live_surface(&panel, &*resolve);
            let panel = resolved.clone().unwrap_or(panel);
            let outcome = panel.runtime.panel(&panel.contribution_id).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(fresh) = resolved {
                    this.rebind(&fresh);
                    let _ = this.store.renew(&fresh.panel_id);
                }
                this.loading = false;
                match outcome {
                    Ok(value) => match serde_json::from_value::<UiDocument>(value) {
                        Ok(document) => this.apply_live(&panel, document),
                        Err(error) => {
                            this.live_failure(format!("panel document is not valid: {error}"))
                        }
                    },
                    Err(error) => this.live_failure(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn dispatch(&mut self, action: &str, payload: Value, cx: &mut Context<Self>) {
        let Some(mut panel) = self.panels.get(self.selected).cloned() else {
            return;
        };
        // A composed slot prefixes its actions with the panel that owns the
        // region. Route the press to that provider's tools and policy; the
        // parent's revision gate still covers the frame the reader saw.
        let mut action = action.to_string();
        while let Some((head, rest)) = action.split_once('\u{1f}') {
            let Some(next) = self
                .panels
                .iter()
                .find(|panel| panel.panel_id == head)
                .cloned()
            else {
                break;
            };
            panel = next;
            action = rest.to_string();
        }
        // The rendered revision is the one the host's engine validated. A
        // surface-backed panel asks the host; a locally seeded one (no open
        // host, as in a unit test) keeps the same revision equality rule.
        let accepted = if self.host_backed {
            self.store.accept(&panel.panel_id, self.render_revision)
        } else if self.render_revision == self.revision {
            Ok(())
        } else {
            Err(format!(
                "stale action at revision {}; the panel is at {}",
                self.render_revision, self.revision
            ))
        };
        if let Err(error) = accepted {
            self.error = Some(error);
            cx.notify();
            return;
        }
        // A panel action is a tool call on a surface that is not the agent, so
        // it needs the same per-call capability decision and audit line.
        //
        // A schema 1.1 panel names a binding, not a tool. Resolve the binding's
        // capability to the tool that implements it: an exact id, or the local
        // name after the provider slash (optionally namespaced). A binding with
        // no tool behind it is refused with the capability named, never sent to
        // the command seam as if it were a contribution id.
        let mut call_name = action.clone();
        let mut descriptor = panel
            .tools
            .iter()
            .find(|descriptor| descriptor.id.as_str() == action.as_str())
            .cloned();
        if descriptor.is_none() {
            if let Some(binding) = self.store.binding(&panel.panel_id, &action) {
                let local = binding.capability.rsplit('/').next().unwrap_or_default();
                match panel
                    .tools
                    .iter()
                    .find(|descriptor| {
                        descriptor.id.as_str() == local
                            || descriptor.id.as_str() == binding.capability
                            || descriptor.id.as_str().ends_with(&format!("__{local}"))
                    })
                    .cloned()
                {
                    Some(found) => {
                        call_name = found.id.as_str().to_string();
                        descriptor = Some(found);
                    }
                    None => {
                        self.error = Some(format!(
                            "binding {action:?} names {}, which has no tool in this panel",
                            binding.capability
                        ));
                        cx.notify();
                        return;
                    }
                }
            }
        }
        if let Some(descriptor) = &descriptor {
            let scope = ferris_sdk::capability::call_scope(&payload, panel.workspace.as_deref());
            if let Err(error) = ferris_sdk::capability::authorize_each(
                &panel.policy,
                panel.owner.as_str(),
                panel.trust,
                &descriptor.capabilities,
                &scope,
            ) {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        }
        let revision = self.render_revision;
        // §9's UI row: the host announces that a panel action was routed.
        crate::extension::publish(
            crate::extension::topics::UI_ACTION,
            serde_json::json!({
                "extension": panel.owner.as_str(),
                "panel": panel.panel_id,
                "action": action.clone(),
                "revision": revision,
            }),
        );
        let resolve = Arc::clone(&self.surface_resolver);
        self.message = Some("Running…".to_string());
        cx.spawn(async move |this, cx| {
            let action_for_call = call_name;
            // The card may have outlived the generation that served it; asking
            // the retired handle would only report that it is gone.
            let resolved = live_surface(&panel, &*resolve);
            let surface = resolved.clone().unwrap_or_else(|| panel.clone());
            let context = CallContext::new(
                panel.owner.clone(),
                format!("panel:{}", panel.panel_id),
                panel.workspace.clone(),
                30_000,
            );
            let outcome = crate::extension::run_on_tokio(async move {
                // An action names either a tool or a runtime command. Tools are
                // the common case (the hello panel's Echo is a tool); fall back
                // to the command seam only when no tool owns the id.
                let runtime = surface.runtime.clone();
                match runtime
                    .call_tool(&action_for_call, payload.clone(), &context)
                    .await
                {
                    Err(ExtensionError::NotFound { .. }) => {
                        runtime.command(&action_for_call, payload).await
                    }
                    other => other,
                }
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(fresh) = resolved {
                    this.rebind(&fresh);
                    let _ = this.store.renew(&fresh.panel_id);
                }
                this.message = None;
                match outcome {
                    Ok(value) => {
                        if let Ok(document) = serde_json::from_value::<UiDocument>(value.clone()) {
                            this.apply_live(&panel, document);
                        } else {
                            this.message = Some(summarize(&value));
                        }
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_node(
        &self,
        node: &UiNode,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            UiNode::Container { children } | UiNode::Column { children } => {
                let mut column = div().flex().flex_col().gap(px(8.0)).w_full();
                for (index, child) in children.iter().enumerate() {
                    column = column.child(self.render_node(
                        child,
                        &format!("{path}-{index}"),
                        window,
                        cx,
                    ));
                }
                column.into_any_element()
            }
            UiNode::Row { children } => {
                let mut row = div().flex().flex_row().items_center().gap(px(8.0)).w_full();
                for (index, child) in children.iter().enumerate() {
                    row =
                        row.child(self.render_node(child, &format!("{path}-{index}"), window, cx));
                }
                row.into_any_element()
            }
            UiNode::Text { text, tone } => {
                txt(text.clone(), 13.0, self.tone_color(*tone)).into_any_element()
            }
            UiNode::Heading { text, level, tone } => {
                let size = match level {
                    1 => 21.0,
                    2 => 19.0,
                    3 => 18.0,
                    4 => 16.0,
                    _ => 14.0,
                };
                txt(text.clone(), size, self.tone_color(*tone))
                    .font_weight(FontWeight::SEMIBOLD)
                    .into_any_element()
            }
            UiNode::Markdown { text } => {
                txt(text.clone(), 13.0, self.palette.text).into_any_element()
            }
            UiNode::Button {
                label,
                action,
                style,
            } => {
                let (fill, ink) = match style {
                    ButtonStyle::Primary => (self.palette.accent, self.palette.page),
                    ButtonStyle::Danger => (self.palette.error, self.palette.page),
                    ButtonStyle::Ghost => (self.palette.page, self.palette.muted),
                    ButtonStyle::Secondary => (self.palette.new_session_fill, self.palette.text),
                };
                let action = action.clone();
                Button::new(format!("extension-action-{path}"))
                    .debug_selector(|| format!("extension-action-{path}"))
                    .accessibility_label(label.clone())
                    .px(px(12.0))
                    .h(px(30.0))
                    .rounded(px(8.0))
                    .bg(rgb(fill))
                    .border_1()
                    .border_color(rgb(self.palette.border_soft))
                    .child(txt(label.clone(), 13.0, ink))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.dispatch(&action, json!({}), cx)
                    }))
                    .into_any_element()
            }
            UiNode::Input {
                label,
                value,
                placeholder,
                action,
            } => {
                let action = action.clone();
                // A declared input is a real field: the state entity keeps what
                // the reader typed across frames, and Apply sends it as
                // `{"value": ...}` instead of an empty payload.
                let entity = self.input_for(path, value, placeholder, window, cx);
                let read_path = path.to_string();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(label.clone(), 12.0, self.palette.muted))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(30.0))
                                    .child(Input::new(&entity).w_full()),
                            )
                            .child(
                                Button::new(format!("extension-apply-{path}"))
                                    .debug_selector(|| format!("extension-apply-{path}"))
                                    .accessibility_label(label.clone())
                                    .px(px(10.0))
                                    .h(px(30.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(self.palette.accent))
                                    .child(txt("Apply", 12.0, self.palette.page))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        let text = this.input_value(&read_path, cx);
                                        this.dispatch(&action, json!({ "value": text }), cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            UiNode::Select {
                label,
                value,
                options,
                action,
            } => {
                let action = action.clone();
                let mut row = div().flex().flex_col().gap(px(4.0)).child(txt(
                    if value.is_empty() {
                        label.clone()
                    } else {
                        format!("{label}: {value}")
                    },
                    12.0,
                    self.palette.muted,
                ));
                let mut chips = div().flex().flex_row().flex_wrap().gap(px(6.0));
                for option in options {
                    let action = action.clone();
                    let option_value = option.value.clone();
                    chips = chips.child(
                        Button::new(format!("extension-option-{path}-{}", option.value))
                            .debug_selector(|| format!("extension-option-{path}-{}", option.value))
                            .accessibility_label(option.label.clone())
                            .px(px(9.0))
                            .h(px(26.0))
                            .rounded(px(7.0))
                            .bg(rgb(self.palette.new_session_fill))
                            .child(txt(option.label.clone(), 12.0, self.palette.text))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.dispatch(&action, json!({ "value": option_value.clone() }), cx)
                            })),
                    );
                }
                row = row.child(chips);
                row.into_any_element()
            }
            UiNode::Checkbox {
                label,
                checked,
                action,
            } => {
                let action = action.clone();
                let next = !*checked;
                Button::new(format!("extension-check-{path}"))
                    .debug_selector(|| format!("extension-check-{path}"))
                    .accessibility_label(label.clone())
                    .h(px(26.0))
                    .px(px(6.0))
                    .rounded(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(if *checked {
                        icon(IconName::Check, 14.0, self.palette.accent).into_any_element()
                    } else {
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(rgb(self.palette.border))
                            .into_any_element()
                    })
                    .child(txt(label.clone(), 13.0, self.palette.text))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.dispatch(&action, json!({ "checked": next }), cx)
                    }))
                    .into_any_element()
            }
            UiNode::List { items } => {
                let mut list = div().flex().flex_col().gap(px(2.0)).w_full();
                for (index, item) in items.iter().enumerate() {
                    let mut content = vec![txt(item.title.clone(), 13.0, self.palette.text)];
                    if !item.detail.is_empty() {
                        content.push(txt(item.detail.clone(), 11.0, self.palette.faint));
                    }
                    if let Some(action) = &item.action {
                        let action = action.clone();
                        let mut row = Button::new(format!("extension-list-{path}-{index}"))
                            .debug_selector(|| format!("extension-list-{path}-{index}"))
                            .w_full()
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .flex()
                            .flex_col()
                            .items_start()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.dispatch(&action, json!({ "index": index }), cx)
                            }));
                        for child in content {
                            row = row.child(child);
                        }
                        list = list.child(row);
                    } else {
                        let mut row = div()
                            .id(SharedString::from(format!("extension-list-{path}-{index}")))
                            .w_full()
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .flex()
                            .flex_col();
                        for child in content {
                            row = row.child(child);
                        }
                        list = list.child(row);
                    }
                }
                list.into_any_element()
            }
            UiNode::Section {
                title,
                open,
                children,
            } => {
                // The document's default is recorded the first time the node is
                // drawn; afterwards the reader's toggle wins, including across
                // an auto-refresh that rebuilds the document.
                let is_open = {
                    let mut sections = self.sections.borrow_mut();
                    *sections.entry(path.to_string()).or_insert(*open)
                };
                let toggle = path.to_string();
                let next = !is_open;
                let mut section = div().flex().flex_col().gap(px(6.0)).w_full();
                section = section.child(
                    Button::new(format!("extension-section-{path}"))
                        .debug_selector(|| format!("extension-section-{path}"))
                        .accessibility_label(title.clone())
                        .w_full()
                        .px(px(2.0))
                        .h(px(28.0))
                        .bg(rgb(self.palette.page))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    icon(
                                        if is_open {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        },
                                        12.0,
                                        self.palette.muted,
                                    )
                                    .into_any_element(),
                                )
                                .child(txt(title.clone(), 13.0, self.palette.text)),
                        )
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.sections.borrow_mut().insert(toggle.clone(), next);
                            cx.notify();
                        })),
                );
                if is_open {
                    let mut body = div().flex().flex_col().gap(px(6.0)).w_full().pl(px(14.0));
                    for (index, child) in children.iter().enumerate() {
                        body = body.child(self.render_node(
                            child,
                            &format!("{path}-{index}"),
                            window,
                            cx,
                        ));
                    }
                    section = section.child(body);
                }
                section.into_any_element()
            }
            UiNode::Table {
                columns,
                rows,
                widths,
            } => {
                // A table is a grid, not a wall of text: the header is quieter
                // and every row is ruled off, so a twelve-row capture table can
                // be read across without losing the row. Weights let a package
                // say which column needs the room.
                let mut table = div().flex().flex_col().w_full();
                let mut header = div().flex().flex_row().gap(px(8.0)).w_full().pb(px(3.0));
                for (index, column) in columns.iter().enumerate() {
                    let weight = widths.get(index).copied().unwrap_or(1.0).max(0.01);
                    header = header.child(
                        div()
                            .flex_grow(weight)
                            .flex_basis(px(0.0))
                            .min_w_0()
                            .child(txt(column.clone(), 11.0, self.palette.muted)),
                    );
                }
                table = table
                    .child(hairline_bottom(header).border_color(rgb(self.palette.border_soft)));
                for row in rows {
                    let mut line = div().flex().flex_row().gap(px(8.0)).w_full().py(px(3.0));
                    for (index, cell) in row.iter().enumerate() {
                        let weight = widths.get(index).copied().unwrap_or(1.0).max(0.01);
                        line = line.child(
                            div()
                                .flex_grow(weight)
                                .flex_basis(px(0.0))
                                .min_w_0()
                                .child(txt(cell.clone(), 12.0, self.palette.text)),
                        );
                    }
                    table = table
                        .child(hairline_bottom(line).border_color(rgb(self.palette.border_soft)));
                }
                table.into_any_element()
            }
            UiNode::Tree { nodes } => {
                let mut tree = div().flex().flex_col().gap(px(2.0));
                for node in nodes {
                    tree = tree.child(self.render_tree(node, path, 0));
                }
                tree.into_any_element()
            }
            UiNode::Tabs { tabs } => {
                let selected = self
                    .tabs
                    .get(path)
                    .copied()
                    .unwrap_or(0)
                    .min(tabs.len().saturating_sub(1));
                let mut body = div().flex().flex_col().gap(px(8.0)).w_full();
                let mut strip = div().flex().flex_row().gap(px(6.0));
                let tab_path = path.to_string();
                for (index, tab) in tabs.iter().enumerate() {
                    let active = index == selected;
                    let mut control = Button::new(format!("extension-tab-{path}-{index}"))
                        .debug_selector(|| format!("extension-tab-{path}-{index}"))
                        .accessibility_label(tab.label.clone())
                        .h(px(26.0))
                        .px(px(8.0))
                        .rounded(px(7.0));
                    if active {
                        control = control.bg(rgb(self.palette.new_session_fill));
                    }
                    strip = strip.child(
                        control
                            .child(txt(
                                tab.label.clone(),
                                12.0,
                                if active {
                                    self.palette.text
                                } else {
                                    self.palette.muted
                                },
                            ))
                            .on_click(cx.listener({
                                let tab_path = tab_path.clone();
                                move |this, _event, _window, cx| {
                                    this.tabs.insert(tab_path.clone(), index);
                                    cx.notify();
                                }
                            })),
                    );
                }
                body = body.child(strip);
                if let Some(tab) = tabs.get(selected) {
                    body = body.child(self.render_node(
                        &tab.content,
                        &format!("{path}-tab-{selected}"),
                        window,
                        cx,
                    ));
                }
                body.into_any_element()
            }
            UiNode::Progress { value, label } => {
                let fraction: f32 = (*value).clamp(0.0, 1.0);
                let width = (220.0 * fraction).max(0.0);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(label.clone(), 12.0, self.palette.muted))
                    .child(
                        div()
                            .w(px(220.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(rgb(self.palette.new_session_fill))
                            .child(
                                div()
                                    .w(px(width))
                                    .h(px(6.0))
                                    .rounded(px(3.0))
                                    .bg(rgb(self.palette.accent)),
                            ),
                    )
                    .into_any_element()
            }
            UiNode::Code {
                language,
                value,
                tone,
            } => {
                let mut code = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .p(px(8.0))
                    .rounded(px(7.0))
                    .bg(rgb(self.palette.new_session_fill));
                if !language.is_empty() {
                    code = code.child(txt(language.clone(), 11.0, self.palette.faint));
                }
                code.child(
                    txt(value.clone(), 11.5, self.tone_color(*tone))
                        .font_family(theme::CODE_FONT)
                        .line_height(px(16.0)),
                )
                .into_any_element()
            }
            UiNode::Image { src, alt } => {
                if let Some(asset) = src.strip_prefix("asset/") {
                    if let Some(root) = &self.panel_root {
                        let path = root.join(asset);
                        if path.is_file() {
                            return img(path).max_w(relative(1.0)).into_any_element();
                        }
                    }
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(txt(
                        if alt.is_empty() {
                            src.clone()
                        } else {
                            alt.clone()
                        },
                        12.0,
                        self.palette.muted,
                    ))
                    .child(txt(src.clone(), 11.0, self.palette.faint))
                    .into_any_element()
            }
            UiNode::Notification { level, text } => {
                let color = match level {
                    NotificationLevel::Info => self.palette.accent,
                    NotificationLevel::Warning => self.palette.warn,
                    NotificationLevel::Error => self.palette.error,
                    NotificationLevel::Success => self.palette.success,
                };
                txt(text.clone(), 12.0, color).into_any_element()
            }
            UiNode::Divider => div()
                .h(px(1.0))
                .w_full()
                .bg(rgb(self.palette.border_soft))
                .into_any_element(),
            UiNode::Spacer { size } => div().h(px(*size as f32)).into_any_element(),
        }
    }

    fn render_tree(
        &self,
        node: &ferris_sdk::ui::UiTreeNode,
        path: &str,
        depth: usize,
    ) -> AnyElement {
        let mut column = div().flex().flex_col().gap(px(2.0));
        column = column.child(div().pl(px(depth as f32 * 12.0)).child(txt(
            node.label.clone(),
            12.0,
            self.palette.text,
        )));
        for (index, child) in node.children.iter().enumerate() {
            column =
                column.child(self.render_tree(child, &format!("{path}-tree-{index}"), depth + 1));
        }
        column.into_any_element()
    }
}

/// Where a panel mounts, as the picker spells it.
fn location_label(location: PanelLocation) -> &'static str {
    match location {
        PanelLocation::Right => "right",
        PanelLocation::Left => "left",
        PanelLocation::Bottom => "bottom",
        PanelLocation::Center => "center",
    }
}

/// The palette a panel should use in the current mode: the dark variant in
/// dark mode (falling back to the light one), the light palette otherwise.
fn panel_palette<'a>(
    light: &'a Option<ThemePalette>,
    dark: &'a Option<ThemePalette>,
) -> Option<&'a ThemePalette> {
    if theme::is_dark() {
        dark.as_ref().or(light.as_ref())
    } else {
        light.as_ref()
    }
}

/// The surface a call must use, when the stored one has been retired.
///
/// A view keeps the `PanelSurface` it was built with, and a host rebuild shuts
/// that surface's runtime down before building the package again. Calling the
/// dead handle is what surfaced `extension lazy runtime error: runtime is shut
/// down`; a retired surface is swapped for the current generation's instead.
/// `None` means the stored surface is still the live one.
fn live_surface(
    stored: &PanelSurface,
    resolve: &dyn Fn(&str) -> Option<PanelSurface>,
) -> Option<PanelSurface> {
    if !stored.runtime.is_shutdown() {
        return None;
    }
    resolve(&stored.panel_id).filter(|fresh| !Arc::ptr_eq(&fresh.runtime, &stored.runtime))
}

fn summarize(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
    .chars()
    .take(4096)
    .collect()
}

impl Render for ExtensionPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The palette is recomputed per frame so a light/dark switch lands
        // without rebuilding the pane, and a package theme's dark variant wins
        // in dark mode.
        self.palette = Palette::new(panel_palette(&self.panel_theme, &self.panel_theme_dark));
        self.ensure_loaded(cx);
        self.schedule_refresh(cx);
        self.ensure_picker(window, cx);
        // Record what this frame draws from; a click later in the frame's life
        // is an action on this revision, not on whatever replaced it since.
        self.render_revision = self.revision;
        let mut root = div()
            .debug_selector(|| "extension-panel-view".to_string())
            .flex()
            .flex_col()
            .gap(px(12.0))
            .w_full()
            .h_full();
        if self.panels.is_empty() {
            return root.child(txt(
                tr(self.lang, "right.extensions.empty"),
                13.0,
                self.palette.muted,
            ));
        }
        // One searchable dropdown over every panel installed for this mount.
        // The pill strip it replaces had to be scanned left to right, and a
        // reader with a dozen packages had no way to ask for one by name.
        let mut chooser = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .w_full()
            .debug_selector(|| "extension-panel-chooser".to_string())
            .child(txt(
                tr(self.lang, "right.extensions.picker"),
                12.0,
                self.palette.muted,
            ));
        if let Some(picker) = self.picker.clone() {
            chooser = chooser.child(
                div()
                    .flex_1()
                    .h(px(30.0))
                    .debug_selector(|| "extension-panel-picker".to_string())
                    .child(
                        Combobox::new(&picker)
                            .placeholder(tr(self.lang, "right.extensions.picker"))
                            .search_placeholder(tr(self.lang, "right.extensions.search"))
                            .menu_width(px(320.0)),
                    ),
            );
        }
        chooser = chooser.child(
            Button::new("extension-panel-refresh")
                .debug_selector(|| "extension-panel-refresh".to_string())
                .accessibility_label(tr(self.lang, "right.extensions.refresh"))
                .px(px(10.0))
                .h(px(30.0))
                .rounded(px(8.0))
                .bg(rgb(self.palette.new_session_fill))
                .border_1()
                .border_color(rgb(self.palette.border_soft))
                .child(txt(
                    tr(self.lang, "right.extensions.refresh"),
                    12.0,
                    self.palette.text,
                ))
                .on_click(cx.listener(|this, _event, _window, cx| this.refresh(cx))),
        );
        root = root.child(chooser);
        if self.loading {
            root = root.child(txt(
                tr(self.lang, "right.extensions.loading"),
                12.0,
                self.palette.muted,
            ));
        }
        if let Some(error) = &self.error {
            root = root.child(txt(
                tr_args(
                    self.lang,
                    "right.extensions.panelError",
                    &[("error", error.clone())],
                ),
                12.0,
                self.palette.error,
            ));
        }
        if let Some(message) = &self.message {
            root = root.child(txt(message.clone(), 12.0, self.palette.muted));
        }
        if let Some(document) = &self.document {
            let mut body = div()
                .id("extension-panel-body")
                .flex()
                .flex_col()
                .gap(px(8.0))
                .w_full()
                .overflow_y_scroll();
            body = body.child(self.render_node(&document.root, "root", window, cx));
            root = root.child(body);
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::{live_surface, ExtensionPanelView, PanelItem};
    use crate::extension::host::PanelSurface;
    use crate::i18n::Lang;
    use crate::theme;
    use async_trait::async_trait;
    use ferris_sdk::host_runtime::{CallContext, Runtime, ToolDescriptor};
    use ferris_sdk::types::{ExtensionError, ExtensionId, RuntimeKind};
    use ferris_sdk::ui::{PanelLocation, ThemePalette, UiDocument};
    use gpui_kit::component::searchable_list::SearchableListItem;
    use serde_json::Value;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Noop;

    #[async_trait]
    impl Runtime for Noop {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Declarative
        }
        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }
        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }
    }

    /// A runtime whose life-cycle state the test controls.
    ///
    /// `is_shutdown` is what the rebind path keys on, so the state has to be
    /// settable without standing up a real host and rebuilding it.
    struct Swappable {
        alive: AtomicBool,
    }

    impl Swappable {
        fn alive() -> Self {
            Self {
                alive: AtomicBool::new(true),
            }
        }

        fn retired() -> Self {
            Self {
                alive: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl Runtime for Swappable {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Declarative
        }

        fn is_shutdown(&self) -> bool {
            !self.alive.load(Ordering::Relaxed)
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }

        async fn panel(&self, _id: &str) -> Result<Value, ExtensionError> {
            Ok(
                serde_json::json!({"schema_version": "1.0", "root": {"type": "text", "text": "live"}}),
            )
        }
    }

    fn surface_with(runtime: Arc<dyn Runtime>, initial: Option<UiDocument>) -> PanelSurface {
        PanelSurface {
            panel_id: "com.example.ui.greeting".to_string(),
            contribution_id: "greeting".to_string(),
            title: "Greeting".to_string(),
            icon: None,
            location: PanelLocation::Right,
            visibility: ferris_sdk::ui::PanelVisibility::Visible,
            runtime,
            owner: ExtensionId::parse("com.example.ui").expect("id"),
            theme: None,
            theme_dark: None,
            initial,
            surface: None,
            refresh_ms: None,
            policy: Arc::new(
                ferris_sdk::capability::PolicyEngine::open(
                    std::env::temp_dir().join(format!("bsh-panel-grants-{}", std::process::id())),
                    std::env::temp_dir().join(format!("bsh-panel-audit-{}", std::process::id())),
                    ferris_sdk::capability::PolicyMode::Advisory,
                )
                .expect("policy"),
            ),
            trust: ferris_sdk::types::TrustLevel::Local,
            tools: Arc::new(Vec::new()),
            workspace: None,
            root: std::path::PathBuf::from("/nonexistent-panel-root"),
        }
    }

    fn panel(document: UiDocument) -> PanelSurface {
        surface_with(Arc::new(Noop), Some(document))
    }

    #[test]
    fn an_initial_document_is_validated_before_it_is_shown() {
        let document: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "revision": 1,
            "root": {
                "type": "column",
                "children": [
                    {"type": "heading", "text": "Hello", "level": 1},
                    {"type": "text", "text": "From an extension"}
                ]
            }
        }))
        .expect("document");
        let view = ExtensionPanelView::new(vec![panel(document)], Lang::En);
        assert!(view.document.is_some());
        assert!(view.revision >= 1);
        assert!(view.error.is_none());
        assert_eq!(view.panels.len(), 1);
    }

    #[test]
    fn a_panel_theme_overrides_the_host_palette() {
        let document: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "revision": 1,
            "root": {"type": "text", "text": "Themed"}
        }))
        .expect("document");
        let mut surface = panel(document);
        let mut colors = std::collections::BTreeMap::new();
        colors.insert("page".to_string(), "#112233".to_string());
        colors.insert("accent".to_string(), "#445566".to_string());
        surface.theme = Some(ThemePalette {
            name: "Test theme".to_string(),
            colors,
        });
        let view = ExtensionPanelView::new(vec![surface], Lang::En);
        assert_eq!(view.palette.page, 0x112233);
        assert_eq!(view.palette.accent, 0x445566);
        assert_eq!(view.palette.text, theme::text());
    }

    /// Every installed panel is one row of the picker, and the row carries the
    /// panel's own index: the dropdown is searchable, so a row number in the
    /// filtered list is not a panel index and must never stand in for one.
    #[test]
    fn the_picker_lists_every_panel_with_its_mount() {
        let document: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "revision": 1,
            "root": {"type": "text", "text": "x"}
        }))
        .expect("document");
        let mut right = panel(document.clone());
        right.title = "Greeting".to_string();
        right.location = PanelLocation::Right;
        let mut left = panel(document);
        left.title = "Second".to_string();
        left.location = PanelLocation::Left;

        let items = ExtensionPanelView::picker_items(&[right, left]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].value(), &0);
        assert_eq!(items[0].title().to_string(), "Greeting · right");
        assert_eq!(items[1].value(), &1);
        assert_eq!(items[1].title().to_string(), "Second · left");
    }

    /// A declared view must not stop the runtime's live card from loading: it
    /// is the placeholder, not the answer. A package that ships both used to
    /// freeze on the static copy and never show the live one.
    #[test]
    fn a_declared_view_still_fetches_the_live_card() {
        let document: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "revision": 1,
            "root": {"type": "text", "text": "declared"}
        }))
        .expect("document");
        let mut view = ExtensionPanelView::new(vec![panel(document)], Lang::En);
        assert!(
            view.document.is_some(),
            "the declared view is shown at once"
        );
        assert!(view.should_fetch(), "and the live card is fetched next");
        view.attempted = true;
        assert!(!view.should_fetch(), "one fetch per selection");
    }

    /// A live card that fails must not blank a declared view; the note says why
    /// while the declared content stays. A panel with nothing to show reports
    /// the error instead, because an empty pane would hide the failure.
    #[test]
    fn a_failed_live_card_keeps_a_declared_view_on_screen() {
        let document: UiDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "revision": 1,
            "root": {"type": "text", "text": "declared"}
        }))
        .expect("document");
        let mut view = ExtensionPanelView::new(vec![panel(document)], Lang::En);
        view.live_failure("boom".to_string());
        assert!(view.document.is_some());
        assert!(view.error.is_none());
        assert!(view
            .message
            .as_deref()
            .is_some_and(|message| message.contains("boom")));

        let mut bare = ExtensionPanelView::new(vec![surface_with(Arc::new(Noop), None)], Lang::En);
        bare.live_failure("boom".to_string());
        assert!(bare
            .error
            .as_deref()
            .is_some_and(|error| error.contains("boom")));
    }

    /// A pane that outlived a host rebuild must ask the new runtime, not the
    /// dead handle it was built with. This is the regression behind
    /// `Panel error: extension lazy runtime error: runtime is shut down`: the
    /// stored surface is retired, the host has a replacement, and the call is
    /// made against the replacement.
    #[test]
    fn a_retired_panel_moves_to_the_current_generations_runtime() {
        let stored = surface_with(Arc::new(Swappable::retired()), None);
        let fresh = surface_with(Arc::new(Swappable::alive()), None);
        let looked_up = AtomicUsize::new(0);
        let resolve = |id: &str| {
            looked_up.fetch_add(1, Ordering::Relaxed);
            (id == stored.panel_id).then(|| fresh.clone())
        };
        let resolved = live_surface(&stored, &resolve).expect("the live generation");
        assert_eq!(resolved.panel_id, stored.panel_id);
        assert!(
            !resolved.runtime.is_shutdown(),
            "the replacement is the runtime the call uses"
        );
        assert!(!Arc::ptr_eq(&resolved.runtime, &stored.runtime));
        assert_eq!(looked_up.load(Ordering::Relaxed), 1, "one lookup");
    }

    /// A live surface is never relooked-up: the check is one atomic read, and
    /// resolving on every render would put a host lock on the panel path.
    #[test]
    fn a_live_panel_is_never_relooked_up() {
        let stored = surface_with(Arc::new(Swappable::alive()), None);
        let resolved = live_surface(&stored, &|_: &str| -> Option<PanelSurface> {
            panic!("a live panel must not be relooked up")
        });
        assert!(resolved.is_none(), "the stored surface is still live");
    }

    /// A retired surface with no replacement keeps its own error: guessing at a
    /// package that is gone would be worse than reporting the shutdown.
    #[test]
    fn a_retired_panel_without_a_replacement_stays_put() {
        let stored = surface_with(Arc::new(Swappable::retired()), None);
        assert!(live_surface(&stored, &|_| None).is_none());
    }

    /// The search field filters on the row text, case-insensitively, which is
    /// the whole point of replacing the pill strip with a dropdown.
    #[test]
    fn the_picker_search_matches_a_row_case_insensitively() {
        let item = PanelItem {
            index: 0,
            label: "Extension DevKit · right".to_string(),
        };
        assert!(item.matches("devkit"));
        assert!(item.matches("RIGHT"));
        assert!(!item.matches("federation"));
    }
}
