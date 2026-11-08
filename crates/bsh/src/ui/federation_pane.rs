//! The Federation pane.
//!
//! One surface for the node, the peers it trusts, the work it delegates, the
//! resources it can reach and the knowledge it serves. The layout is the
//! reference's own card stack: a gradient hero that reports health and offers
//! the three node actions, a segmented tab row, and one card per concern.
//!
//! Two rules shape the code:
//!
//! - The ordinary surface never asks for an id. A peer is trusted from the row
//!   that announces it, a grant is a peer then a capability this node offers,
//!   publishing is a target peer and a payload,
//!   and exposing a capability is a row checked against the node's own list.
//!   The raw fields (a node id to enroll by hand, the expose root, a lease id
//!   and a resource uri) live behind Advanced.
//! - Every action reports one state. [\`FederationActivity\`] is the pane's one
//!   operation log, so a store write, a remote query and a node restart all say
//!   pending, running, succeeded, failed, revoked or cancelled in the same
//!   place, with the target they touched.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

use super::right_panel::{
    display_name, enroll_and_grant, federation_status_off, grant_capability, parse_peer,
    revoke_grant_action, revoke_peer_action, short_node, FederationPeerRow,
};
use super::shell::Shell;
use super::{hairline_all, hairline_bottom, icon, txt, DivExt, IconName};
use crate::theme;

/// The card corner the design uses for every section.
const CARD_RADIUS: f32 = 16.0;
/// The card's own inset.
const CARD_PAD: f32 = 14.0;
/// A row's corner inside a card.
const ROW_RADIUS: f32 = 12.0;
/// The stack gap between cards.
const CARD_GAP: f32 = 12.0;
/// A violet accent for the identity and work sections, so the four tabs keep
/// four identities instead of repeating one blue.
const VIOLET: u32 = 0x7C5CFC;

/// The state every federation action reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FederationActivityState {
    /// The action is waiting on a confirmation or a selection.
    #[default]
    Pending,
    /// The action is in flight across the node or the wire.
    Running,
    /// The action completed.
    Succeeded,
    /// The action was refused or failed.
    Failed,
    /// The peer's grants were withdrawn.
    Revoked,
    /// The peer record was removed from this node's list.
    Removed,
    /// The reader backed out before the action ran.
    Cancelled,
}

/// One entry in the pane's unified operation log.
#[derive(Debug, Clone)]
pub(crate) struct FederationActivity {
    /// Stable id, so an async completion settles the row it started.
    pub(crate) id: u64,
    /// The action's label key.
    pub(crate) action: &'static str,
    /// What the action touched: a peer, a capability, a node.
    pub(crate) target: String,
    /// Where the action stands.
    pub(crate) state: FederationActivityState,
    /// The result or the refusal, when there is one worth keeping.
    pub(crate) detail: Option<String>,
}

/// One artifact preview the Resources tab read from disk.
#[derive(Debug, Clone)]
pub(crate) struct FederationPreview {
    /// The file name the reader pressed.
    pub(crate) title: String,
    /// The bounded text the pane shows.
    pub(crate) body: String,
}

impl Shell {
    /// Start one operation and return the id its completion settles.
    pub(crate) fn fed_op_begin(&mut self, action: &'static str, target: impl Into<String>) -> u64 {
        let pane = &mut self.right.federation;
        pane.activity_seq = pane.activity_seq.wrapping_add(1).max(1);
        let id = pane.activity_seq;
        pane.activity.push(FederationActivity {
            id,
            action,
            target: target.into(),
            state: FederationActivityState::Pending,
            detail: None,
        });
        // The log is a glance, not history: the audit receipts action owns the
        // durable record. Keep the newest eight so a burst of retries cannot
        // grow a frame.
        while pane.activity.len() > 8 {
            pane.activity.remove(0);
        }
        id
    }

    /// Move one operation to a state, keeping any detail already recorded.
    pub(crate) fn fed_op_set(
        &mut self,
        id: u64,
        state: FederationActivityState,
        detail: Option<String>,
    ) {
        if let Some(operation) = self
            .right
            .federation
            .activity
            .iter_mut()
            .find(|operation| operation.id == id)
        {
            operation.state = state;
            if detail.is_some() {
                operation.detail = detail;
            }
        }
    }

    /// Settle one operation from a result.
    pub(crate) fn fed_op_result(&mut self, id: u64, result: &Result<String, String>) {
        match result {
            Ok(note) => self.fed_op_set(
                id,
                FederationActivityState::Succeeded,
                (!note.is_empty()).then(|| note.clone()),
            ),
            Err(error) => self.fed_op_set(id, FederationActivityState::Failed, Some(error.clone())),
        }
    }
}

/// The pane the Federation tab draws.
pub(crate) fn pane(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let runtime = crate::federation_node::running();
    let tab = shell.right.federation.tab;

    let mut content = div()
        .flex()
        .flex_col()
        .gap(px(CARD_GAP))
        .p(px(14.0))
        .w_full();

    content = content.child(hero(shell, runtime.as_ref(), cx));

    if let Some(error) = &shell.right.federation.error {
        content = content.child(banner(error.clone(), false));
    }
    if let Some(notice) = &shell.right.federation.notice {
        content = content.child(banner(notice.clone(), true));
    }
    if !shell.right.federation.activity.is_empty() {
        content = content.child(activity_card(shell, cx));
    }

    content = content.child(tabs(shell, cx));

    if shell.right.federation.advanced {
        content = content.child(advanced_card(shell, cx));
    }

    content = content.child(match tab {
        1 => work_tab(shell, runtime.as_ref(), cx),
        2 => resources_tab(shell, cx),
        3 => knowledge_tab(shell, cx),
        _ => peers_tab(shell, runtime.as_ref(), cx),
    });

    if !shell.right.federation.advanced {
        content = content.child(advanced_footer(shell, cx));
    }

    div()
        .id("federation-pane")
        .debug_selector(|| "federation-pane".to_string())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .w_full()
        .bg(rgb(theme::new_session_fill()))
        .overflow_y_scroll()
        .child(content)
        .into_any_element()
}

/// One opaque packed RGB colour as a gradient stop.
///
/// gpui-pre's `rgba` reads its argument as `0xRRGGBBAA`, so an RGB token (the
/// shape every `theme` accessor returns) must be shifted up and given a full
/// alpha byte; passing it directly made the first version of the hero render
/// cyan, because the blue byte became the alpha and red became zero.
fn opaque(color: u32) -> Rgba {
    rgba((color << 8) | 0xFF)
}

// ---------------------------------------------------------------------------
// The hero: identity, health and the three node actions.
// ---------------------------------------------------------------------------

fn hero(
    shell: &Shell,
    runtime: Option<&std::sync::Arc<crate::federation_node::FederationRuntime>>,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let lang = shell.state.lang;
    let profile =
        crate::federation::resolve_profile(Some(&shell.state.settings.global.federation.profile));
    let status = match runtime {
        Some(runtime) => crate::i18n::tr_args(
            lang,
            "right.federation.running",
            &[
                (
                    "node",
                    runtime
                        .node_name()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| crate::federation::display_name(None, &profile)),
                ),
                ("peers", runtime.announced_peers().len().to_string()),
            ],
        ),
        None => federation_status_off(lang, shell.state.settings.global.federation.enabled),
    };
    let healthy = runtime.is_some();

    let mut actions = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0));
    actions = actions.child(action_button(
        "federation-refresh",
        IconName::RefreshCw,
        lang.tr("right.federation.refresh"),
        false,
        cx,
        |this, cx| {
            let target = this.state.lang.tr("right.federation.theNode");
            let op = this.fed_op_begin("right.federation.opRefresh", target);
            this.reload_federation();
            let peers = this.right.federation.discovered.len();
            this.fed_op_set(
                op,
                FederationActivityState::Succeeded,
                Some(crate::i18n::tr_args(
                    this.state.lang,
                    "right.federation.opRefreshDetail",
                    &[("count", peers.to_string())],
                )),
            );
            cx.notify();
        },
    ));
    actions = actions.child(if runtime.is_some() {
        action_button(
            "federation-node-stop",
            IconName::Square,
            lang.tr("right.federation.stop"),
            false,
            cx,
            |this, cx| {
                let target = this.state.lang.tr("right.federation.theNode");
                let op = this.fed_op_begin("right.federation.opStop", target);
                let stopped = crate::federation_node::stop();
                this.fed_op_set(
                    op,
                    if stopped {
                        FederationActivityState::Succeeded
                    } else {
                        FederationActivityState::Failed
                    },
                    None,
                );
                if !stopped {
                    this.right.federation.error =
                        Some(this.state.lang.tr("right.federation.notRunning"));
                }
                this.reload_federation();
                cx.notify();
            },
        )
    } else {
        action_button(
            "federation-node-start",
            IconName::Play,
            lang.tr("right.federation.start"),
            true,
            cx,
            |this, cx| {
                let target = this.state.lang.tr("right.federation.theNode");
                let op = this.fed_op_begin("right.federation.opStart", target);
                this.fed_op_set(op, FederationActivityState::Running, None);
                this.right.federation.notice =
                    Some(this.state.lang.tr("right.federation.starting"));
                this.right.federation.error = None;
                let settings = this.state.settings.clone();
                let handle = crate::extension::spawn_async(async move {
                    crate::federation_node::start_from_settings(&settings)
                });
                cx.spawn(async move |this, cx| {
                    let started = handle.await.unwrap_or(false);
                    let _ = this.update(cx, |this, cx| {
                        this.fed_op_set(
                            op,
                            if started {
                                FederationActivityState::Succeeded
                            } else {
                                FederationActivityState::Failed
                            },
                            None,
                        );
                        if !started {
                            this.right.federation.error =
                                Some(this.state.lang.tr("right.federation.failed"));
                        }
                        this.reload_federation_quiet();
                        cx.notify();
                    });
                })
                .detach();
                cx.notify();
            },
        )
    });
    if runtime.is_some() {
        actions = actions.child(action_button(
            "federation-tls-reload",
            IconName::ShieldCheck,
            lang.tr("right.federation.reloadTls"),
            false,
            cx,
            |this, cx| {
                let target = this.state.lang.tr("right.federation.theNode");
                let op = this.fed_op_begin("right.federation.opReloadTls", target);
                this.fed_op_set(op, FederationActivityState::Running, None);
                this.right.federation.notice =
                    Some(this.state.lang.tr("right.federation.reloadingTls"));
                this.right.federation.error = None;
                let settings = this.state.settings.clone();
                cx.spawn(async move |this, cx| {
                    let reloaded = crate::extension::spawn_async(async move {
                        crate::federation_node::reload_tls(&settings)
                    })
                    .await
                    .unwrap_or(false);
                    let _ = this.update(cx, |this, cx| {
                        this.fed_op_set(
                            op,
                            if reloaded {
                                FederationActivityState::Succeeded
                            } else {
                                FederationActivityState::Failed
                            },
                            None,
                        );
                        cx.notify();
                    });
                })
                .detach();
                cx.notify();
            },
        ));
    }

    let mark = div()
        .size(px(46.0))
        .flex_none()
        .rounded(px(15.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(
            135.0,
            linear_color_stop(opaque(theme::accent()), 0.0),
            linear_color_stop(opaque(VIOLET), 1.0),
        ))
        .child(icon(IconName::GitBranch, 23.0, 0xFFFFFF));

    let titles = div()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .flex_1()
        .min_w_0()
        .child(
            txt(lang.tr("right.federation.title"), 23.0, theme::text())
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            txt(lang.tr("right.federation.subtitle"), 12.0, theme::muted()).line_height(px(17.0)),
        );

    let mut status_row = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .flex_wrap()
        .child(dot(if healthy {
            theme::success()
        } else {
            theme::faint()
        }))
        .child(
            txt(status, 12.0, theme::muted())
                .flex_1()
                .min_w_0()
                .text_ellipsis(),
        )
        .child(pill(
            if healthy {
                lang.tr("right.federation.healthy")
            } else {
                lang.tr("right.federation.offline")
            },
            if healthy {
                theme::success()
            } else {
                theme::faint()
            },
            if healthy {
                theme::soft(theme::success())
            } else {
                theme::hover()
            },
        ));
    let trusted = shell.right.federation.peers.len();
    if trusted > 0 {
        status_row = status_row.child(pill(
            crate::i18n::tr_args(
                lang,
                "right.federation.trustedCount",
                &[("count", trusted.to_string())],
            ),
            theme::accent(),
            theme::soft(theme::accent()),
        ));
    }

    let card = div()
        .relative()
        .overflow_hidden()
        .rounded(px(22.0))
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .bg(linear_gradient(
            135.0,
            linear_color_stop(
                opaque(theme::blend_into(theme::accent(), theme::page(), 0.84)),
                0.0,
            ),
            linear_color_stop(opaque(theme::page()), 1.0),
        ))
        .child(
            div()
                .absolute()
                .top(px(-16.0))
                .right(px(-16.0))
                .opacity(0.07)
                .child(icon(IconName::Network, 128.0, theme::accent())),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(mark)
                .child(titles),
        )
        .child(actions)
        .child(status_row);

    hairline_all(card)
        .border_color(rgb(theme::border_soft()))
        .shadow_sm()
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Tabs and the shared card vocabulary.
// ---------------------------------------------------------------------------

fn tabs(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let current = shell.right.federation.tab;
    let entries = [
        (0usize, "right.federation.tabPeers", IconName::Users),
        (1, "right.federation.tabWork", IconName::Workflow),
        (2, "right.federation.tabResources", IconName::Boxes),
        (3, "right.federation.tabKnowledge", IconName::BookOpen),
    ];

    let mut row = div()
        .flex()
        .items_center()
        .gap(px(3.0))
        .p(px(3.0))
        .rounded(px(13.0))
        .bg(rgb(theme::new_session_fill()));
    for (index, key, glyph) in entries {
        let active = current == index;
        let id = SharedString::from(format!("federation-tab-{index}"));
        row = row.child(
            div()
                .id(id)
                .debug_selector(move || format!("federation-tab-{index}"))
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_center()
                .gap(px(5.0))
                .h(px(32.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .when(active, |tab| tab.bg(rgb(theme::page())).shadow_sm())
                .child(icon(
                    glyph,
                    13.0,
                    if active {
                        theme::accent()
                    } else {
                        theme::faint()
                    },
                ))
                .child(
                    txt(
                        lang.tr(key),
                        12.0,
                        if active {
                            theme::text()
                        } else {
                            theme::faint()
                        },
                    )
                    .font_weight(if active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .min_w_0()
                    .text_ellipsis(),
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.right.federation.tab = index;
                    this.right.federation.error = None;
                    this.right.federation.notice = None;
                    cx.notify();
                })),
        );
    }
    row.into_any_element()
}

fn card() -> Div {
    hairline_all(
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(CARD_PAD))
            .rounded(px(CARD_RADIUS))
            .bg(rgb(theme::page())),
    )
    .border_color(rgb(theme::border_soft()))
    .shadow_sm()
}

fn card_header(
    mark: IconName,
    tint: u32,
    title: String,
    subtitle: String,
    trailing: Option<AnyElement>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(icon_tile(mark, tint))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(txt(title, 14.0, theme::text()).font_weight(FontWeight::MEDIUM))
                .child(txt(subtitle, 11.0, theme::faint()).line_height(px(15.0))),
        )
        .when_some(trailing, |header, trailing| header.child(trailing))
}

fn icon_tile(mark: IconName, tint: u32) -> Div {
    div()
        .size(px(32.0))
        .flex_none()
        .rounded(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(theme::soft(tint)))
        .child(icon(mark, 16.0, tint))
}

fn pill(label: impl Into<SharedString>, fg: u32, bg: u32) -> Div {
    div()
        .flex_none()
        .h(px(20.0))
        .px(px(7.0))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(4.0))
        .bg(rgb(bg))
        .child(txt(label, 10.0, fg).font_weight(FontWeight::MEDIUM))
}

fn dot(color: u32) -> Div {
    div()
        .size(px(6.0))
        .flex_none()
        .rounded_full()
        .bg(rgb(color))
}

fn action_button<F>(
    id: &'static str,
    mark: IconName,
    label: String,
    primary: bool,
    cx: &mut Context<Shell>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut Shell, &mut Context<Shell>) + 'static,
{
    let (fg, bg, border) = if primary {
        (0xFFFFFF, theme::accent(), theme::accent())
    } else {
        (theme::text(), theme::page(), theme::border())
    };
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(9.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(border))
        .cursor_pointer()
        .hover(|style| style.opacity(0.88))
        .child(icon(mark, 13.0, fg))
        .child(txt(label, 12.0, fg))
        .on_click(cx.listener(move |this, _event, _window, cx| action(this, cx)))
        .into_any_element()
}

fn icon_button<F>(
    id: impl Into<SharedString>,
    mark: IconName,
    tip: String,
    color: u32,
    cx: &mut Context<Shell>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut Shell, &mut Context<Shell>) + 'static,
{
    let id = id.into();
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .size(px(26.0))
        .flex_none()
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(theme::hover())))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(icon(mark, 14.0, color))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            cx.stop_propagation();
            action(this, cx);
        }))
        .into_any_element()
}

fn banner(text: String, good: bool) -> AnyElement {
    let color = if good {
        theme::success()
    } else {
        theme::error()
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(12.0))
        .bg(rgb(theme::soft(color)))
        .child(icon(
            if good {
                IconName::CircleCheck
            } else {
                IconName::TriangleAlert
            },
            14.0,
            color,
        ))
        .child(txt(text, 12.0, color).flex_1().min_w_0())
        .into_any_element()
}

fn empty_state(label: String, mark: IconName) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(12.0))
        .rounded(px(ROW_RADIUS))
        .bg(rgb(theme::new_session_fill()))
        .child(icon(mark, 15.0, theme::faint()))
        .child(txt(label, 12.0, theme::faint()).flex_1().min_w_0())
        .into_any_element()
}

fn field(input: &Entity<InputState>, id: &'static str) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .debug_selector(move || id.to_string())
        .h(px(34.0))
        .flex_1()
        .min_w_0()
        .rounded(px(9.0))
        .bg(rgb(theme::page()))
        .border_1()
        .border_color(rgb(theme::border()))
        .px(px(10.0))
        .flex()
        .items_center()
        .child(Input::new(input).appearance(false).w_full())
        .into_any_element()
}

fn solid_button<F>(
    id: impl Into<SharedString>,
    label: String,
    primary: bool,
    enabled: bool,
    cx: &mut Context<Shell>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut Shell, &mut Context<Shell>) + 'static,
{
    let id = id.into();
    let selector = id.to_string();
    let (fg, bg) = if primary {
        (0xFFFFFF, theme::accent())
    } else {
        (theme::text(), theme::new_session_fill())
    };
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .h(px(32.0))
        .px(px(12.0))
        .rounded(px(9.0))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .bg(rgb(bg))
        .when(!primary, |button| {
            button.border_1().border_color(rgb(theme::border()))
        })
        .when(enabled, |button| button.cursor_pointer())
        .when(!enabled, |button| button.opacity(0.45))
        .child(txt(label, 12.0, fg).font_weight(FontWeight::MEDIUM))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            if enabled {
                action(this, cx);
            }
        }))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Activity: one log for every action.
// ---------------------------------------------------------------------------

fn activity_card(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let mut card = card();
    card = card.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(icon(IconName::Activity, 14.0, theme::muted()))
            .child(
                txt(lang.tr("right.federation.activity"), 12.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .flex_1(),
            )
            .child(icon_button(
                "federation-activity-clear",
                IconName::X,
                lang.tr("right.federation.activityClear"),
                theme::faint(),
                cx,
                |this, cx| {
                    this.right.federation.activity.clear();
                    cx.notify();
                },
            )),
    );
    for operation in shell.right.federation.activity.iter().rev().take(4) {
        let color = activity_color(operation.state);
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(rgb(theme::new_session_fill()));
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .child(dot(color))
                .child(
                    txt(lang.tr(operation.action), 11.0, theme::text())
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(
                    txt(
                        if operation.target.chars().count() > 32 {
                            short_node(operation.target.as_str())
                        } else {
                            operation.target.clone()
                        },
                        10.0,
                        theme::faint(),
                    )
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis(),
                )
                .child(pill(
                    activity_label(lang, operation.state),
                    color,
                    theme::soft(color),
                )),
        );
        if let Some(detail) = &operation.detail {
            row = row.child(txt(
                detail.chars().take(140).collect::<String>(),
                10.0,
                theme::faint(),
            ));
        }
        card = card.child(row);
    }
    card.into_any_element()
}

fn activity_color(state: FederationActivityState) -> u32 {
    match state {
        FederationActivityState::Pending => theme::warn(),
        FederationActivityState::Running => theme::accent(),
        FederationActivityState::Succeeded => theme::success(),
        FederationActivityState::Failed => theme::error(),
        FederationActivityState::Revoked => theme::error(),
        FederationActivityState::Removed => theme::muted(),
        FederationActivityState::Cancelled => theme::faint(),
    }
}

fn activity_label(lang: crate::i18n::Lang, state: FederationActivityState) -> String {
    lang.tr(match state {
        FederationActivityState::Pending => "right.federation.statePending",
        FederationActivityState::Running => "right.federation.stateRunning",
        FederationActivityState::Succeeded => "right.federation.stateSucceeded",
        FederationActivityState::Failed => "right.federation.stateFailed",
        FederationActivityState::Revoked => "right.federation.stateRevoked",
        FederationActivityState::Removed => "right.federation.stateRemoved",
        FederationActivityState::Cancelled => "right.federation.stateCancelled",
    })
}

// ---------------------------------------------------------------------------
// Peers.
// ---------------------------------------------------------------------------

fn advanced_footer(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    div()
        .id("federation-advanced-open")
        .debug_selector(|| "federation-advanced-open".to_string())
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(6.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(theme::hover())))
        .child(icon(IconName::SlidersHorizontal, 13.0, theme::faint()))
        .child(
            txt(
                lang.tr("right.federation.advancedTitle"),
                11.0,
                theme::faint(),
            )
            .flex_1(),
        )
        .child(icon(IconName::ChevronRight, 13.0, theme::faint()))
        .on_click(cx.listener(|this, _event, _window, cx| {
            this.right.federation.advanced = true;
            cx.notify();
        }))
        .into_any_element()
}

fn peers_tab(
    shell: &Shell,
    runtime: Option<&std::sync::Arc<crate::federation_node::FederationRuntime>>,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let lang = shell.state.lang;
    let discovered = &shell.right.federation.discovered;
    let online = shell
        .right
        .federation
        .peers
        .iter()
        .filter(|peer| discovered.iter().any(|row| row.node == peer.node))
        .count();

    let mut column = div().flex().flex_col().gap(px(CARD_GAP));

    // Peers: enrolled rows first, then discovered candidates, then the
    // discovery action. The online count is the header's trailing fact.
    let mut peers_card = card();
    peers_card = peers_card.child(card_header(
        IconName::Users,
        theme::accent(),
        lang.tr("right.federation.tabPeers"),
        lang.tr("right.federation.peersSubtitle"),
        Some(
            pill(
                crate::i18n::tr_args(
                    lang,
                    "right.federation.onlineCount",
                    &[("count", online.to_string())],
                ),
                if online == 0 {
                    theme::faint()
                } else {
                    theme::success()
                },
                if online == 0 {
                    theme::hover()
                } else {
                    theme::soft(theme::success())
                },
            )
            .into_any_element(),
        ),
    ));
    if shell.right.federation.peers.is_empty() {
        peers_card = peers_card.child(empty_state(
            lang.tr("right.federation.noPeers"),
            IconName::Users,
        ));
    }
    for (index, peer) in shell.right.federation.peers.iter().enumerate() {
        peers_card = peers_card.child(peer_row(shell, peer, index, cx));
    }

    let candidates: Vec<(usize, &super::right_panel::DiscoveredPeerRow)> = discovered
        .iter()
        .filter(|row| {
            !shell
                .right
                .federation
                .peers
                .iter()
                .any(|peer| peer.node == row.node)
        })
        .take(8)
        .enumerate()
        .collect();
    if !candidates.is_empty() {
        peers_card = peers_card.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .pt(px(2.0))
                .child(txt(
                    lang.tr("right.federation.discovered"),
                    11.0,
                    theme::faint(),
                ))
                .child(pill(
                    candidates.len().to_string(),
                    theme::accent(),
                    theme::soft(theme::accent()),
                )),
        );
        for (index, row) in candidates {
            peers_card = peers_card.child(discovered_row(shell, row, index, cx));
        }
    }

    peers_card = peers_card.child(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pt(px(2.0))
            .child(
                div()
                    .id("federation-discover-more")
                    .debug_selector(|| "federation-discover-more".to_string())
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(28.0))
                    .px(px(4.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::new_session_fill())))
                    .child(icon(IconName::RefreshCw, 13.0, theme::accent()))
                    .child(txt(
                        lang.tr("right.federation.discoverMore"),
                        12.0,
                        theme::accent(),
                    ))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.reload_federation();
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(icon(IconName::ChevronRight, 14.0, theme::faint())),
    );
    column = column.child(peers_card);

    column = column.child(identity_card(shell, runtime, cx));
    column = column.child(grant_card(shell, cx));
    column = column.child(exposed_card(shell, cx));
    column.into_any_element()
}

fn peer_row(
    shell: &Shell,
    peer: &FederationPeerRow,
    index: usize,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let lang = shell.state.lang;
    let selected = shell.right.federation.selected == Some(index);
    let online = shell
        .right
        .federation
        .discovered
        .iter()
        .any(|row| row.node == peer.node);
    let advertised: Vec<String> = shell
        .right
        .federation
        .discovered
        .iter()
        .find(|row| row.node == peer.node)
        .map(|row| row.capabilities.iter().map(|(id, _)| id.clone()).collect())
        .unwrap_or_default();
    let label = display_name(peer.name.as_deref(), &peer.node);
    let (state_label, state_color) = trust_state(lang, &peer.state);
    let confirming = shell.right.federation.confirm_revoke.as_deref() == Some(peer.node.as_str());
    let confirming_remove =
        shell.right.federation.confirm_remove.as_deref() == Some(peer.node.as_str());

    let mut row = hairline_all(
        div()
            .id(SharedString::from(format!("federation-peer-{index}")))
            .debug_selector(move || format!("federation-peer-{index}"))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .p(px(10.0))
            .rounded(px(ROW_RADIUS))
            .bg(rgb(if selected {
                theme::soft(theme::accent())
            } else {
                theme::page()
            }))
            .cursor_pointer(),
    )
    .border_color(rgb(if selected {
        theme::accent()
    } else {
        theme::border_soft()
    }))
    .on_click(cx.listener(move |this, _event, _window, cx| {
        this.right.federation.selected = Some(index);
        this.right.federation.grant_choice = None;
        cx.notify();
    }));

    let mut head = div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(peer_avatar(index, online))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(
                    txt(label.clone(), 13.0, theme::text())
                        .font_weight(FontWeight::MEDIUM)
                        .text_ellipsis(),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(dot(if online {
                            theme::success()
                        } else {
                            theme::faint()
                        }))
                        .child(txt(
                            if online {
                                lang.tr("right.federation.online")
                            } else {
                                lang.tr("right.federation.offlinePeer")
                            },
                            10.0,
                            theme::faint(),
                        ))
                        .child(txt(
                            format!(" · {}", short_node(&peer.node)),
                            10.0,
                            theme::faint(),
                        )),
                ),
        )
        .child(pill(state_label, state_color, theme::soft(state_color)));
    if !confirming && !confirming_remove {
        if peer.state != "revoked" {
            let node = peer.node.clone();
            let target = display_name(peer.name.as_deref(), &peer.node);
            head = head.child(icon_button(
                "federation-revoke",
                IconName::ShieldX,
                lang.tr("right.federation.revoke"),
                theme::faint(),
                cx,
                move |this, cx| {
                    if let Some(op) = this.right.federation.confirm_remove_op.take() {
                        this.fed_op_set(op, FederationActivityState::Cancelled, None);
                    }
                    this.right.federation.confirm_remove = None;
                    this.right.federation.confirm_revoke = Some(node.clone());
                    let op = this.fed_op_begin("right.federation.opRevoke", target.clone());
                    this.right.federation.confirm_op = Some(op);
                    cx.notify();
                },
            ));
        }
        // Removal works for any state: a trusted peer is revoked implicitly by
        // forgetting the record, and a revoked one disappears from the list.
        let node = peer.node.clone();
        let target = display_name(peer.name.as_deref(), &peer.node);
        head = head.child(icon_button(
            SharedString::from(format!("federation-remove-{index}")),
            IconName::Trash,
            lang.tr("right.federation.removePeer"),
            theme::faint(),
            cx,
            move |this, cx| {
                if let Some(op) = this.right.federation.confirm_op.take() {
                    this.fed_op_set(op, FederationActivityState::Cancelled, None);
                }
                this.right.federation.confirm_revoke = None;
                this.right.federation.confirm_remove = Some(node.clone());
                let op = this.fed_op_begin("right.federation.opRemove", target.clone());
                this.right.federation.confirm_remove_op = Some(op);
                cx.notify();
            },
        ));
    }
    row = row.child(head);

    if !peer.grants.is_empty() {
        row = row.child(capability_pills(lang, &peer.grants, 4));
    }
    if selected {
        let mut detail = div().flex().flex_col().gap(px(6.0)).pt(px(2.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(txt(lang.tr("right.federation.peerId"), 10.0, theme::faint()).flex_none())
                .child(
                    txt(peer.node.clone(), 10.0, theme::muted())
                        .font_family("monospace")
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis(),
                )
                .child(copy_button(
                    "federation-copy-peer",
                    peer.node.clone(),
                    lang.tr("right.federation.copyId"),
                    cx,
                )),
        );
        if !peer.grants.is_empty() {
            let mut chips = div().flex().flex_row().flex_wrap().gap(px(4.0));
            for (grant_index, grant) in peer.grants.iter().take(8).enumerate() {
                let node = peer.node.clone();
                let capability = grant.clone();
                let label = capability_label(grant);
                let op_target = label.clone();
                chips = chips.child(
                    div()
                        .id(SharedString::from(format!(
                            "federation-revoke-grant-{index}-{grant_index}"
                        )))
                        .debug_selector(move || {
                            format!("federation-revoke-grant-{index}-{grant_index}")
                        })
                        .h(px(22.0))
                        .px(px(7.0))
                        .rounded(px(7.0))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .bg(rgb(theme::soft(theme::accent())))
                        .hover(|style| style.opacity(0.8))
                        .child(txt(label, 10.0, theme::accent()))
                        .child(icon(IconName::X, 10.0, theme::accent()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            let op =
                                this.fed_op_begin("right.federation.opRevoke", op_target.clone());
                            let node = node.clone();
                            let capability = capability.clone();
                            this.apply_federation(
                                move |store| revoke_grant_action(store, &node, &capability),
                                cx,
                            );
                            finish_store_op(this, op);
                            cx.notify();
                        })),
                );
            }
            detail = detail.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(
                        lang.tr("right.federation.grantsLabel"),
                        10.0,
                        theme::faint(),
                    ))
                    .child(chips),
            );
        }
        if !advertised.is_empty() {
            detail = detail.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(
                        lang.tr("right.federation.capabilityMenu"),
                        10.0,
                        theme::faint(),
                    ))
                    .child(capability_pills(lang, &advertised, 8)),
            );
        }
        row = row.child(detail);
    }

    if confirming {
        let confirm_node = peer.node.clone();
        let confirm_label = label.clone();
        row = row.child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .p(px(8.0))
                .rounded(px(9.0))
                .bg(rgb(theme::soft(theme::error())))
                .child(txt(
                    crate::i18n::tr_args(
                        lang,
                        "right.federation.confirmRevoke",
                        &[("node", label.clone())],
                    ),
                    11.0,
                    theme::error(),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(solid_button(
                            "federation-revoke-confirm",
                            lang.tr("right.federation.revoke"),
                            true,
                            true,
                            cx,
                            move |this, cx| {
                                let node = confirm_node.clone();
                                let op = this.right.federation.confirm_op.take();
                                if let Some(op) = op {
                                    this.fed_op_set(op, FederationActivityState::Running, None);
                                }
                                this.right.federation.confirm_revoke = None;
                                let label_for_action = confirm_label.clone();
                                this.apply_federation(
                                    move |store| {
                                        revoke_peer_action(store, &node, &label_for_action)
                                    },
                                    cx,
                                );
                                if let Some(op) = op {
                                    if let Some(error) = this.right.federation.error.clone() {
                                        this.fed_op_set(
                                            op,
                                            FederationActivityState::Failed,
                                            Some(error),
                                        );
                                    } else {
                                        this.fed_op_set(
                                            op,
                                            FederationActivityState::Revoked,
                                            this.right.federation.notice.clone(),
                                        );
                                    }
                                }
                                cx.notify();
                            },
                        ))
                        .child(solid_button(
                            "federation-revoke-cancel",
                            lang.tr("right.federation.cancel"),
                            false,
                            true,
                            cx,
                            move |this, cx| {
                                if let Some(op) = this.right.federation.confirm_op.take() {
                                    this.fed_op_set(op, FederationActivityState::Cancelled, None);
                                }
                                this.right.federation.confirm_revoke = None;
                                cx.notify();
                            },
                        )),
                ),
        );
    }

    if confirming_remove {
        let remove_node = peer.node.clone();
        row = row.child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .p(px(8.0))
                .rounded(px(9.0))
                .bg(rgb(theme::soft(theme::error())))
                .child(txt(
                    crate::i18n::tr_args(
                        lang,
                        "right.federation.confirmRemovePeer",
                        &[("node", label.clone())],
                    ),
                    11.0,
                    theme::error(),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(solid_button(
                            "federation-remove-confirm",
                            lang.tr("right.federation.removePeer"),
                            true,
                            true,
                            cx,
                            move |this, cx| {
                                let node = remove_node.clone();
                                let op = this.right.federation.confirm_remove_op.take();
                                if let Some(op) = op {
                                    this.fed_op_set(op, FederationActivityState::Running, None);
                                }
                                this.right.federation.confirm_remove = None;
                                this.apply_federation(
                                    move |store| {
                                        let node_id = parse_peer(&node)?;
                                        if !store.remove(&node_id) {
                                            return Err("peer is not enrolled".to_string());
                                        }
                                        Ok(format!("removed {}", short_node(node.as_str())))
                                    },
                                    cx,
                                );
                                if let Some(op) = op {
                                    if let Some(error) = this.right.federation.error.clone() {
                                        this.fed_op_set(
                                            op,
                                            FederationActivityState::Failed,
                                            Some(error),
                                        );
                                    } else {
                                        this.fed_op_set(
                                            op,
                                            FederationActivityState::Removed,
                                            this.right.federation.notice.clone(),
                                        );
                                    }
                                }
                                cx.notify();
                            },
                        ))
                        .child(solid_button(
                            "federation-remove-cancel",
                            lang.tr("right.federation.cancel"),
                            false,
                            true,
                            cx,
                            move |this, cx| {
                                if let Some(op) = this.right.federation.confirm_remove_op.take() {
                                    this.fed_op_set(op, FederationActivityState::Cancelled, None);
                                }
                                this.right.federation.confirm_remove = None;
                                cx.notify();
                            },
                        )),
                ),
        );
    }

    row.into_any_element()
}

fn discovered_row(
    shell: &Shell,
    row: &super::right_panel::DiscoveredPeerRow,
    index: usize,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let lang = shell.state.lang;
    let node = row.node.clone();
    let label = display_name(row.name.as_deref(), &row.node);

    let mut card = hairline_all(
        div()
            .id(SharedString::from(format!("federation-discovered-{index}")))
            .debug_selector(move || format!("federation-discovered-{index}"))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .p(px(10.0))
            .rounded(px(ROW_RADIUS))
            .bg(rgb(theme::new_session_fill())),
    )
    .border_color(rgb(theme::border_soft()));

    let head = div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(peer_avatar(index + 1, true))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(
                    txt(label.clone(), 13.0, theme::text())
                        .font_weight(FontWeight::MEDIUM)
                        .text_ellipsis(),
                )
                .child(txt(
                    format!(
                        "{} · {}",
                        lang.tr("right.federation.discovered"),
                        short_node(&node)
                    ),
                    10.0,
                    theme::faint(),
                )),
        )
        .child(pill(
            lang.tr("right.federation.discoveredBadge"),
            theme::accent(),
            theme::soft(theme::accent()),
        ))
        .child(solid_button(
            SharedString::from(format!("federation-peer-trust-{index}")),
            lang.tr("right.federation.trust"),
            true,
            true,
            cx,
            {
                let node_for_trust = node.clone();
                let label = label.clone();
                move |this, cx| {
                    let grants = std::collections::BTreeSet::new();
                    let op = this.fed_op_begin("right.federation.opTrust", label.clone());
                    let node_for_store = node_for_trust.clone();
                    let label_for_store = this
                        .right
                        .federation
                        .discovered
                        .iter()
                        .find(|row| row.node == node_for_store)
                        .and_then(|row| row.name.clone());
                    this.apply_federation(
                        move |store| {
                            enroll_and_grant(
                                store,
                                &node_for_store,
                                label_for_store.as_deref(),
                                &grants,
                            )
                        },
                        cx,
                    );
                    finish_store_op(this, op);
                    cx.notify();
                }
            },
        ));
    card = card.child(head);

    if !row.capabilities.is_empty() {
        let ids: Vec<String> = row
            .capabilities
            .iter()
            .take(8)
            .map(|(capability, _)| capability.clone())
            .collect();
        card = card.child(
            div()
                .debug_selector(|| format!("federation-discovered-caps-{index}"))
                .child(capability_pills(lang, &ids, 8)),
        );
    }
    card.into_any_element()
}

fn peer_avatar(index: usize, online: bool) -> Div {
    let tints = [theme::accent(), VIOLET, theme::success()];
    let marks = [IconName::Laptop, IconName::Server, IconName::Computer];
    let tint = tints[index % tints.len()];
    let mark = marks[index % marks.len()];
    div()
        .size(px(38.0))
        .flex_none()
        .rounded(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(
            135.0,
            linear_color_stop(opaque(theme::soft(tint)), 0.0),
            linear_color_stop(opaque(theme::blend_into(tint, theme::page(), 0.96)), 1.0),
        ))
        .when(!online, |avatar| avatar.opacity(0.7))
        .child(icon(mark, 18.0, tint))
}

fn capability_pills(lang: crate::i18n::Lang, ids: &[String], limit: usize) -> Div {
    let mut labels: Vec<String> = Vec::new();
    for id in ids {
        let label = capability_label_localized(lang, id);
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    let extra = labels.len().saturating_sub(limit);
    let knowledge = lang.tr("right.federation.capKnowledge");
    let resource = lang.tr("right.federation.capResource");
    let work = lang.tr("right.federation.capWork");
    let mut row = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for label in labels.into_iter().take(limit) {
        let tint = if label == knowledge {
            theme::accent()
        } else if label == resource {
            theme::success()
        } else if label == work {
            VIOLET
        } else {
            theme::muted()
        };
        row = row.child(
            div()
                .h(px(20.0))
                .px(px(7.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .bg(rgb(theme::soft(tint)))
                .child(txt(label, 10.0, tint)),
        );
    }
    if extra > 0 {
        row = row.child(
            div()
                .h(px(20.0))
                .px(px(6.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .bg(rgb(theme::new_session_fill()))
                .child(txt(format!("+{extra}"), 10.0, theme::faint())),
        );
    }
    row
}

fn capability_label(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).replace('_', " ")
}

fn capability_label_localized(lang: crate::i18n::Lang, id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    if lower.contains("knowledge") {
        lang.tr("right.federation.capKnowledge")
    } else if lower.contains("resource") || lower.contains("lease") {
        lang.tr("right.federation.capResource")
    } else if lower.contains("delegate") || lower.contains("work") || lower.contains("task") {
        lang.tr("right.federation.capWork")
    } else {
        capability_label(id)
    }
}

/// The menu label for a capability this node can grant.
///
/// Read tools reuse the expose wording ("read_file — read one text file") so
/// the grant menu and the expose list speak the same language; the knowledge
/// service has its own two lines.
fn grantable_label(lang: crate::i18n::Lang, id: &str) -> String {
    if let Some(name) = id.strip_prefix("builtin/") {
        if crate::federation_node::exposable_capabilities().contains(&name) {
            return expose_label(lang, name);
        }
    }
    match id {
        "builtin/knowledge_query" => lang.tr("right.federation.grantKnowledgeQuery"),
        "builtin/knowledge_publish" => lang.tr("right.federation.grantKnowledgePublish"),
        other => capability_label(other),
    }
}

/// A grant-menu label that includes the extension's own description.
fn grantable_label_with(
    lang: crate::i18n::Lang,
    id: &str,
    extension_tools: &[crate::extension::host::NodeToolInfo],
) -> String {
    if let Some(info) = extension_tools
        .iter()
        .find(|info| id == format!("builtin/{}", info.qualified))
    {
        return format!(
            "{} — {} ({})",
            info.qualified, info.description, info.extension
        );
    }
    grantable_label(lang, id)
}

fn trust_state(lang: crate::i18n::Lang, state: &str) -> (String, u32) {
    match state {
        "trusted" => (lang.tr("right.federation.badgeTrusted"), theme::success()),
        "pending" | "invited" => (lang.tr("right.federation.badgePending"), theme::warn()),
        "revoked" | "suspended" => (lang.tr("right.federation.badgeRevoked"), theme::error()),
        other => (other.to_string(), theme::muted()),
    }
}

fn finish_store_op(shell: &mut Shell, op: u64) {
    let result = match (
        shell.right.federation.error.clone(),
        shell.right.federation.notice.clone(),
    ) {
        (Some(error), _) => Err(error),
        (None, Some(notice)) => Ok(notice),
        (None, None) => Ok(String::new()),
    };
    shell.fed_op_result(op, &result);
}

fn copy_button(
    id: &'static str,
    value: String,
    tip: String,
    cx: &mut Context<Shell>,
) -> AnyElement {
    icon_button(
        id,
        IconName::Copy,
        tip,
        theme::faint(),
        cx,
        move |this, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
            this.right.federation.notice = Some(this.state.lang.tr("right.federation.idCopied"));
            cx.notify();
        },
    )
}

// ---------------------------------------------------------------------------
// Node identity and the contextual grant.
// ---------------------------------------------------------------------------

fn identity_card(
    shell: &Shell,
    runtime: Option<&std::sync::Arc<crate::federation_node::FederationRuntime>>,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let lang = shell.state.lang;
    let profile =
        crate::federation::resolve_profile(Some(&shell.state.settings.global.federation.profile));
    let name = runtime
        .and_then(|runtime| runtime.node_name().filter(|name| !name.trim().is_empty()))
        .or_else(|| {
            shell
                .state
                .settings
                .global
                .federation
                .node_name
                .clone()
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| crate::federation::display_name(None, &profile));
    let node_id = runtime.map(|runtime| runtime.node_id().to_string());
    let grants: usize = shell
        .right
        .federation
        .peers
        .iter()
        .map(|peer| peer.grants.len())
        .sum();

    let mut row = div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .p(px(10.0))
        .rounded(px(ROW_RADIUS))
        .bg(rgb(theme::new_session_fill()))
        .child(icon_tile(IconName::Laptop, VIOLET))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(
                    txt(name, 13.0, theme::text())
                        .font_weight(FontWeight::MEDIUM)
                        .text_ellipsis(),
                )
                .child(txt(
                    crate::i18n::tr_args(
                        lang,
                        "right.federation.identityMeta",
                        &[
                            ("peers", shell.right.federation.peers.len().to_string()),
                            ("grants", grants.to_string()),
                        ],
                    ),
                    10.0,
                    theme::faint(),
                )),
        )
        .child(if runtime.is_some() {
            pill(
                lang.tr("right.federation.currentNode"),
                theme::success(),
                theme::soft(theme::success()),
            )
        } else {
            pill(
                lang.tr("right.federation.offline"),
                theme::faint(),
                theme::hover(),
            )
        });
    row = row.child(solid_button(
        "federation-change-name",
        lang.tr("right.federation.change"),
        false,
        true,
        cx,
        |this, cx| {
            this.right.federation.advanced = true;
            this.right.federation.notice =
                Some(this.state.lang.tr("right.federation.changeNameHint"));
            cx.notify();
        },
    ));

    let mut card = card();
    card = card.child(card_header(
        IconName::FingerprintPattern,
        VIOLET,
        lang.tr("right.federation.identityTitle"),
        lang.tr("right.federation.identitySubtitle"),
        None,
    ));
    card = card.child(row);
    if let Some(node_id) = node_id {
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    txt(node_id.clone(), 10.0, theme::faint())
                        .font_family("monospace")
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis(),
                )
                .child(copy_button(
                    "federation-copy-node",
                    node_id,
                    lang.tr("right.federation.copyNode"),
                    cx,
                )),
        );
    }
    card.into_any_element()
}

fn grant_card(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let mut card = card();
    card = card.child(card_header(
        IconName::ShieldCheck,
        theme::success(),
        lang.tr("right.federation.grantCardTitle"),
        lang.tr("right.federation.grantSubtitle"),
        None,
    ));

    if shell.right.federation.peers.is_empty() {
        card = card.child(empty_state(
            lang.tr("right.federation.noPeers"),
            IconName::ShieldCheck,
        ));
    }

    let selected = shell.right.federation.selected;
    let peer = selected.and_then(|index| shell.right.federation.peers.get(index));
    let peer_id = peer.map(|peer| peer.node.clone()).unwrap_or_default();
    // A grant is a permission the peer holds to call a capability ON THIS
    // NODE, so the menu is this node's grantable surface. The peer's own
    // advertised catalogue is what it offers us; it can be empty when the peer
    // is offline, and it never belonged in this menu.
    let mut capabilities: Vec<String> = crate::federation_node::grantable_capabilities();
    for info in &shell.right.federation.extension_tools {
        capabilities.push(format!("builtin/{}", info.qualified));
    }
    let choice = shell.right.federation.grant_choice.clone();
    let granted: std::collections::BTreeSet<String> = peer
        .map(|peer| peer.grants.iter().cloned().collect())
        .unwrap_or_default();

    let peer_button = peer_picker(shell, "federation-peer-select", cx);

    let shell_entity = cx.entity();
    let capability_rows = capabilities.clone();
    let chosen = choice.clone();
    let granted_for_menu = granted.clone();
    let extension_tools_for_menu = shell.right.federation.extension_tools.clone();
    let capability_button = Button::new("federation-capability-select")
        .ghost()
        .w_full()
        .h(px(32.0))
        .px(px(10.0))
        .rounded(px(9.0))
        .bg(rgb(theme::new_session_fill()))
        .flex()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        .child(
            txt(
                choice.as_deref().map(capability_label).unwrap_or_else(|| {
                    if capability_rows.is_empty() {
                        lang.tr("right.federation.noKnownCaps")
                    } else {
                        lang.tr("right.federation.capabilityMenu")
                    }
                }),
                12.0,
                if chosen.is_some() {
                    theme::text()
                } else {
                    theme::faint()
                },
            )
            .flex_1()
            .min_w_0()
            .text_ellipsis(),
        )
        .child(icon(IconName::ChevronDown, 12.0, theme::faint()))
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
            let mut menu = menu;
            if capability_rows.is_empty() {
                return menu.item(PopupMenuItem::label(crate::i18n::tr(
                    lang,
                    "right.federation.noKnownCaps",
                )));
            }
            for capability in capability_rows.iter() {
                let shell = shell_entity.clone();
                let capability = capability.clone();
                let checked = chosen.as_deref() == Some(capability.as_str())
                    || granted_for_menu.contains(&capability);
                let label = grantable_label_with(lang, &capability, &extension_tools_for_menu);
                menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                    move |_, _, cx| {
                        shell.update(cx, |this, cx| {
                            this.right.federation.grant_choice = Some(capability.clone());
                            cx.notify();
                        });
                    },
                ));
            }
            menu
        });

    let grant_enabled = selected.is_some() && choice.is_some();
    let grant_peer = peer_id.clone();
    let grant_choice = choice.clone();
    card = card.child(
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(label_line(
                lang.tr("right.federation.peerLabel"),
                peer_button.into_any_element(),
            ))
            .child(label_line(
                lang.tr("right.federation.capabilityLabel"),
                capability_button.into_any_element(),
            ))
            .child(txt(
                lang.tr("right.federation.grantNodeHint"),
                9.5,
                theme::faint(),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(solid_button(
                        "federation-grant",
                        lang.tr("right.federation.grant"),
                        true,
                        grant_enabled,
                        cx,
                        move |this, cx| {
                            let Some(capability) = grant_choice.clone() else {
                                return;
                            };
                            if grant_peer.is_empty() {
                                return;
                            }
                            let peer = grant_peer.clone();
                            let target = this
                                .right
                                .federation
                                .peers
                                .iter()
                                .find(|row| row.node == peer)
                                .map(|row| display_name(row.name.as_deref(), &row.node))
                                .unwrap_or_else(|| short_node(&peer));
                            let op = this.fed_op_begin("right.federation.opGrant", target);
                            let peer_for_store = peer.clone();
                            this.apply_federation(
                                move |store| {
                                    grant_capability(store, &peer_for_store, &capability, None)
                                },
                                cx,
                            );
                            finish_store_op(this, op);
                            cx.notify();
                        },
                    ))
                    .child(solid_button(
                        "federation-view-peer",
                        lang.tr("right.federation.view"),
                        false,
                        !peer_id.is_empty(),
                        cx,
                        move |this, cx| {
                            if !peer_id.is_empty() {
                                cx.write_to_clipboard(ClipboardItem::new_string(peer_id.clone()));
                                this.right.federation.notice =
                                    Some(this.state.lang.tr("right.federation.idCopied"));
                                cx.notify();
                            }
                        },
                    )),
            ),
    );
    card.into_any_element()
}

fn peer_picker(shell: &Shell, id: &'static str, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let selected = shell.right.federation.selected;
    let label = selected
        .and_then(|index| shell.right.federation.peers.get(index))
        .map(|peer| display_name(peer.name.as_deref(), &peer.node))
        .unwrap_or_else(|| lang.tr("right.federation.selectPeer"));
    let rows: Vec<(usize, String)> = shell
        .right
        .federation
        .peers
        .iter()
        .enumerate()
        .map(|(index, peer)| (index, display_name(peer.name.as_deref(), &peer.node)))
        .collect();
    let shell_entity = cx.entity();
    Button::new(id)
        .ghost()
        .w_full()
        .h(px(32.0))
        .px(px(10.0))
        .rounded(px(9.0))
        .bg(rgb(theme::new_session_fill()))
        .flex()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        .child(
            txt(label, 12.0, theme::text())
                .flex_1()
                .min_w_0()
                .text_ellipsis(),
        )
        .child(icon(IconName::ChevronDown, 12.0, theme::faint()))
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
            if rows.is_empty() {
                return menu.item(PopupMenuItem::label(crate::i18n::tr(
                    lang,
                    "right.federation.noPeers",
                )));
            }
            let mut menu = menu;
            for (index, label) in rows.iter().cloned() {
                let shell = shell_entity.clone();
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .checked(selected == Some(index))
                        .on_click(move |_, _, cx| {
                            shell.update(cx, |this, cx| {
                                this.right.federation.selected = Some(index);
                                this.right.federation.grant_choice = None;
                                cx.notify();
                            });
                        }),
                );
            }
            menu
        })
        .into_any_element()
}

fn label_line(label: String, control: AnyElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(txt(label, 10.0, theme::faint()))
        .child(control)
}

// ---------------------------------------------------------------------------
// Exposed capabilities.
// ---------------------------------------------------------------------------

fn exposed_card(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let exposed = shell.state.settings.global.federation.expose.clone();
    let mut card = card();
    card = card.child(card_header(
        IconName::Blocks,
        theme::warn(),
        lang.tr("right.federation.exposeTitle"),
        lang.tr("right.federation.exposeCardSubtitle"),
        None,
    ));

    let mut summary = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .p(px(9.0))
        .rounded(px(ROW_RADIUS))
        .bg(rgb(theme::new_session_fill()))
        .child(icon_tile(IconName::Code, theme::accent()))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(txt(
                    format!(
                        "{} ({})",
                        lang.tr("right.federation.exposeMenu"),
                        exposed.len()
                    ),
                    12.0,
                    theme::text(),
                ))
                .child(
                    txt(
                        if exposed.is_empty() {
                            lang.tr("right.federation.exposeNone")
                        } else {
                            exposed
                                .iter()
                                .map(|id| {
                                    shell
                                        .right
                                        .federation
                                        .extension_tools
                                        .iter()
                                        .find(|info| id == &format!("builtin/{}", info.qualified))
                                        .map(|info| {
                                            format!("{} ({})", info.qualified, info.extension)
                                        })
                                        .unwrap_or_else(|| capability_label(id))
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        },
                        10.0,
                        theme::faint(),
                    )
                    .text_ellipsis(),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(dot(if exposed.is_empty() {
                    theme::faint()
                } else {
                    theme::success()
                }))
                .child(txt(
                    if exposed.is_empty() {
                        lang.tr("right.federation.badgeInactive")
                    } else {
                        lang.tr("right.federation.badgeActive")
                    },
                    10.0,
                    if exposed.is_empty() {
                        theme::faint()
                    } else {
                        theme::success()
                    },
                )),
        );
    summary = summary.child(solid_button(
        "federation-expose-manage",
        lang.tr("right.federation.manage"),
        false,
        true,
        cx,
        |this, cx| {
            this.right.federation.advanced = true;
            cx.notify();
        },
    ));
    card = card.child(summary);
    if let Some(root) = crate::federation_node::expose_root(&shell.state.settings) {
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(icon(IconName::FolderOpen, 12.0, theme::faint()))
                .child(
                    txt(
                        crate::i18n::tr_args(
                            lang,
                            "right.federation.exposeRootLine",
                            &[("root", root.display().to_string())],
                        ),
                        10.0,
                        theme::faint(),
                    )
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis(),
                ),
        );
    }
    card.into_any_element()
}

// ---------------------------------------------------------------------------
// Work.
// ---------------------------------------------------------------------------

fn work_tab(
    shell: &Shell,
    runtime: Option<&std::sync::Arc<crate::federation_node::FederationRuntime>>,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let lang = shell.state.lang;
    let delegations = &shell.right.federation.delegations;
    let active = delegations
        .iter()
        .filter(|row| {
            matches!(
                row.state,
                "right.federation.stateRunning" | "right.federation.stateAccepted"
            )
        })
        .count();

    let mut card = card();
    let mut trailing = div().flex().items_center().gap(px(6.0));
    if active > 0 {
        trailing = trailing.child(pill(
            crate::i18n::tr_args(
                lang,
                "right.federation.workActive",
                &[("count", active.to_string())],
            ),
            theme::accent(),
            theme::soft(theme::accent()),
        ));
    }
    trailing = trailing.child(solid_button(
        "federation-audit-receipts",
        lang.tr("right.federation.auditReceipts"),
        false,
        true,
        cx,
        |this, cx| {
            let Some(runtime) = crate::federation_node::running() else {
                this.right.federation.error =
                    Some(this.state.lang.tr("right.federation.notRunning"));
                cx.notify();
                return;
            };
            let Some(peers_path) = this.state.settings.federation_peers_path() else {
                this.right.federation.error = Some(this.state.lang.tr("right.federation.noTasks"));
                cx.notify();
                return;
            };
            let tasks_path = peers_path.with_file_name("tasks.json");
            let Ok(graph) = federation::TaskGraph::load(&tasks_path) else {
                this.right.federation.error = Some(this.state.lang.tr("right.federation.noTasks"));
                cx.notify();
                return;
            };
            let tasks: Vec<federation::TaskNode> = graph.tasks.values().cloned().collect();
            let auditor = federation::ReceiptAuditor::new(
                std::sync::Arc::clone(runtime.node()),
                agent::capability::InvocationContext::local(
                    agent::capability::InvocationId::new(format!(
                        "audit-{}",
                        agent::capability::unix_ms()
                    )),
                    agent::capability::Principal::Local {
                        session: "federation-pane".to_string(),
                        preset: String::new(),
                    },
                    agent::capability::WorkspaceId::new("ws:federation").expect("a workspace"),
                ),
            );
            let target = this.state.lang.tr("right.federation.theNode");
            let op = this.fed_op_begin("right.federation.opAudit", target);
            this.fed_op_set(op, FederationActivityState::Running, None);
            let handle = crate::extension::spawn_async(async move { auditor.audit(&tasks).await });
            cx.spawn(async move |this, cx| {
                let mismatches = handle.await;
                let _ = this.update(cx, |this, cx| {
                    match mismatches {
                        Ok(mismatches) if mismatches.is_empty() => {
                            this.right.federation.notice =
                                Some(this.state.lang.tr("right.federation.receiptsMatch"));
                            this.fed_op_set(op, FederationActivityState::Succeeded, None);
                        }
                        Ok(mismatches) => {
                            this.right.federation.error = Some(crate::i18n::tr_args(
                                this.state.lang,
                                "right.federation.receiptsMismatch",
                                &[("count", mismatches.len().to_string())],
                            ));
                            this.fed_op_set(
                                op,
                                FederationActivityState::Failed,
                                this.right.federation.error.clone(),
                            );
                        }
                        Err(error) => {
                            this.right.federation.error = Some(error.to_string());
                            this.fed_op_set(
                                op,
                                FederationActivityState::Failed,
                                this.right.federation.error.clone(),
                            );
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
        },
    ));
    trailing = trailing.child(icon_button(
        "federation-clear-all",
        IconName::Trash,
        lang.tr("right.federation.clearAll"),
        theme::faint(),
        cx,
        |this, cx| {
            let Some(runtime) = crate::federation_node::running() else {
                this.right.federation.error =
                    Some(this.state.lang.tr("right.federation.notRunning"));
                cx.notify();
                return;
            };
            let op = this.fed_op_begin(
                "right.federation.opClearWork",
                this.state.lang.tr("right.federation.theNode"),
            );
            let count = runtime.node().clear_delegations(true).unwrap_or_default();
            this.right.federation.notice = Some(crate::i18n::tr_args(
                this.state.lang,
                "right.federation.clearedAll",
                &[("count", count.to_string())],
            ));
            this.right.federation.error = None;
            this.fed_op_set(op, FederationActivityState::Succeeded, None);
            this.reload_federation_quiet();
            cx.notify();
        },
    ));
    card = card.child(card_header(
        IconName::Workflow,
        VIOLET,
        lang.tr("right.federation.workTitle"),
        lang.tr("right.federation.workSubtitle"),
        Some(trailing.into_any_element()),
    ));

    if delegations.is_empty() {
        card = card.child(empty_state(
            lang.tr("right.federation.noWork"),
            IconName::Workflow,
        ));
    }
    for (index, delegation) in delegations.iter().take(12).enumerate() {
        let failure = delegation.error.clone();
        let mut row = hairline_all(
            div()
                .id(SharedString::from(format!("federation-work-{index}")))
                .debug_selector(move || format!("federation-work-{index}"))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .p(px(10.0))
                .rounded(px(ROW_RADIUS))
                .bg(rgb(theme::new_session_fill())),
        )
        .border_color(rgb(theme::border_soft()));
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(pill(
                    lang.tr(delegation.state),
                    work_state_color(delegation.state),
                    theme::soft(work_state_color(delegation.state)),
                ))
                .child(
                    txt(
                        crate::i18n::tr_args(
                            lang,
                            "right.federation.workTarget",
                            &[("peer", peer_display(shell, &delegation.target))],
                        ),
                        10.0,
                        theme::faint(),
                    )
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis(),
                )
                .child(txt(short_node(&delegation.id), 10.0, theme::faint())),
        );
        row = row.child(txt(delegation.objective.clone(), 12.0, theme::text()));
        if let Some(failure) = failure {
            row = row.child(txt(failure, 10.0, theme::error()));
        }
        let id = delegation.id.clone();
        let target = delegation.target.clone();
        if delegation.cancellable {
            let runtime = runtime.cloned();
            row = row.child(solid_button(
                SharedString::from(format!("federation-work-cancel-{index}")),
                lang.tr("right.federation.cancelDelegation"),
                false,
                true,
                cx,
                move |this, cx| {
                    let Some(runtime) = runtime.as_ref() else {
                        return;
                    };
                    let Ok(target_node) = agent::capability::NodeId::new(target.clone()) else {
                        return;
                    };
                    let delegation_id = federation::DelegationId::new(id.clone());
                    let node = std::sync::Arc::clone(runtime.node());
                    let context = agent::capability::InvocationContext::local(
                        agent::capability::InvocationId::new(format!("cancel-{}", delegation_id)),
                        agent::capability::Principal::Local {
                            session: "federation-pane".to_string(),
                            preset: String::new(),
                        },
                        agent::capability::WorkspaceId::new("ws:federation").expect("a workspace"),
                    );
                    let op = this.fed_op_begin("right.federation.opCancelWork", short_node(&id));
                    this.fed_op_set(op, FederationActivityState::Running, None);
                    let handle = crate::extension::spawn_async(async move {
                        node.cancel_delegation(&target_node, &delegation_id, &context)
                            .await
                    });
                    cx.spawn(async move |this, cx| {
                        let outcome = handle.await;
                        let _ = this.update(cx, |this, cx| {
                            match outcome {
                                Ok(Ok(_)) => {
                                    this.right.federation.notice = Some(
                                        this.state.lang.tr("right.federation.delegationCancelled"),
                                    );
                                    this.fed_op_set(op, FederationActivityState::Cancelled, None);
                                }
                                Ok(Err(error)) => {
                                    this.right.federation.error = Some(error.to_string());
                                    this.fed_op_set(
                                        op,
                                        FederationActivityState::Failed,
                                        this.right.federation.error.clone(),
                                    );
                                }
                                Err(error) => {
                                    this.right.federation.error = Some(error.to_string());
                                    this.fed_op_set(
                                        op,
                                        FederationActivityState::Failed,
                                        this.right.federation.error.clone(),
                                    );
                                }
                            }
                            this.reload_federation_quiet();
                            cx.notify();
                        });
                    })
                    .detach();
                    cx.notify();
                },
            ));
        } else {
            let runtime = runtime.cloned();
            row = row.child(solid_button(
                SharedString::from(format!("federation-work-clear-{index}")),
                lang.tr("right.federation.clearWork"),
                false,
                true,
                cx,
                move |this, cx| {
                    if let Some(runtime) = runtime.as_ref() {
                        runtime
                            .node()
                            .clear_delegation(&federation::DelegationId::new(id.clone()));
                    }
                    this.reload_federation_quiet();
                    cx.notify();
                },
            ));
        }
        card = card.child(row);
    }
    card.into_any_element()
}

fn work_state_color(state: &str) -> u32 {
    match state {
        "right.federation.stateRunning" | "right.federation.stateAccepted" => theme::accent(),
        "right.federation.stateSucceeded" => theme::success(),
        "right.federation.stateFailed" | "right.federation.stateRejected" => theme::error(),
        _ => theme::faint(),
    }
}

fn peer_display(shell: &Shell, node: &str) -> String {
    shell
        .right
        .federation
        .peers
        .iter()
        .find(|peer| peer.node == node)
        .map(|peer| display_name(peer.name.as_deref(), &peer.node))
        .unwrap_or_else(|| short_node(node))
}

// ---------------------------------------------------------------------------
// Resources.
// ---------------------------------------------------------------------------

fn resources_tab(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let now = agent::capability::unix_ms();
    let mut column = div().flex().flex_col().gap(px(CARD_GAP));

    let mut leases = card();
    leases = leases.child(card_header(
        IconName::KeyRound,
        theme::success(),
        lang.tr("right.federation.leaseTitle"),
        lang.tr("right.federation.leaseSubtitle"),
        None,
    ));
    if shell.right.federation.leases.is_empty() {
        leases = leases.child(empty_state(
            lang.tr("right.federation.noResources"),
            IconName::KeyRound,
        ));
    }
    for (index, lease) in shell.right.federation.leases.iter().take(8).enumerate() {
        let expired = lease.expires_at_ms.is_some_and(|at| at <= now);
        let remaining = lease
            .expires_at_ms
            .map(|at| at.saturating_sub(now) / 60_000);
        let mut row = hairline_all(
            div()
                .id(SharedString::from(format!("federation-lease-{index}")))
                .debug_selector(move || format!("federation-lease-{index}"))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .p(px(10.0))
                .rounded(px(ROW_RADIUS))
                .bg(rgb(theme::new_session_fill())),
        )
        .border_color(rgb(theme::border_soft()));
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(icon_tile(IconName::Database, theme::success()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .flex_1()
                        .min_w_0()
                        .child(
                            txt(capability_label(&lease.capability), 12.0, theme::text())
                                .font_weight(FontWeight::MEDIUM)
                                .text_ellipsis(),
                        )
                        .child(txt(
                            format!(
                                "{} · {}",
                                peer_display(shell, &lease.holder),
                                format_bytes(lease.used, lease.max)
                            ),
                            10.0,
                            theme::faint(),
                        )),
                )
                .child(pill(
                    if expired {
                        lang.tr("right.federation.badgeExpired")
                    } else {
                        lang.tr("right.federation.badgeActive")
                    },
                    if expired {
                        theme::error()
                    } else {
                        theme::success()
                    },
                    theme::soft(if expired {
                        theme::error()
                    } else {
                        theme::success()
                    }),
                )),
        );
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(txt(short_node(&lease.id), 10.0, theme::faint()).flex_1())
                .child(txt(
                    match remaining {
                        None => lang.tr("right.federation.noExpiry"),
                        Some(0) => lang.tr("right.federation.expired"),
                        Some(minutes) => crate::i18n::tr_args(
                            lang,
                            "right.federation.expiresIn",
                            &[("minutes", minutes.to_string())],
                        ),
                    },
                    10.0,
                    theme::faint(),
                )),
        );
        leases = leases.child(row);
    }
    column = column.child(leases);

    let mut artifacts = card();
    artifacts = artifacts.child(card_header(
        IconName::Boxes,
        theme::accent(),
        lang.tr("right.federation.artifactsTitle"),
        lang.tr("right.federation.artifactsSubtitle"),
        None,
    ));
    if shell.right.federation.artifacts.is_empty() {
        artifacts = artifacts.child(empty_state(
            lang.tr("right.federation.noArtifacts"),
            IconName::FileText,
        ));
    }
    for (index, artifact) in shell.right.federation.artifacts.iter().take(8).enumerate() {
        let path = artifact.path.clone();
        let row = hairline_all(
            div()
                .id(SharedString::from(format!("federation-artifact-{index}")))
                .debug_selector(move || format!("federation-artifact-{index}"))
                .flex()
                .items_center()
                .gap(px(8.0))
                .p(px(9.0))
                .rounded(px(ROW_RADIUS))
                .bg(rgb(theme::new_session_fill())),
        )
        .border_color(rgb(theme::border_soft()))
        .child(icon_tile(IconName::FileText, theme::accent()))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(
                    txt(artifact.name.clone(), 12.0, theme::text())
                        .font_weight(FontWeight::MEDIUM)
                        .text_ellipsis(),
                )
                .child(txt(format!("{} B", artifact.bytes), 10.0, theme::faint())),
        )
        .child(solid_button(
            SharedString::from(format!("federation-artifact-open-{index}")),
            lang.tr("right.federation.open"),
            false,
            true,
            cx,
            {
                let path = path.clone();
                let name = artifact.name.clone();
                move |this, cx| {
                    let op = this.fed_op_begin("right.federation.opOpen", name.clone());
                    match std::fs::read(&path) {
                        Ok(bytes) => {
                            let body = String::from_utf8_lossy(&bytes[..bytes.len().min(8 * 1024)])
                                .to_string();
                            this.right.federation.preview = Some(FederationPreview {
                                title: name.clone(),
                                body,
                            });
                            this.right.federation.error = None;
                            this.fed_op_set(op, FederationActivityState::Succeeded, None);
                        }
                        Err(error) => {
                            this.right.federation.error =
                                Some(format!("{}: {error}", path.display()));
                            this.fed_op_set(
                                op,
                                FederationActivityState::Failed,
                                this.right.federation.error.clone(),
                            );
                        }
                    }
                    cx.notify();
                }
            },
        ))
        .child(icon_button(
            SharedString::from(format!("federation-artifact-remove-{index}")),
            IconName::Trash,
            lang.tr("right.federation.remove"),
            theme::faint(),
            cx,
            {
                let path = path.clone();
                move |this, cx| {
                    if let Err(error) = std::fs::remove_file(&path) {
                        this.right.federation.error = Some(format!("{}: {error}", path.display()));
                    } else {
                        this.right.federation.notice =
                            Some(this.state.lang.tr("right.federation.artifactRemoved"));
                    }
                    this.reload_federation_quiet();
                    cx.notify();
                }
            },
        ));
        artifacts = artifacts.child(row);
    }
    column = column.child(artifacts);

    if let Some(preview) = &shell.right.federation.preview {
        let mut viewer = card();
        viewer = viewer.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(icon_tile(IconName::FileText, theme::accent()))
                .child(
                    txt(preview.title.clone(), 12.0, theme::text())
                        .font_weight(FontWeight::MEDIUM)
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis(),
                )
                .child(icon_button(
                    "federation-preview-close",
                    IconName::X,
                    lang.tr("right.federation.close"),
                    theme::faint(),
                    cx,
                    |this, cx| {
                        this.right.federation.preview = None;
                        cx.notify();
                    },
                )),
        );
        viewer = viewer.child(
            div()
                .id("federation-preview-body")
                .max_h(px(220.0))
                .overflow_y_scroll()
                .rounded(px(9.0))
                .bg(rgb(theme::new_session_fill()))
                .p(px(10.0))
                .child(
                    txt(preview.body.clone(), 10.0, theme::muted())
                        .font_family("monospace")
                        .line_height(px(15.0)),
                ),
        );
        column = column.child(viewer);
    }
    column.into_any_element()
}

fn format_bytes(used: u64, max: Option<u64>) -> String {
    match max {
        Some(max) => format!("{used} / {max} B"),
        None => format!("{used} B"),
    }
}

// ---------------------------------------------------------------------------
// Knowledge.
// ---------------------------------------------------------------------------

fn knowledge_tab(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let selected = shell.right.federation.selected;
    let peer = selected
        .and_then(|index| shell.right.federation.peers.get(index))
        .or_else(|| shell.right.federation.peers.first());
    let peer_label = peer
        .map(|peer| display_name(peer.name.as_deref(), &peer.node))
        .unwrap_or_else(|| lang.tr("right.federation.noPeer"));

    let mut column = div().flex().flex_col().gap(px(CARD_GAP));

    // Ask: one question to the selected peer.
    let mut ask = card();
    ask = ask.child(card_header(
        IconName::BookOpen,
        theme::accent(),
        lang.tr("right.federation.knowledgeTitle"),
        crate::i18n::tr_args(
            lang,
            "right.federation.askTitle",
            &[("peer", peer_label.clone())],
        ),
        None,
    ));
    if shell.right.federation.peers.is_empty() {
        ask = ask.child(empty_state(
            lang.tr("right.federation.noPeers"),
            IconName::BookOpen,
        ));
    } else {
        let fallback_peer = shell.right.federation.peers.first().cloned();
        ask = ask.child(peer_picker(shell, "federation-knowledge-peer", cx));
        ask = ask.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(field(
                    &shell.federation_knowledge,
                    "federation-field-knowledge",
                ))
                .child(solid_button(
                    "federation-knowledge-ask",
                    lang.tr("right.federation.ask"),
                    true,
                    true,
                    cx,
                    move |this, cx| {
                        let index = this.right.federation.selected.or(
                            if this.right.federation.peers.is_empty() {
                                None
                            } else {
                                Some(0)
                            },
                        );
                        let Some(peer) = index
                            .and_then(|index| this.right.federation.peers.get(index).cloned())
                            .or_else(|| fallback_peer.clone())
                        else {
                            this.right.federation.knowledge_error =
                                Some(this.state.lang.tr("right.federation.noPeer"));
                            cx.notify();
                            return;
                        };
                        let Ok(node_id) = agent::capability::NodeId::new(peer.node.clone()) else {
                            return;
                        };
                        let terms: Vec<String> = this
                            .federation_knowledge
                            .read(cx)
                            .value()
                            .split_whitespace()
                            .map(str::to_string)
                            .collect();
                        let mut query = federation::KnowledgeQuery::new(
                            format!("pane-{}", agent::capability::unix_ms()),
                            federation::KnowledgePurpose::Answer,
                        );
                        query.terms = terms;
                        query.max_items = 8;
                        let Some(runtime) = crate::federation_node::running() else {
                            this.right.federation.knowledge_error =
                                Some(this.state.lang.tr("right.federation.notRunning"));
                            cx.notify();
                            return;
                        };
                        let node = std::sync::Arc::clone(runtime.node());
                        let context = agent::capability::InvocationContext::local(
                            agent::capability::InvocationId::new(format!(
                                "knowledge-{}",
                                agent::capability::unix_ms()
                            )),
                            agent::capability::Principal::Local {
                                session: "federation-pane".to_string(),
                                preset: String::new(),
                            },
                            agent::capability::WorkspaceId::new("ws:federation")
                                .expect("a workspace"),
                        );
                        let op = this.fed_op_begin(
                            "right.federation.opAsk",
                            display_name(peer.name.as_deref(), &peer.node),
                        );
                        this.fed_op_set(op, FederationActivityState::Running, None);
                        let handle = crate::extension::spawn_async(async move {
                            node.query_knowledge(&node_id, &query, &context).await
                        });
                        cx.spawn(async move |this, cx| {
                            let outcome = handle.await;
                            let _ = this.update(cx, |this, cx| {
                                match outcome {
                                    Ok(Ok(response)) => {
                                        this.right.federation.knowledge_output =
                                            serde_json::to_string_pretty(&response).ok();
                                        this.right.federation.knowledge_error = None;
                                        this.fed_op_set(
                                            op,
                                            FederationActivityState::Succeeded,
                                            None,
                                        );
                                    }
                                    Ok(Err(error)) => {
                                        this.right.federation.knowledge_error =
                                            Some(error.to_string());
                                        this.fed_op_set(
                                            op,
                                            FederationActivityState::Failed,
                                            this.right.federation.knowledge_error.clone(),
                                        );
                                    }
                                    Err(error) => {
                                        this.right.federation.knowledge_error =
                                            Some(error.to_string());
                                        this.fed_op_set(
                                            op,
                                            FederationActivityState::Failed,
                                            this.right.federation.knowledge_error.clone(),
                                        );
                                    }
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                        cx.notify();
                    },
                )),
        );
        if let Some(output) = &shell.right.federation.knowledge_output {
            ask = ask.child(
                div()
                    .id("federation-knowledge-output")
                    .max_h(px(240.0))
                    .overflow_y_scroll()
                    .rounded(px(9.0))
                    .bg(rgb(theme::new_session_fill()))
                    .p(px(10.0))
                    .child(
                        txt(
                            output.chars().take(4_000).collect::<String>(),
                            10.0,
                            theme::muted(),
                        )
                        .font_family("monospace")
                        .line_height(px(15.0)),
                    ),
            );
        }
        if let Some(error) = &shell.right.federation.knowledge_error {
            ask = ask.child(banner(error.clone(), false));
        }
    }
    column = column.child(ask);

    // Publish: the target peer and the payload, both chosen here.
    let mut publish = card();
    publish = publish.child(card_header(
        IconName::Send,
        VIOLET,
        lang.tr("right.federation.publishTitle"),
        lang.tr("right.federation.publishSubtitle"),
        None,
    ));
    if shell.right.federation.peers.is_empty() {
        publish = publish.child(empty_state(
            lang.tr("right.federation.noPeers"),
            IconName::Send,
        ));
    } else {
        publish = publish.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(icon(IconName::Users, 13.0, theme::faint()))
                .child(txt(
                    lang.tr("right.federation.peerLabel"),
                    10.0,
                    theme::faint(),
                ))
                .child(div().flex_1().min_w_0().child(peer_picker(
                    shell,
                    "federation-publish-peer",
                    cx,
                ))),
        );
        publish = publish.child(field(
            &shell.federation_knowledge_payload,
            "federation-field-knowledge-payload",
        ));
        let fallback_peer = shell.right.federation.peers.first().cloned();
        publish =
            publish.child(solid_button(
                "federation-knowledge-publish",
                lang.tr("right.federation.knowledgePublish"),
                true,
                true,
                cx,
                move |this, cx| {
                    let index = this.right.federation.selected.or(
                        if this.right.federation.peers.is_empty() {
                            None
                        } else {
                            Some(0)
                        },
                    );
                    let Some(peer) = index
                        .and_then(|index| this.right.federation.peers.get(index).cloned())
                        .or_else(|| fallback_peer.clone())
                    else {
                        this.right.federation.error =
                            Some(this.state.lang.tr("right.federation.noPeer"));
                        cx.notify();
                        return;
                    };
                    let Ok(peer_id) = agent::capability::NodeId::new(peer.node.clone()) else {
                        return;
                    };
                    let raw = this
                        .federation_knowledge_payload
                        .read(cx)
                        .value()
                        .to_string();
                    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&raw) else {
                        this.right.federation.error =
                            Some(this.state.lang.tr("right.federation.knowledgeBadJson"));
                        cx.notify();
                        return;
                    };
                    let Some(runtime) = crate::federation_node::running() else {
                        this.right.federation.error =
                            Some(this.state.lang.tr("right.federation.notRunning"));
                        cx.notify();
                        return;
                    };
                    let node = std::sync::Arc::clone(runtime.node());
                    let origin = node.node();
                    let now = agent::capability::unix_ms();
                    let item = federation::KnowledgeItem {
                        id: format!("ui-{now}"),
                        class: federation::KnowledgeClass::Public,
                        workspace: None,
                        payload: payload.clone(),
                        provenance: federation::Provenance {
                            origin,
                            source: "bsh:ui".to_string(),
                            issued_at_ms: now,
                            expires_at_ms: None,
                            content_hash: federation::knowledge_hash(&payload),
                        },
                        retention_ms: None,
                    };
                    let publication = federation::KnowledgePublication::new(vec![item]);
                    let context = agent::capability::InvocationContext::local(
                        agent::capability::InvocationId::new(format!("publish-{now}")),
                        agent::capability::Principal::Local {
                            session: "federation-pane".to_string(),
                            preset: String::new(),
                        },
                        agent::capability::WorkspaceId::new("ws:federation").expect("a workspace"),
                    );
                    let op = this.fed_op_begin(
                        "right.federation.opPublish",
                        display_name(peer.name.as_deref(), &peer.node),
                    );
                    this.fed_op_set(op, FederationActivityState::Running, None);
                    let handle = crate::extension::spawn_async(async move {
                        node.publish_knowledge(&peer_id, &publication, &context)
                            .await
                    });
                    cx.spawn(async move |this, cx| {
                        let outcome = handle.await;
                        let _ = this.update(cx, |this, cx| {
                            match outcome {
                                Ok(Ok(accepted)) => {
                                    this.right.federation.knowledge_notice =
                                        Some(crate::i18n::tr_args(
                                            this.state.lang,
                                            "right.federation.knowledgePublished",
                                            &[("count", accepted.to_string())],
                                        ));
                                    this.right.federation.error = None;
                                    this.fed_op_set(op, FederationActivityState::Succeeded, None);
                                }
                                Ok(Err(error)) => {
                                    this.right.federation.error = Some(error.to_string());
                                    this.fed_op_set(
                                        op,
                                        FederationActivityState::Failed,
                                        this.right.federation.error.clone(),
                                    );
                                }
                                Err(error) => {
                                    this.right.federation.error = Some(error.to_string());
                                    this.fed_op_set(
                                        op,
                                        FederationActivityState::Failed,
                                        this.right.federation.error.clone(),
                                    );
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                    cx.notify();
                },
            ));
    }
    column = column.child(publish);

    // Served knowledge: what this node currently answers with.
    let mut served = card();
    served = served.child(card_header(
        IconName::Library,
        theme::success(),
        crate::i18n::tr_args(
            lang,
            "right.federation.servedTitle",
            &[(
                "count",
                shell.right.federation.knowledge_items.len().to_string(),
            )],
        ),
        lang.tr("right.federation.servedSubtitle"),
        None,
    ));
    if shell.right.federation.knowledge_items.is_empty() {
        served = served.child(empty_state(
            lang.tr("right.federation.noKnowledge"),
            IconName::Library,
        ));
    }
    for (index, item) in shell
        .right
        .federation
        .knowledge_items
        .iter()
        .take(6)
        .enumerate()
    {
        let id = item.id.clone();
        served = served.child(
            hairline_all(
                div()
                    .id(SharedString::from(format!("federation-knowledge-{index}")))
                    .debug_selector(move || format!("federation-knowledge-{index}"))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(9.0))
                    .rounded(px(ROW_RADIUS))
                    .bg(rgb(theme::new_session_fill())),
            )
            .border_color(rgb(theme::border_soft()))
            .child(icon_tile(IconName::BookOpen, theme::success()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .flex_1()
                    .min_w_0()
                    .child(
                        txt(short_node(&item.id), 11.0, theme::text())
                            .font_family("monospace")
                            .text_ellipsis(),
                    )
                    .child(txt(
                        format!("{} · {}", item.class, item.source),
                        10.0,
                        theme::faint(),
                    )),
            )
            .child(icon_button(
                SharedString::from(format!("federation-knowledge-revoke-{index}")),
                IconName::Trash,
                lang.tr("right.federation.remove"),
                theme::faint(),
                cx,
                move |this, cx| {
                    let Some(runtime) = crate::federation_node::running() else {
                        return;
                    };
                    match runtime.node().revoke_knowledge(&id) {
                        Ok(_) => {
                            this.right.federation.notice =
                                Some(this.state.lang.tr("right.federation.knowledgeRevoked"));
                        }
                        Err(error) => {
                            this.right.federation.error = Some(error.to_string());
                        }
                    }
                    this.reload_federation_quiet();
                    cx.notify();
                },
            )),
        );
    }
    column = column.child(served);
    column.into_any_element()
}

// ---------------------------------------------------------------------------
// Advanced: the raw controls, behind one disclosure.
// ---------------------------------------------------------------------------

fn advanced_card(shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    let lang = shell.state.lang;
    let mut card = card();
    card = card.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(icon_tile(IconName::SlidersHorizontal, theme::muted()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .flex_1()
                    .min_w_0()
                    .child(
                        txt(
                            lang.tr("right.federation.advancedTitle"),
                            13.0,
                            theme::text(),
                        )
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .child(txt(
                        lang.tr("right.federation.advancedSubtitle"),
                        10.0,
                        theme::faint(),
                    )),
            )
            .child(solid_button(
                "federation-advanced-toggle",
                lang.tr("right.federation.hideAdvanced"),
                false,
                true,
                cx,
                |this, cx| {
                    this.right.federation.advanced = false;
                    cx.notify();
                },
            )),
    );

    // Node name: the identity is read automatically; renaming is the one
    // identity control a reader needs, and it stays here.
    card = card.child(section_title(lang.tr("right.federation.nodeName")));
    card = card.child(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(field(&shell.federation_name, "federation-field-name"))
            .child(solid_button(
                "federation-name-save",
                lang.tr("right.federation.save"),
                true,
                true,
                cx,
                move |this, cx| {
                    let raw = this.federation_name.read(cx).value().trim().to_string();
                    let configured = if raw.is_empty() { None } else { Some(raw) };
                    this.state.settings.global.federation.node_name = configured.clone();
                    if let Some(runtime) = crate::federation_node::running() {
                        runtime.set_name(configured);
                    }
                    this.save_settings(cx);
                    let shown = this.federation_name.read(cx).value().to_string();
                    this.right.federation.notice = Some(crate::i18n::tr_args(
                        this.state.lang,
                        "right.federation.nameStored",
                        &[("name", shown)],
                    ));
                    cx.notify();
                },
            )),
    );
    card = card.child(txt(
        lang.tr("right.federation.nodeNameHint"),
        10.0,
        theme::faint(),
    ));

    // The three feature switches. They apply at the next launch or turn, and
    // the row says which, so a pressed switch is not mistaken for a live one.
    card = card.child(switch_row(
        shell,
        "federation-run-node",
        lang.tr("right.federation.runNode"),
        lang.tr("right.federation.runNodeHint"),
        shell.state.settings.global.federation.enabled,
        |settings, on| settings.global.federation.enabled = on,
        cx,
    ));
    card = card.child(switch_row(
        shell,
        "federation-remote-tools",
        lang.tr("right.federation.remoteTools"),
        lang.tr("right.federation.remoteToolsHint"),
        shell.state.settings.global.federation.include_remote_tools,
        |settings, on| settings.global.federation.include_remote_tools = on,
        cx,
    ));
    card = card.child(switch_row(
        shell,
        "federation-knowledge-service",
        lang.tr("right.federation.knowledgeService"),
        lang.tr("right.federation.knowledgeServiceHint"),
        shell.state.settings.global.federation.knowledge_enabled,
        |settings, on| settings.global.federation.knowledge_enabled = on,
        cx,
    ));

    // Exposure: the node's own list, checked one row at a time.
    card = card.child(section_title(lang.tr("right.federation.exposeTitle")));
    let exposed = shell.state.settings.global.federation.expose.clone();
    let needle = shell
        .federation_expose_search
        .read(cx)
        .value()
        .trim()
        .to_lowercase();
    let mut candidates = expose_candidates(lang, &shell.right.federation.extension_tools);
    for id in &exposed {
        if !candidates.iter().any(|(candidate, _)| candidate == id) {
            candidates.push((id.clone(), capability_label(id)));
        }
    }
    if !needle.is_empty() {
        candidates.retain(|(id, label)| {
            id.to_lowercase().contains(&needle) || label.to_lowercase().contains(&needle)
        });
    }

    let shell_entity = cx.entity();
    let menu_candidates = candidates.clone();
    let menu_exposed = exposed.clone();
    let menu_count = exposed.len();
    card = card.child(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(field(
                &shell.federation_expose_search,
                "federation-field-expose-search",
            ))
            .child(
                div()
                    .debug_selector(|| "federation-expose-menu".to_string())
                    .child(
                        Button::new("federation-expose-menu")
                            .ghost()
                            .h(px(32.0))
                            .px(px(10.0))
                            .rounded(px(9.0))
                            .bg(rgb(theme::new_session_fill()))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .cursor_pointer()
                            .child(txt(
                                format!(
                                    "{} ({menu_count})",
                                    lang.tr("right.federation.exposeMenu")
                                ),
                                11.0,
                                theme::text(),
                            ))
                            .child(txt("▾", 10.0, theme::faint()))
                            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                                let mut menu = menu;
                                if menu_candidates.is_empty() {
                                    menu = menu.item(PopupMenuItem::label(crate::i18n::tr(
                                        lang,
                                        "right.federation.exposeNone",
                                    )));
                                }
                                for (capability, label) in menu_candidates.iter().cloned() {
                                    let shell = shell_entity.clone();
                                    let active = menu_exposed.iter().any(|id| id == &capability);
                                    let id = capability.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(label).checked(active).on_click(
                                            move |_, _, cx| {
                                                shell.update(cx, |this, cx| {
                                                    toggle_expose(this, &id, cx);
                                                });
                                            },
                                        ),
                                    );
                                }
                                let shell = shell_entity.clone();
                                menu = menu.item(PopupMenuItem::separator()).item(
                                    PopupMenuItem::new(crate::i18n::tr(
                                        lang,
                                        "right.federation.exposeClear",
                                    ))
                                    .on_click(
                                        move |_, _, cx| {
                                            shell.update(cx, |this, cx| {
                                                this.state
                                                    .settings
                                                    .global
                                                    .federation
                                                    .expose
                                                    .clear();
                                                this.save_settings(cx);
                                                this.right.federation.notice = Some(
                                                    this.state
                                                        .lang
                                                        .tr("right.federation.exposeChanged"),
                                                );
                                                this.right.federation.error = None;
                                                cx.notify();
                                            });
                                        },
                                    ),
                                );
                                menu
                            }),
                    ),
            ),
    );
    for (index, (id, label)) in candidates.iter().enumerate() {
        let active = exposed.iter().any(|exposed| exposed == id);
        let id_for_click = id.clone();
        card = card.child(
            div()
                .id(SharedString::from(format!("federation-expose-{index}")))
                .debug_selector(move || format!("federation-expose-{index}"))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::new_session_fill())))
                .child(icon(
                    if active {
                        IconName::CircleCheck
                    } else {
                        IconName::Circle
                    },
                    14.0,
                    if active {
                        theme::success()
                    } else {
                        theme::faint()
                    },
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .flex_1()
                        .min_w_0()
                        .child(txt(label.clone(), 11.0, theme::text()))
                        .child(txt(id.clone(), 9.0, theme::faint()).font_family("monospace")),
                )
                .child(pill(
                    if active {
                        lang.tr("right.federation.badgeActive")
                    } else {
                        lang.tr("right.federation.badgeInactive")
                    },
                    if active {
                        theme::success()
                    } else {
                        theme::faint()
                    },
                    if active {
                        theme::soft(theme::success())
                    } else {
                        theme::hover()
                    },
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    toggle_expose(this, &id_for_click, cx);
                })),
        );
    }
    card = card.child(txt(
        lang.tr("right.federation.exposeHint"),
        10.0,
        theme::faint(),
    ));
    if let Some(root) = crate::federation_node::expose_root(&shell.state.settings) {
        card = card.child(txt(
            crate::i18n::tr_args(
                lang,
                "right.federation.exposeRootLine",
                &[("root", root.display().to_string())],
            ),
            10.0,
            theme::faint(),
        ));
    } else {
        card = card.child(txt(
            lang.tr("right.federation.exposeNoRoot"),
            10.0,
            theme::error(),
        ));
    }
    card.into_any_element()
}

/// The rows the expose menu offers: the node's read-only tools, then every
/// extension tool the host listed. A duplicate id keeps the first row, so a
/// built-in always wins over an extension with the same qualified name.
fn expose_candidates(
    lang: crate::i18n::Lang,
    extension_tools: &[crate::extension::host::NodeToolInfo],
) -> Vec<(String, String)> {
    let mut candidates: Vec<(String, String)> = crate::federation_node::exposable_capabilities()
        .iter()
        .map(|name| (format!("builtin/{name}"), expose_label(lang, name)))
        .collect();
    for info in extension_tools {
        let id = format!("builtin/{}", info.qualified);
        if !candidates.iter().any(|(candidate, _)| candidate == &id) {
            candidates.push((
                id,
                format!(
                    "{} — {} ({})",
                    info.qualified, info.description, info.extension
                ),
            ));
        }
    }
    candidates
}

fn expose_label(lang: crate::i18n::Lang, name: &str) -> String {
    lang.tr(match name {
        "read_file" => "right.federation.exposeReadFile",
        "read_files" => "right.federation.exposeReadFiles",
        "glob" => "right.federation.exposeGlob",
        "grep" => "right.federation.exposeGrep",
        "workspace_context" => "right.federation.exposeWorkspaceContext",
        "skill" => "right.federation.exposeSkill",
        other => return capability_label(other),
    })
}

fn toggle_expose(this: &mut Shell, id: &str, cx: &mut Context<Shell>) {
    if crate::federation_node::expose_root(&this.state.settings).is_none() {
        this.right.federation.error = Some(this.state.lang.tr("right.federation.exposeNoRoot"));
        this.right.federation.notice = None;
        cx.notify();
        return;
    }
    let op = this.fed_op_begin("right.federation.opExpose", capability_label(id));
    let expose = &mut this.state.settings.global.federation.expose;
    if let Some(position) = expose.iter().position(|grant| grant == id) {
        expose.remove(position);
    } else {
        expose.push(id.to_string());
    }
    this.save_settings(cx);
    this.right.federation.notice = Some(this.state.lang.tr("right.federation.exposeChanged"));
    this.right.federation.error = None;
    this.fed_op_set(op, FederationActivityState::Succeeded, None);
    cx.notify();
}

fn section_title(label: String) -> Div {
    div()
        .pt(px(2.0))
        .child(txt(label, 11.0, theme::muted()).font_weight(FontWeight::MEDIUM))
}

fn switch_row(
    shell: &Shell,
    id: &'static str,
    label: String,
    hint: String,
    checked: bool,
    set: fn(&mut crate::settings::Settings, bool),
    cx: &mut Context<Shell>,
) -> AnyElement {
    hairline_bottom(
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .py(px(8.0))
            .border_color(rgb(theme::border_soft()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .flex_1()
                    .min_w_0()
                    .child(txt(label, 12.0, theme::text()).font_weight(FontWeight::MEDIUM))
                    .child(txt(hint, 10.0, theme::faint()).line_height(px(14.0))),
            )
            .child(shell.switch(
                id,
                checked,
                true,
                move |this, on, cx| {
                    set(&mut this.state.settings, on);
                    match this.state.settings.save() {
                        Ok(()) => {
                            this.right.federation.notice =
                                Some(this.state.lang.tr("right.federation.saved"));
                            this.right.federation.error = None;
                        }
                        Err(error) => {
                            this.right.federation.error = Some(error.to_string());
                            this.right.federation.notice = None;
                        }
                    }
                    cx.notify();
                },
                cx,
            )),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        activity_label, capability_label, capability_label_localized, expose_candidates,
        grantable_label, grantable_label_with, opaque, FederationActivityState,
    };
    use crate::extension::host::NodeToolInfo;
    use crate::i18n::Lang;

    /// A gradient stop must keep the RGB token it was given and stay opaque.
    ///
    /// gpui-pre's `rgba` reads `0xRRGGBBAA`, so handing it an `0xRRGGBB`
    /// token turned the blue byte into alpha and dropped red - the hero
    /// rendered cyan until this helper shifted the colour up.
    #[test]
    fn a_gradient_stop_keeps_its_colour_and_is_opaque() {
        let colour = opaque(0x4176E6);
        assert!((colour.r * 255.0 - 0x41 as f32).abs() < 1.0);
        assert!((colour.g * 255.0 - 0x76 as f32).abs() < 1.0);
        assert!((colour.b * 255.0 - 0xE6 as f32).abs() < 1.0);
        assert_eq!(colour.a, 1.0);
    }

    /// The peers card groups a grant into the reference's three families and
    /// falls back to the capability's own name for anything else.
    #[test]
    fn capabilities_read_as_their_group() {
        assert_eq!(
            capability_label_localized(Lang::En, "builtin/knowledge_query"),
            "Knowledge"
        );
        assert_eq!(
            capability_label_localized(Lang::En, "builtin/resource_open"),
            "Resource"
        );
        assert_eq!(
            capability_label_localized(Lang::En, "builtin/delegate"),
            "Work"
        );
        assert_eq!(
            capability_label_localized(Lang::En, "builtin/read_file"),
            "read file"
        );
        assert_eq!(
            capability_label("builtin/workspace_context"),
            "workspace context"
        );
    }

    /// Extension tools join both menus, after the node's built-in surface.
    #[test]
    fn extension_tools_join_the_expose_and_grant_menus() {
        let tools = vec![NodeToolInfo {
            qualified: "extdev__scaffold".to_string(),
            extension: "Extension DevKit".to_string(),
            description: "Scaffold a package".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }];
        let rows = expose_candidates(Lang::En, &tools);
        assert_eq!(rows[0].0, "builtin/read_file");
        assert!(rows.iter().any(|(id, label)| {
            id == "builtin/extdev__scaffold" && label.contains("Extension DevKit")
        }));
        assert_eq!(
            grantable_label_with(Lang::En, "builtin/extdev__scaffold", &tools),
            "extdev__scaffold — Scaffold a package (Extension DevKit)"
        );
    }

    /// The grant menu is the node's own surface, so it always has entries even
    /// when the selected peer is offline and advertises nothing.
    #[test]
    fn the_grant_menu_names_local_capabilities() {
        assert!(grantable_label(Lang::En, "builtin/read_file").starts_with("read_file"));
        assert!(grantable_label(Lang::En, "builtin/knowledge_query").contains("knowledge_query"));
        assert_eq!(grantable_label(Lang::En, "custom/thing"), "thing");
        let grantable = crate::federation_node::grantable_capabilities();
        assert!(grantable.iter().any(|id| id == "builtin/read_file"));
        assert!(grantable.iter().any(|id| id == "builtin/knowledge_publish"));
    }

    /// The expose dropdown's two actions and the peer-id label are keys, not
    /// literals: a missing key renders itself, which is what put
    /// "right.federation.exposeClear" in the menu.
    #[test]
    fn the_expose_dropdown_is_translated() {
        assert_eq!(
            Lang::En.tr("right.federation.exposeClear"),
            "Clear exposed capabilities"
        );
        assert_eq!(
            Lang::Zh.tr("right.federation.exposeClear"),
            "清空已暴露能力"
        );
        assert_eq!(
            Lang::En.tr("right.federation.exposeNone"),
            "No capabilities exposed"
        );
        assert_eq!(Lang::En.tr("right.federation.peerId"), "Peer node id");
    }
    /// Every operation state has a word, and it is the reference's word.
    #[test]
    fn every_activity_state_names_itself() {
        assert_eq!(
            activity_label(Lang::En, FederationActivityState::Pending),
            "Pending"
        );
        assert_eq!(
            activity_label(Lang::En, FederationActivityState::Running),
            "Running"
        );
        assert_eq!(
            activity_label(Lang::En, FederationActivityState::Succeeded),
            "Succeeded"
        );
        assert_eq!(
            activity_label(Lang::En, FederationActivityState::Failed),
            "Failed"
        );
        assert_eq!(
            activity_label(Lang::En, FederationActivityState::Revoked),
            "Revoked"
        );
        assert_eq!(
            activity_label(Lang::En, FederationActivityState::Removed),
            "Removed"
        );
        assert_eq!(
            activity_label(Lang::En, FederationActivityState::Cancelled),
            "Cancelled"
        );
    }
}
