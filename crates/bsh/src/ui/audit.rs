//! The Audit tab.
//!
//! This is the deterministic view over the persisted tool events and the
//! policy denial ring: one row per turn, expandable to the calls it made and the
//! refusals it met. It does not call a model and generates no prose - every
//! figure comes from a count over the records.
//!
//! What it sees, and what it cannot, is stated on the page itself in two
//! layered notes. A shell call records the command, its output, and the
//! confinement the tool's own result carried per call - the sandbox backend and
//! the network reach (both drawn in the call detail, read from the stored
//! record) - but the files the command creates are not individually attributed.
//! Network REACHABILITY is gated and recorded in two tiers: the command line's
//! named endpoints, then the sandbox proxy asking the policy per connection. The
//! PAYLOAD is not inspected (TLS is tunnelled); "controlled" means reachability,
//! never content, and a host assembled at runtime is invisible to the lexical
//! tier and caught only at the connection tier. Those are limits of the data, not
//! of this view.
//!
//! The list is a timeline of turns, not a grouped table by resource, because the
//! question "what happened, in order?" is the one an audit reader asks first.
//! Clicking a turn opens its calls; clicking a call opens its bounded, redacted
//! preview and, on demand, the whole stored record. The full record is read only
//! in that last branch, so a huge output never becomes a per-frame cost.

use gpui_kit::component::input::Input;
use gpui_kit::*;

use super::shell::Shell;
use super::{icon, txt, DivExt, IconName};
use crate::audit::{
    self, AuditAllows, AuditCall, AuditCategory, AuditOutcome, AuditPolicy, AuditRefusal,
    AuditTurn, AuditView,
};
use crate::i18n::tr_args;
use crate::state::Tab;
use crate::theme;

/// The facts the audit page's view model is built from.
///
/// The build walks every message and scans every tool output, so the page keeps
/// its result until one of these changes: the session's shape, the policy the
/// page prints, and the namespaces that classify a tool. Messages are
/// append-only and only the newest one mutates while a turn streams, so the
/// session's shape is its message count plus that tail.
#[derive(Clone, PartialEq)]
struct AuditKey {
    session: String,
    messages: usize,
    /// The newest message's own shape: the only message a live turn changes.
    /// A tool result FILLS an existing call in place, so the byte count and the
    /// recorded clocks are part of the shape, not only the call count.
    last_tools: usize,
    last_bytes: usize,
    last_clocks: usize,
    last_start: Option<u64>,
    last_ms: Option<u64>,
    /// The denial ring's shape, and how many of it blocked a call.
    denials: usize,
    blocking: usize,
    denials_total: u64,
    /// The applied policy, which the page and its export print.
    policy: AuditPolicy,
    /// The MCP namespaces, which decide a tools's category.
    mcp: Vec<String>,
}

/// The audit page's built view model, kept between frames.
#[derive(Default)]
pub(crate) struct AuditCache {
    key: Option<AuditKey>,
    view: std::rc::Rc<AuditView>,
}

impl Shell {
    /// The audit view for the active session, built from persisted records.
    ///
    /// The denial snapshot is the policy pane's own source of truth, so the two
    /// surfaces cannot disagree about what was refused. The live MCP namespaces
    /// let a namespaced tool classify as MCP rather than Other.
    pub(crate) fn audit_view(&self) -> std::rc::Rc<AuditView> {
        let Some(session) = self.state.active_session() else {
            return std::rc::Rc::new(AuditView::default());
        };
        let mcp = self.mcp_namespaces();
        let last = session.messages.last();
        let key = AuditKey {
            session: session.id.clone(),
            messages: session.messages.len(),
            last_tools: last.map_or(0, |message| message.tools.len()),
            last_bytes: last.map_or(0, |message| {
                message
                    .tools
                    .iter()
                    .map(|tool| tool.args.len() + tool.output.as_deref().map_or(0, str::len))
                    .sum()
            }),
            last_clocks: last.map_or(0, |message| {
                message
                    .tools
                    .iter()
                    .map(|tool| usize::from(tool.ms.is_some()) + usize::from(tool.at.is_some()))
                    .sum()
            }),
            last_start: last.and_then(|message| {
                message
                    .timing()
                    .and_then(|timing: &crate::model::MessageTiming| timing.start)
            }),
            last_ms: last.and_then(|message| {
                message
                    .timing()
                    .and_then(|timing: &crate::model::MessageTiming| timing.ms)
            }),
            denials: self
                .state
                .policy_audit
                .as_ref()
                .map_or(0, |audit| audit.denials.len()),
            blocking: self.blocking_denials_count(),
            denials_total: self
                .state
                .policy_audit
                .as_ref()
                .map_or(0, |audit| audit.denials_total),
            policy: self.audit_policy(),
            mcp: mcp.clone(),
        };
        let mut cache = self.audit_cache.borrow_mut();
        if cache.key.as_ref() != Some(&key) {
            // ADVISORY refusals are refusals the reader was TOLD about, not ones
            // that stopped a call: the Audit tab counts what was blocked, while
            // the Policy pane lists the advisory ring on its own. Mixing them
            // here would make "refusals" mean two different things on one page.
            let denials = self.blocking_denials();
            let view = AuditView::build(session, &denials, &mcp).with_policy(self.audit_policy());
            cache.key = Some(key);
            cache.view = std::rc::Rc::new(view);
        }
        cache.view.clone()
    }

    /// The live MCP namespaces, sorted and deduped: the classifier's own input.
    fn mcp_namespaces(&self) -> Vec<String> {
        let mut mcp: Vec<String> = self
            .state
            .mcp
            .tools()
            .map(|tool| tool.namespace.clone())
            .collect();
        mcp.sort();
        mcp.dedup();
        mcp
    }

    /// The denials that blocked a call: the advisory ring is not one of them.
    fn blocking_denials(&self) -> Vec<agent::policy::PolicyAuditEntry> {
        self.state
            .policy_audit
            .as_ref()
            .map(|audit| {
                audit
                    .denials
                    .iter()
                    .filter(|entry| !entry.advisory)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How many ring entries blocked a call, without cloning the ring.
    fn blocking_denials_count(&self) -> usize {
        self.state.policy_audit.as_ref().map_or(0, |audit| {
            audit.denials.iter().filter(|entry| !entry.advisory).count()
        })
    }

    /// The policy that was applied and the results it counted.
    ///
    /// The snapshot is the policy pane's own source; the live set is the
    /// fallback for a frame that has not refreshed its snapshot yet. Permission,
    /// evaluator and the allow counters are facts the policy itself records -
    /// never an inference from the tool list.
    pub(crate) fn audit_policy(&self) -> AuditPolicy {
        let mut policy = if let Some(view) = self.state.policy_audit.as_ref() {
            AuditPolicy {
                permission: view.permission.clone(),
                preset: view.preset.clone(),
                presets_disabled: view.presets_disabled,
                evaluator: view.evaluator.clone(),
                error: view.error.clone(),
                allows: AuditAllows {
                    total: view.allows.total,
                    reads: view.allows.reads,
                    writes: view.allows.writes,
                    egress: view.allows.egress,
                    spawns: view.allows.spawns,
                    capabilities: view.allows.capabilities,
                },
                denials_total: view.denials_total,
                ..AuditPolicy::default()
            }
        } else if let Some(live) = self.state.policy.as_ref() {
            let allows = live.allows();
            AuditPolicy {
                permission: live.data().permission.clone(),
                evaluator: live.evaluator().to_string(),
                error: live.error().map(ToString::to_string),
                allows: AuditAllows {
                    total: allows.total,
                    reads: allows.reads,
                    writes: allows.writes,
                    egress: allows.egress,
                    spawns: allows.spawns,
                    capabilities: allows.capabilities,
                },
                denials_total: live.denials_total(),
                ..AuditPolicy::default()
            }
        } else {
            AuditPolicy::default()
        };
        // The APPLIED policy is the set's own data: preset and the scopes it
        // installed. These are facts the policy carries, not a reading of the
        // tool list, so they can be shown beside the permission it enforces.
        if let Some(live) = self.state.policy.as_ref() {
            let data = live.data();
            policy.preset = data.preset.clone();
            policy.presets_disabled = data.presets_disabled;
            policy.read_scope = data.preset_read_roots.len();
            policy.write_scope = data.preset_write_roots.len();
            policy.deny_roots = data.preset_deny_roots.len();
            policy.egress_allowlist = data.egress_allowlist.len();
            // RESTRICTED is the EFFECTIVE state, not only the preset's field:
            // an absent workspace file leaves egress unrestricted, while a
            // present file - even one that declares no rule - constrains it (to
            // the allowlist, or to a deny-all when it is empty). Reading only
            // the preset flag would print "unrestricted" for an empty file.
            policy.egress_restricted = data.preset_egress_restricted || !data.egress_unrestricted;
        }
        policy
    }

    /// Jump to the turn in Chat, reusing the rail's own scroll machinery.
    ///
    /// The audit numbers turns with the same ordinal the rail does, so the
    /// target is the nearest user message at or after the turn's first message.
    /// The newest turn re-engages the tail; every other jump leaves it, exactly
    /// like a rail mark.
    pub(crate) fn jump_to_audit_turn(&mut self, index: usize, cx: &mut Context<Self>) {
        let view = self.audit_view();
        let Some(turn) = view.turns.iter().find(|turn| turn.index == index) else {
            return;
        };
        let turns = self.turn_indices();
        if turns.is_empty() {
            return;
        }
        let ordinal = turns
            .iter()
            .position(|message| *message >= turn.message)
            .unwrap_or(turns.len() - 1);
        let newest = ordinal == turns.len() - 1;
        self.state.tab = Tab::Chat;
        if newest {
            // The tail is re-engaged, and the render pass brings the newest
            // turn into view from wherever the reader was.
            self.jump_to_turn(1 + turns[ordinal], true);
        } else {
            self.following_tail = false;
            // One mounted Chat frame first, then the request: see
            // Shell::audit_jump_to.
            self.audit_jump_to = Some((1 + turns[ordinal], 1));
        }
        cx.notify();
    }

    /// Open or close one turn's calls.
    pub(crate) fn toggle_audit_turn(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.audit_turn_open.remove(&index) {
            self.audit_turn_open.insert(index);
        }
        cx.notify();
    }

    /// Open or close one call's preview.
    pub(crate) fn toggle_audit_call(&mut self, turn: usize, call: usize, cx: &mut Context<Self>) {
        let key = (turn, call);
        if !self.audit_open.remove(&key) {
            self.audit_open.insert(key);
        }
        cx.notify();
    }

    /// Show or hide one call's full stored text.
    pub(crate) fn toggle_audit_full(&mut self, turn: usize, call: usize, cx: &mut Context<Self>) {
        let key = (turn, call);
        if !self.audit_full.remove(&key) {
            self.audit_full.insert(key);
        }
        cx.notify();
    }

    /// Export the audit as Markdown plus JSON through the platform save dialog.
    pub(crate) fn export_audit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A copy, not the shared cache: the export runs on the background
        // executor, which wants an owned, Send value.
        let view = (*self.audit_view()).clone();
        if view.totals.calls == 0 && view.totals.refusals == 0 {
            let notice = self.state.lang.tr("audit.exportEmpty");
            self.state.notice(notice);
            cx.notify();
            return;
        }
        let dir = self
            .state
            .workspace_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let target = cx.prompt_for_new_path(&dir, Some("audit.md"));
        let lang = self.state.lang;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = target.await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { crate::export::write_audit(&view, lang, &path) })
                .await;
            let _ = this.update_in(cx, |this, _window, cx| {
                let text = match result {
                    Ok((markdown, json)) => tr_args(
                        this.state.lang,
                        "audit.exported",
                        &[
                            ("path", markdown.display().to_string()),
                            ("json", json.display().to_string()),
                        ],
                    ),
                    Err(_) => this.state.lang.tr("audit.exportFailed"),
                };
                this.state.notice(text);
                cx.notify();
            });
        })
        .detach();
    }

    /// The filter toolbar: the reference-style measure toggles, search, export.
    /// Padding inside one audit card.
    const CARD_PAD: f32 = 12.0;
    /// A card's corner radius, matching the rest of the shell's surfaces.
    const CARD_RADIUS: f32 = 12.0;
    /// The minimum width one summary cell needs before the row wraps.
    ///
    /// The cells share the row with `flex_grow`, so this is a floor rather than a
    /// fixed column: a wide window fits them all on one line instead of stranding
    /// the last cell on a row of its own.
    const STAT_CELL_MIN_W: f32 = 96.0;
    /// The height of a summary cell's value line.
    const STAT_VALUE_H: f32 = 22.0;
    /// The turn row's own height: exact, which is what lets the window place a
    /// row it does not build.
    pub(crate) const TURN_ROW_H: f32 = 38.0;
    /// The measure bar's full width.
    const MEASURE_W: f32 = 96.0;
    /// The measure bar's thickness.
    const MEASURE_H: f32 = 5.0;
    fn audit_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = [
            self.state.lang.tr("traj.duration"),
            self.state.lang.tr("traj.turns"),
            self.state.lang.tr("traj.calls"),
        ];
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(32.0))
            .px(px(6.0))
            .gap(px(8.0))
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::page()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .children(modes.into_iter().enumerate().map(|(index, label)| {
                        let on = self.audit_mode == index;
                        let other = label.clone();
                        div()
                            .debug_selector(|| format!("audit-mode-{index}"))
                            .id(SharedString::from(format!("audit-mode-{index}")))
                            .h(px(20.0))
                            .px(px(7.0))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .rounded(px(6.0))
                            .when(on, |cell| {
                                cell.bg(rgb(theme::hover()))
                                    .child(txt(label, 12.0, theme::text()))
                            })
                            .when(!on, |cell| cell.child(txt(other, 12.0, theme::faint())))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.audit_mode = index;
                                cx.notify();
                            }))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .children(
                        [
                            (None, "audit.category.all"),
                            (Some(AuditCategory::FileRead), "audit.category.fileRead"),
                            (Some(AuditCategory::FileWrite), "audit.category.fileWrite"),
                            (Some(AuditCategory::Shell), "audit.category.shell"),
                            (Some(AuditCategory::Network), "audit.category.network"),
                            (Some(AuditCategory::Browser), "audit.category.browser"),
                            (Some(AuditCategory::Mcp), "audit.category.mcp"),
                            (Some(AuditCategory::Subagent), "audit.category.subagent"),
                            (Some(AuditCategory::Other), "audit.category.other"),
                        ]
                        .into_iter()
                        .map(|(category, key)| {
                            let label = self.state.lang.tr(key);
                            let on = self.audit_category == category;
                            div()
                                .id(SharedString::from(format!("audit-category-{}", key)))
                                .h(px(20.0))
                                .px(px(5.0))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .rounded(px(5.0))
                                .when(on, |cell| {
                                    cell.bg(rgb(theme::hover())).child(txt(
                                        label.clone(),
                                        10.0,
                                        theme::text(),
                                    ))
                                })
                                .when(!on, |cell| {
                                    cell.child(txt(label.clone(), 10.0, theme::faint()))
                                })
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    this.audit_category = category;
                                    this.audit_page = 0;
                                    this.audit_scroll.set_offset(gpui_kit::point(
                                        gpui_kit::px(0.0),
                                        gpui_kit::px(0.0),
                                    ));
                                    cx.notify();
                                }))
                        }),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .h(px(22.0))
                    .w(px(164.0))
                    .flex_none()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .bg(rgb(theme::new_session_fill()))
                    .child(icon(IconName::Search, 11.0, theme::faint()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.audit_search).appearance(false)),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "audit-export".to_string())
                    .id("audit-export")
                    .h(px(20.0))
                    .px(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .flex_none()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|cell| cell.bg(rgb(theme::hover())))
                    .child(icon(IconName::Download, 11.0, theme::muted()))
                    .child(txt(
                        self.state.lang.tr("audit.export"),
                        12.0,
                        theme::muted(),
                    ))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.export_audit(window, cx);
                    })),
            )
    }

    /// The deterministic summary line: every figure is a count over the records.
    fn audit_summary(&self, view: &AuditView) -> impl IntoElement {
        let lang = self.state.lang;
        let totals = view.totals;
        // The run-on sentence is gone: every figure is a cell in the grid
        // below, so there is nothing to join here.
        let rules = view.rule_counts();
        let rules_line = if rules.is_empty() {
            None
        } else {
            let unnamed = lang.tr("audit.ruleUnnamed");
            Some(
                rules
                    .iter()
                    .map(|(rule, count)| {
                        format!("{} x{}", rule.as_deref().unwrap_or(unnamed.as_str()), count)
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        };
        // A grid of value-over-label cells rather than one run-on sentence: the
        // figures a reader scans for are the big ones, and the labels sit under
        // them where they do not compete with the numbers.
        let cell = move |label: String, value: String, ink: u32| {
            div()
                .min_w(px(Self::STAT_CELL_MIN_W))
                .flex_grow(1.0)
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    txt(value, 17.0, ink)
                        .font_weight(FontWeight::SEMIBOLD)
                        .line_height(px(Self::STAT_VALUE_H)),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(txt(label, 11.0, theme::faint()).line_height(px(14.0))),
                )
        };
        let mut grid = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(8.0))
            .child(cell(
                lang.tr("audit.stat.turns"),
                totals.turns.to_string(),
                theme::text(),
            ))
            .child(cell(
                lang.tr("audit.stat.calls"),
                totals.calls.to_string(),
                theme::text(),
            ))
            .child(cell(
                lang.tr("audit.stat.reads"),
                totals.files_read.to_string(),
                theme::text(),
            ))
            .child(cell(
                lang.tr("audit.stat.writes"),
                totals.files_written.to_string(),
                theme::text(),
            ))
            .child(cell(
                lang.tr("audit.stat.commands"),
                totals.commands.to_string(),
                theme::text(),
            ))
            .child(cell(
                lang.tr("audit.stat.duration"),
                audit::duration_text(totals.duration_ms),
                theme::text(),
            ))
            // A refusal and a failure are the two figures that mean something is
            // wrong, so they carry ink only when they are non-zero.
            .child(cell(
                lang.tr("audit.stat.refusals"),
                totals.refusals.to_string(),
                if totals.refusals > 0 {
                    theme::error()
                } else {
                    theme::faint()
                },
            ))
            .child(cell(
                lang.tr("audit.stat.failed"),
                totals.failed.to_string(),
                if totals.failed > 0 {
                    theme::error()
                } else {
                    theme::faint()
                },
            ));
        if let Some(listed) = rules_line {
            grid = grid.child(
                div()
                    .min_w(px(Self::STAT_CELL_MIN_W))
                    .flex_grow(1.0)
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(
                                txt(listed, 13.0, theme::text())
                                    .font_weight(FontWeight::MEDIUM)
                                    .line_height(px(Self::STAT_VALUE_H)),
                            ),
                    )
                    .child(
                        txt(lang.tr("audit.stat.rules"), 11.0, theme::faint())
                            .line_height(px(14.0)),
                    ),
            );
        }
        div()
            .debug_selector(|| "audit-summary".to_string())
            .flex()
            .flex_row()
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::page()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(12.0))
                    .py(px(10.0))
                    .child(grid),
            )
    }

    /// The policy that was applied and the results it counted.
    ///
    /// This is where a policy question is answered without inference: the
    /// evaluator and permission come from the policy's own data, the allow
    /// counters from its ring, and the shown/total denial figures say how much
    /// of the bounded ring the per-turn rows actually carry.
    fn audit_policy_strip(&self, view: &AuditView) -> Div {
        let lang = self.state.lang;
        let policy = &view.policy;
        let allows = tr_args(
            lang,
            "audit.policy.allowsLine",
            &[
                ("reads", policy.allows.reads.to_string()),
                ("writes", policy.allows.writes.to_string()),
                ("egress", policy.allows.egress.to_string()),
                ("spawns", policy.allows.spawns.to_string()),
                ("capabilities", policy.allows.capabilities.to_string()),
            ],
        );
        let shown = tr_args(
            lang,
            "audit.policy.shown",
            &[
                ("shown", view.totals.refusals.to_string()),
                ("denied", policy.denials_total.to_string()),
            ],
        );
        let applied = tr_args(
            lang,
            "audit.policy.appliedLine",
            &[
                ("preset", policy.preset_cell(lang)),
                ("read", policy.read_scope.to_string()),
                ("write", policy.write_scope.to_string()),
                ("deny", policy.deny_roots.to_string()),
                ("egress", policy.egress_allowlist.to_string()),
                (
                    "egressMode",
                    lang.tr(if policy.egress_restricted {
                        "audit.policy.egressRestricted"
                    } else {
                        "audit.policy.egressOpen"
                    }),
                ),
            ],
        );
        let exec = tr_args(
            lang,
            "audit.policy.execLine",
            &[
                ("ok", view.totals.succeeded.to_string()),
                ("failed", view.totals.failed.to_string()),
                ("pending", view.totals.pending.to_string()),
                ("denied", view.totals.refusals.to_string()),
            ],
        );
        // A card, not a full-bleed strip: the policy is its own piece of
        // information, and a surface of its own separates "what this page
        // counted" from "under what rules". The figures a reader compares -
        // permission, checks, allowed, denied - are chips so they line up
        // instead of hiding inside a sentence.
        let chip = |label: String, ink: u32, bg: u32| {
            div()
                .h(px(20.0))
                .px(px(7.0))
                .flex()
                .items_center()
                .flex_none()
                .rounded(px(5.0))
                .bg(rgb(bg))
                .child(
                    txt(label, 11.0, ink)
                        .font_weight(FontWeight::MEDIUM)
                        .line_height(px(15.0)),
                )
        };
        let denied_ink = if policy.denials_total > 0 {
            theme::error()
        } else {
            theme::faint()
        };
        let verdict = if policy.denials_total == 0 {
            lang.tr("audit.policy.clean")
        } else {
            lang.tr("audit.policy.deniedSome")
        };
        let verdict_ink = if policy.denials_total == 0 {
            theme::success()
        } else {
            theme::error()
        };
        // One labelled row: a fixed muted key, then the value, so three rows
        // read as a table rather than three differently-shaped sentences.
        // A stable debug selector per labelled row: the figures a test (or a
        // future reader) looks for should be findable by name, not by position.
        let row = move |id: &'static str, label: String, value: String| {
            div()
                .debug_selector(move || id.to_string())
                .flex()
                .flex_row()
                .gap(px(10.0))
                .min_w_0()
                .child(
                    txt(label, 11.0, theme::faint())
                        .w(px(64.0))
                        .flex_none()
                        .line_height(px(17.0)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(txt(value, 11.0, theme::text()).line_height(px(17.0))),
                )
        };
        div()
            .debug_selector(|| "audit-policy".to_string())
            .flex_none()
            .px(px(12.0))
            .pt(px(8.0))
            .flex()
            .flex_col()
            .child(
                super::hairline_all(div())
                    .rounded(px(Self::CARD_RADIUS))
                    .bg(rgb(theme::new_session_fill()))
                    .border_color(rgb(theme::border_soft()))
                    .p(px(Self::CARD_PAD))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .flex_wrap()
                            .child(
                                div()
                                    .debug_selector(|| "audit-policy-badge".to_string())
                                    .h(px(20.0))
                                    .px(px(7.0))
                                    .flex()
                                    .items_center()
                                    .flex_none()
                                    .rounded(px(5.0))
                                    .bg(rgb(theme::hover()))
                                    .child(
                                        txt(lang.tr("audit.policy.title"), 11.0, theme::accent())
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .line_height(px(15.0)),
                                    ),
                            )
                            .child(chip(
                                policy.evaluator.clone(),
                                theme::muted(),
                                theme::hover(),
                            ))
                            .child(chip(
                                tr_args(
                                    lang,
                                    "audit.policy.chipPermission",
                                    &[("permission", policy.permission.clone())],
                                ),
                                theme::accent(),
                                theme::hover(),
                            ))
                            .child(chip(
                                tr_args(
                                    lang,
                                    "audit.policy.chipChecks",
                                    &[("count", policy.checks().to_string())],
                                ),
                                theme::muted(),
                                theme::hover(),
                            ))
                            .child(chip(
                                tr_args(
                                    lang,
                                    "audit.policy.chipAllowed",
                                    &[("count", policy.allows.total.to_string())],
                                ),
                                theme::success(),
                                theme::hover(),
                            ))
                            .child(chip(
                                tr_args(
                                    lang,
                                    "audit.policy.chipDenied",
                                    &[("count", policy.denials_total.to_string())],
                                ),
                                denied_ink,
                                theme::hover(),
                            ))
                            .child(div().flex_1())
                            .child(
                                txt(verdict, 11.0, verdict_ink)
                                    .font_weight(FontWeight::MEDIUM)
                                    .line_height(px(16.0)),
                            )
                            .when_some(policy.error.clone(), |row, error| {
                                row.child(
                                    txt(
                                        tr_args(lang, "audit.policy.error", &[("error", error)]),
                                        11.0,
                                        theme::error(),
                                    )
                                    .line_height(px(16.0)),
                                )
                            }),
                    )
                    .child(row(
                        "audit-policy-applied",
                        lang.tr("audit.policy.applied"),
                        applied,
                    ))
                    .child(row("audit-policy-exec", lang.tr("audit.policy.exec"), exec))
                    .child(row(
                        "audit-policy-allows",
                        lang.tr("audit.policy.allowsTitle"),
                        allows,
                    ))
                    .child(
                        // Two facts about the denial ring itself: how much of it
                        // this page shows, and that allows are counted rather than
                        // listed. Muted, because they qualify the numbers above
                        // rather than adding to them.
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .flex_wrap()
                            .child(txt(shown, 11.0, theme::faint()).line_height(px(16.0)))
                            .child(
                                txt(lang.tr("audit.policy.counted"), 11.0, theme::faint())
                                    .line_height(px(16.0)),
                            ),
                    ),
            )
    }

    /// The category tag the call rows carry.
    fn audit_category_tag(&self, category: audit::AuditCategory) -> Div {
        let ink = match category {
            audit::AuditCategory::FileRead => theme::success(),
            audit::AuditCategory::FileWrite => theme::warn(),
            audit::AuditCategory::Shell => theme::text(),
            audit::AuditCategory::Network => theme::accent(),
            // The browser owns the right pane, so it reads as a workspace
            // surface rather than as plain egress.
            audit::AuditCategory::Browser => theme::accent(),
            audit::AuditCategory::Mcp => theme::accent(),
            audit::AuditCategory::Subagent => theme::warn(),
            audit::AuditCategory::Other => theme::muted(),
        };
        div().w(px(64.0)).flex_none().flex().justify_end().child(
            div()
                .h(px(18.0))
                .px(px(5.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .bg(rgb(theme::hover()))
                .child(
                    txt(self.state.lang.tr(category.key()), 10.0, ink)
                        .font_weight(FontWeight::BOLD)
                        .line_height(px(15.0)),
                ),
        )
    }

    /// One turn: expandable, with its call count, refusals and measure.
    fn audit_turn_row(
        &self,
        view: &AuditView,
        turn: &AuditTurn,
        max_weight: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        #[cfg(test)]
        AUDIT_ROWS_BUILT.with(|built| built.set(built.get() + 1));
        let lang = self.state.lang;
        let open = self.audit_turn_open.contains(&turn.index);
        // The measure's ceiling is the frame's, passed in: asking the view per
        // row re-scanned every turn, which is quadratic over the page.
        let share = if max_weight <= 0.0 {
            0.0
        } else {
            (view.weight(turn, self.audit_mode) / max_weight).clamp(0.0, 1.0)
        };
        let calls = tr_args(
            lang,
            "audit.turnCalls",
            &[("count", turn.calls.len().to_string())],
        );
        let refusals = tr_args(
            lang,
            "audit.turnRefusals",
            &[("count", turn.refusals.len().to_string())],
        );
        let index = turn.index;
        let label = lang
            .tr("chat.railTurn")
            .replace("{turn}", &index.to_string());
        let stamp = turn
            .started_at
            .map(|at| crate::state::stamp(at / 1_000))
            .unwrap_or_default();
        let duration = audit::duration_text(turn.duration_ms);
        let row = div()
            .debug_selector(|| format!("audit-turn-{index}"))
            .id(SharedString::from(format!("audit-turn-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(Self::TURN_ROW_H))
            .px(px(10.0))
            .flex_none()
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(|row| row.bg(rgb(theme::hover())))
            .when(open, |row| row.bg(rgb(theme::selected_row())))
            .child(icon(
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                11.0,
                theme::faint(),
            ))
            .child(
                txt(label, 12.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(18.0)),
            )
            .when(!stamp.is_empty(), |row| {
                row.child(txt(stamp, 11.0, theme::faint()))
            })
            .child(txt(duration, 11.0, theme::muted()))
            .child(div().flex_1())
            .child(txt(calls, 11.0, theme::muted()))
            .when(!turn.refusals.is_empty(), |row| {
                row.child(txt(refusals, 11.0, theme::error()))
            })
            .child(
                // The measure bar is the toolbar's own control made visible:
                // a turn's duration relative to the slowest turn, an equal
                // share, or its call count. It is a per-turn FILL, not a lane
                // block; the trajectory strip's shared time-axis geometry lives
                // in `ui::lane` and is not duplicated here.
                div()
                    .w(px(Self::MEASURE_W))
                    .h(px(Self::MEASURE_H))
                    .flex_none()
                    .rounded(px(Self::MEASURE_H / 2.0))
                    .bg(rgb(theme::border_soft()))
                    .child(
                        div()
                            .debug_selector(|| format!("audit-measure-{index}"))
                            .h(px(Self::MEASURE_H))
                            .w(px(if share > 0.0 {
                                (share * Self::MEASURE_W).max(2.0)
                            } else {
                                0.0
                            }))
                            .rounded(px(Self::MEASURE_H / 2.0))
                            .bg(rgb(theme::accent())),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| format!("audit-jump-{index}"))
                    .id(SharedString::from(format!("audit-jump-{index}")))
                    .h(px(20.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .flex_none()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|cell| cell.bg(rgb(theme::selected_row())))
                    .child(icon(IconName::MessageSquare, 10.0, theme::muted()))
                    .child(txt(lang.tr("audit.jump"), 10.0, theme::muted()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.jump_to_audit_turn(index, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_audit_turn(index, cx);
            }));
        super::hairline_bottom(row)
            .border_color(rgb(theme::border_soft()))
            .into_any_element()
    }

    /// One call, with its preview and full text below when opened.
    fn audit_call_row(
        &self,
        turn: usize,
        index: usize,
        call: &AuditCall,
        policy: &AuditPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.audit_open.contains(&(turn, index));
        let full = self.audit_full.contains(&(turn, index));
        let ink = match call.outcome {
            AuditOutcome::Ok => theme::success(),
            AuditOutcome::Failed => theme::error(),
            AuditOutcome::Pending => theme::warn(),
        };
        let head = div()
            .debug_selector(|| format!("audit-call-{turn}-{index}"))
            .id(SharedString::from(format!("audit-call-{turn}-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(30.0))
            .ml(px(18.0))
            .pl(px(8.0))
            .pr(px(8.0))
            .flex_none()
            .cursor_pointer()
            .hover(|row| row.bg(rgb(theme::hover())))
            .when(open, |row| row.bg(rgb(theme::selected_row())))
            .child(icon(
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                10.0,
                theme::faint(),
            ))
            .child(self.audit_category_tag(call.category))
            .child(
                div()
                    .w(px(110.0))
                    .flex_none()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(theme::CODE_FONT)
                    .child(txt(call.tool.clone(), 12.0, theme::text()).line_height(px(18.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(theme::CODE_FONT)
                    .child(txt(call.target.clone(), 12.0, theme::muted()).line_height(px(18.0))),
            )
            // The row's summary of the confinement, when the call's own result
            // reported one: a compact "heel · deny-all". A record with neither
            // fact draws no chip - the row cannot imply a tier the stored call
            // never carried (the open detail says "no sandbox" instead).
            .when_some(self.audit_confinement_chip(call), |row, chip| {
                row.child(
                    div()
                        .flex_none()
                        .h(px(16.0))
                        .px(px(5.0))
                        .flex()
                        .items_center()
                        .rounded(px(4.0))
                        .bg(rgb(theme::hover()))
                        .child(
                            txt(chip, 10.0, theme::faint())
                                .font_family(theme::CODE_FONT)
                                .line_height(px(14.0)),
                        ),
                )
            })
            .child(
                div()
                    .w(px(56.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(txt(self.state.lang.tr(call.outcome.key()), 11.0, ink)),
            )
            .child(
                div()
                    .w(px(52.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(txt(
                        call.ms.map(|ms| format!("{ms} ms")).unwrap_or_default(),
                        11.0,
                        theme::faint(),
                    )),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_audit_call(turn, index, cx);
            }));
        let head = super::hairline_bottom(head).border_color(rgb(theme::border_soft()));
        let mut column = div().flex().flex_col();
        column = column.child(head);
        if open {
            let full_open = full;
            let detail = div()
                .debug_selector(|| format!("audit-preview-{turn}-{index}"))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .ml(px(94.0))
                .mr(px(8.0))
                .py(px(6.0))
                // The three facts the reader asked the audit to carry: the
                // permission in force, the policy that was applied, and what
                // the call actually did.
                .child({
                    let lang = self.state.lang;
                    let permission = tr_args(
                        lang,
                        "audit.policy.permissionField",
                        &[("permission", policy.permission.clone())],
                    );
                    let applied = tr_args(
                        lang,
                        "audit.policy.appliedField",
                        &[
                            ("evaluator", policy.evaluator.clone()),
                            ("preset", policy.preset_cell(lang)),
                        ],
                    );
                    let exec = tr_args(
                        lang,
                        "audit.policy.execField",
                        &[
                            ("outcome", lang.tr(call.outcome.key())),
                            (
                                "ms",
                                call.ms
                                    .map(|ms| format!("{ms} ms"))
                                    .unwrap_or_else(|| "\u{2014}".to_string()),
                            ),
                        ],
                    );
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .debug_selector(|| format!("audit-call-permission-{turn}-{index}"))
                                .child(txt(permission, 11.0, theme::muted())),
                        )
                        .child(
                            div()
                                .debug_selector(|| format!("audit-call-policy-{turn}-{index}"))
                                .child(txt(applied, 11.0, theme::faint())),
                        )
                        .child(
                            div()
                                .debug_selector(|| format!("audit-call-exec-{turn}-{index}"))
                                .child(txt(exec, 11.0, ink)),
                        )
                        // The PER-CALL confinement the tool's own result
                        // reported. Read from the stored record: a call whose
                        // output carries no sandbox field says so plainly rather
                        // than borrowing the session's permission.
                        .when_some(self.audit_confinement(call), |column, line| {
                            column.child(
                                div()
                                    .debug_selector(|| {
                                        format!("audit-call-confinement-{turn}-{index}")
                                    })
                                    .child(txt(line, 11.0, theme::faint())),
                            )
                        })
                        // The failure the sandbox's networking explains, in the
                        // reader's language. The stored record carries an id, so
                        // this line survives a language switch and never claims
                        // the policy refused a command it allowed.
                        .when_some(self.audit_diagnostic(call), |column, line| {
                            column.child(
                                div()
                                    .debug_selector(|| {
                                        format!("audit-call-diagnostic-{turn}-{index}")
                                    })
                                    .child(txt(line, 11.0, theme::warn())),
                            )
                        })
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(6.0))
                        .child(txt(
                            self.state.lang.tr("audit.preview"),
                            10.0,
                            theme::faint(),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(theme::CODE_FONT)
                                .child(
                                    txt(call.preview.clone(), 11.0, theme::text())
                                        .line_height(px(16.0)),
                                ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| format!("audit-show-full-{turn}-{index}"))
                        .id(SharedString::from(format!(
                            "audit-show-full-{turn}-{index}"
                        )))
                        .cursor_pointer()
                        .child(txt(
                            self.state.lang.tr(if full_open {
                                "audit.hideFull"
                            } else {
                                "audit.showFull"
                            }),
                            11.0,
                            theme::accent(),
                        ))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_audit_full(turn, index, cx);
                        })),
                );
            let detail = if full_open {
                let body = self
                    .state
                    .active_session()
                    .map(|session| audit::full_text(session, call))
                    .unwrap_or_default();
                detail.child(
                    div()
                        .debug_selector(|| format!("audit-full-{turn}-{index}"))
                        .id(SharedString::from(format!("audit-full-{turn}-{index}")))
                        .max_h(px(240.0))
                        .overflow_y_scroll()
                        .p(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::new_session_fill()))
                        .font_family(theme::CODE_FONT)
                        .child(txt(body, 11.0, theme::text()).line_height(px(16.0))),
                )
            } else {
                detail
            };
            column = column.child(detail);
        }
        column.into_any_element()
    }

    /// One refused operation, stating that it did NOT happen.
    fn audit_refusal_row(
        &self,
        turn: &AuditTurn,
        index: usize,
        refusal: &AuditRefusal,
    ) -> AnyElement {
        let lang = self.state.lang;
        let rule = refusal
            .rule
            .as_deref()
            .map(|rule| tr_args(lang, "audit.rule", &[("rule", rule.to_string())]))
            .unwrap_or_else(|| lang.tr("audit.ruleUnnamed"));
        let permission = tr_args(
            lang,
            "audit.permission",
            &[("permission", refusal.permission.clone())],
        );
        let action = tr_args(
            lang,
            "audit.action.label",
            &[("action", lang.tr(audit::action_key(&refusal.action)))],
        );
        let row = div()
            .debug_selector(|| format!("audit-refusal-{}-{index}", turn.index))
            .id(SharedString::from(format!(
                "audit-refusal-{}-{index}",
                turn.index
            )))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .ml(px(18.0))
            .px(px(8.0))
            .py(px(5.0))
            .flex_none()
            .bg(rgb(theme::page()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .h(px(18.0))
                            .px(px(5.0))
                            .flex()
                            .items_center()
                            .rounded(px(4.0))
                            .bg(rgb(theme::hover()))
                            .child(
                                txt(lang.tr("audit.refused"), 10.0, theme::error())
                                    .font_weight(FontWeight::BOLD)
                                    .line_height(px(15.0)),
                            ),
                    )
                    .child(div().flex_none().font_family(theme::CODE_FONT).child(txt(
                        rule,
                        11.0,
                        theme::error(),
                    )))
                    .child(txt(action, 10.0, theme::faint()))
                    .child(div().flex_none().font_family(theme::CODE_FONT).child(txt(
                        refusal.tool.clone(),
                        11.0,
                        theme::text(),
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(theme::CODE_FONT)
                            .child(txt(refusal.target.clone(), 11.0, theme::muted())),
                    )
                    .child(txt(permission, 10.0, theme::faint())),
            )
            .child(txt(lang.tr("audit.refusedNote"), 11.0, theme::error()).line_height(px(15.0)))
            .when_some(refusal.reason.clone(), |row, reason| {
                row.child(txt(reason, 11.0, theme::faint()).line_height(px(15.0)))
            });
        super::hairline_bottom(row)
            .border_color(rgb(theme::border_soft()))
            .into_any_element()
    }

    /// What confinement the call's own result reported, as one line.
    ///
    /// Both facts come from the STORED result, never from a session-wide
    /// assumption: the sandbox backend and the network reach the tool itself
    /// recorded for THAT call. A call with no sandbox field - full access, a
    /// refusal that produced no result, or a record written before the field
    /// existed - says "no sandbox" rather than implying a confinement it never
    /// had. The network half is drawn only when the record carries it, so the
    /// two facts stay independent.
    pub(crate) fn audit_confinement(&self, call: &AuditCall) -> Option<String> {
        let lang = self.state.lang;
        let mut sandbox = match call.sandbox.as_deref() {
            Some(sandbox) => tr_args(
                lang,
                "audit.call.sandboxField",
                &[("sandbox", sandbox.to_string())],
            ),
            None => lang.tr("audit.call.noSandbox"),
        };
        // `none` alone does not say who released confinement. The bash result
        // always stores a reason; the three known ones are localized through
        // ONE mapping (the record, the page and the export agree), and an
        // id/text from a newer build is printed rather than guessed at.
        if let Some(reason) = call.sandbox_reason.as_deref() {
            let reason = match call.sandbox_reason_key() {
                Some(key) => lang.tr(key),
                None => reason.to_string(),
            };
            sandbox = format!("{sandbox} ({reason})");
        }
        match call.network.as_deref() {
            Some(network) => {
                // `unconfined` is prose, not a policy state: no sandbox proxy
                // existed, so the workspace allowlist was never consulted. It
                // resolves through the audit's ONE mapping, the same one the
                // Markdown export uses.
                let network = match call.network_key() {
                    Some(key) => lang.tr(key),
                    None => network.to_string(),
                };
                let network = tr_args(lang, "audit.call.networkField", &[("network", network)]);
                Some(format!("{sandbox} \u{00b7} {network}"))
            }
            None => Some(sandbox),
        }
    }

    /// The reader-facing explanation for a failure the SANDBOX's networking
    /// caused, or none when the record carried no diagnostic this build knows.
    ///
    /// The text lives in the locale table (one key shared with the transcript
    /// step body), so the same record reads in English or Chinese and switching
    /// languages does not require a new tool call. It says the kernel refused a
    /// direct socket/ICMP path the sandbox does not proxy, names the clients that
    /// do work, and points at the policy document - it never says the policy
    /// refused a command the policy allowed.
    pub(crate) fn audit_diagnostic(&self, call: &AuditCall) -> Option<String> {
        call.diagnostic_key().map(|key| self.state.lang.tr(key))
    }

    /// The row's compact confinement chip, or none when the record carries
    /// neither fact.
    ///
    /// The row is a summary, so it draws the RAW recorded values ("heel",
    /// "deny-all") rather than the labelled field form the open detail uses.
    /// None means nothing was reported, and the row then draws no chip at all -
    /// a summary must not imply a tier the call never had.
    pub(crate) fn audit_confinement_chip(&self, call: &AuditCall) -> Option<String> {
        match (call.sandbox.as_deref(), call.network.as_deref()) {
            (Some(sandbox), Some(network)) => Some(format!("{sandbox} \u{00b7} {network}")),
            (Some(sandbox), None) => Some(sandbox.to_string()),
            (None, Some(network)) => Some(network.to_string()),
            (None, None) => None,
        }
    }

    /// The whole tab.
    pub(crate) fn render_audit(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(_session) = self.state.active_session() else {
            return div()
                .debug_selector(|| "audit".to_string())
                .id("audit")
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(txt(
                    self.state.lang.tr("empty.none"),
                    theme::body_size_at(self.state.draft().global.general.content_font_size),
                    theme::muted(),
                ))
                .into_any_element();
        };
        let view = self.audit_view();
        let query = self.audit_query.clone();
        let category = self.audit_category;
        let filtering = !query.trim().is_empty();
        // One measure ceiling for the frame: a row's share is relative to the
        // largest turn, and asking the view per row re-scanned every turn.
        let max_weight = view.max_weight(self.audit_mode);
        // The page is windowed only while every turn is collapsed and nothing is
        // searched: each row is then exactly TURN_ROW_H tall and two spacers
        // stand in for the rows outside the viewport. An opened turn (or a
        // search, which opens every turn) grows rows with their calls, whose
        // heights belong to the content, so that frame builds them all.
        let collapsed = !filtering && self.audit_turn_open.is_empty() && self.audit_open.is_empty();
        let mut rows: Vec<AnyElement> = if collapsed {
            self.audit_window(&view, category, max_weight, cx)
        } else {
            self.audit_rows(&view, &query, filtering, category, max_weight, cx)
        };
        if rows.is_empty() {
            rows.push(
                div()
                    .flex_1()
                    .min_h(px(120.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(txt(self.state.lang.tr("audit.empty"), 12.0, theme::muted()))
                    .into_any_element(),
            );
        }
        div()
            .debug_selector(|| "audit".to_string())
            .id("audit")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.audit_toolbar(cx))
            .child(self.audit_summary(&view))
            .child(self.audit_policy_strip(&view))
            .child(
                div()
                    .debug_selector(|| "audit-list".to_string())
                    .id("audit-list")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.audit_scroll)
                    .py(px(4.0))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .into_any_element()
    }

    /// The collapsed page's rows: the band around the viewport is built, and a
    /// spacer holding every other row's exact height stands in for it, so the
    /// list's total height - and its thumb - is the whole page's.
    fn audit_window(
        &self,
        view: &AuditView,
        category: Option<AuditCategory>,
        max_weight: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let turns: Vec<&AuditTurn> = view
            .turns
            .iter()
            .filter(|turn| {
                category
                    .is_none_or(|category| turn.calls.iter().any(|call| call.category == category))
            })
            .collect();
        let row_h = Self::TURN_ROW_H;
        let view_h = f32::from(self.audit_scroll.bounds().size.height);
        let view_h = if view_h > 0.0 {
            view_h
        } else {
            AUDIT_FIRST_VIEWPORT
        };
        let view_top = -f32::from(self.audit_scroll.offset().y);
        let (first, end, leading, trailing) = audit_band(turns.len(), row_h, view_top, view_h);
        let mut rows: Vec<AnyElement> = Vec::with_capacity(2 + end.saturating_sub(first));
        if leading > 0.0 {
            rows.push(audit_spacer(leading));
        }
        for turn in &turns[first..end] {
            rows.push(self.audit_turn_row(view, turn, max_weight, cx));
        }
        if trailing > 0.0 {
            rows.push(audit_spacer(trailing));
        }
        rows
    }

    /// The rows of the expanded or searched page, one element per turn and per
    /// call. These rows take their height from their content, so they are built
    /// in full; that is the mode the reader asked for, not the page they land on.
    fn audit_rows(
        &self,
        view: &AuditView,
        query: &str,
        filtering: bool,
        category: Option<AuditCategory>,
        max_weight: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = Vec::new();
        let matching_turns: Vec<&AuditTurn> = view
            .turns
            .iter()
            .filter(|turn| {
                let category_has_calls = category
                    .is_none_or(|category| turn.calls.iter().any(|call| call.category == category));
                if !category_has_calls {
                    return false;
                }
                !filtering
                    || turn.calls.iter().any(|call| {
                        category.is_none_or(|category| call.category == category)
                            && audit::matches_call(call, query)
                    })
                    || (category.is_none()
                        && turn
                            .refusals
                            .iter()
                            .any(|refusal| audit::matches_refusal(refusal, query)))
            })
            .collect();
        let pages = matching_turns.len().div_ceil(AUDIT_TURNS_PER_PAGE).max(1);
        let page = self.audit_page.min(pages - 1);
        let first_turn = page * AUDIT_TURNS_PER_PAGE;
        for turn in matching_turns
            .iter()
            .skip(first_turn)
            .take(AUDIT_TURNS_PER_PAGE)
        {
            let matching_calls = turn
                .calls
                .iter()
                .enumerate()
                .filter(|(_, call)| category.is_none_or(|category| call.category == category))
                .filter(|(_, call)| audit::matches_call(call, query));
            let matching_count = matching_calls.clone().count();
            let limit = self
                .audit_call_limits
                .get(&turn.index)
                .copied()
                .unwrap_or(AUDIT_CALLS_PER_TURN);
            let calls: Vec<(usize, &AuditCall)> = matching_calls.take(limit).collect();
            let remaining = matching_count.saturating_sub(calls.len());
            // Category selection and search are applied before building rows,
            // so a filtered turn cannot still expand into unrelated calls.
            rows.push(self.audit_turn_row(view, turn, max_weight, cx));
            // A search shows the matching calls even in a collapsed turn, so a
            // hit is never hidden behind the turn's own fold.
            let expanded = filtering || self.audit_turn_open.contains(&turn.index);
            if !expanded {
                continue;
            }
            if turn.calls.is_empty() && turn.refusals.is_empty() {
                rows.push(
                    div()
                        .ml(px(94.0))
                        .py(px(4.0))
                        .child(txt(
                            self.state.lang.tr("audit.noCalls"),
                            11.0,
                            theme::faint(),
                        ))
                        .into_any_element(),
                );
            }
            for (index, call) in &calls {
                rows.push(self.audit_call_row(turn.index, *index, call, &view.policy, cx));
            }
            if remaining > 0 {
                let turn_index = turn.index;
                let label = crate::i18n::tr_args(
                    self.state.lang,
                    "audit.showMoreCalls",
                    &[("count", remaining.to_string())],
                );
                rows.push(
                    div()
                        .id(SharedString::from(format!(
                            "audit-more-calls-{}",
                            turn_index
                        )))
                        .ml(px(94.0))
                        .py(px(6.0))
                        .cursor_pointer()
                        .child(txt(label, 11.0, theme::accent()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            let shown = this
                                .audit_call_limits
                                .entry(turn_index)
                                .or_insert(AUDIT_CALLS_PER_TURN);
                            *shown = shown.saturating_add(AUDIT_CALLS_PER_TURN);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            for (index, refusal) in
                turn.refusals.iter().enumerate().filter(|(_, refusal)| {
                    category.is_none() && audit::matches_refusal(refusal, query)
                })
            {
                rows.push(self.audit_refusal_row(turn, index, refusal));
            }
        }
        if pages > 1 {
            let page_label = crate::i18n::tr_args(
                self.state.lang,
                "audit.page",
                &[
                    ("page", (page + 1).to_string()),
                    ("pages", pages.to_string()),
                ],
            );
            let last_page = pages - 1;
            rows.push(
                div()
                    .id("audit-page-controls")
                    .mt(px(8.0))
                    .pb(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("audit-page-prev")
                            .cursor_pointer()
                            .child(txt("‹", 16.0, theme::accent()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.audit_page = this.audit_page.saturating_sub(1);
                                this.audit_scroll.set_offset(gpui_kit::point(
                                    gpui_kit::px(0.0),
                                    gpui_kit::px(0.0),
                                ));
                                cx.notify();
                            })),
                    )
                    .child(txt(page_label, 11.0, theme::muted()))
                    .child(
                        div()
                            .id("audit-page-next")
                            .cursor_pointer()
                            .child(txt("›", 16.0, theme::accent()))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.audit_page = (this.audit_page + 1).min(last_page);
                                this.audit_scroll.set_offset(gpui_kit::point(
                                    gpui_kit::px(0.0),
                                    gpui_kit::px(0.0),
                                ));
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows
    }
}

/// The list's own vertical padding: the window's arithmetic and the list's
/// padding must be the same number.
const AUDIT_LIST_PAD_Y: f32 = 4.0;
/// Rows this close to the viewport are built too, so a small scroll never
/// reveals an empty frame.
const AUDIT_OVERDRAW: f32 = 480.0;
/// What the first frame after the tab mounts assumes it can show, before the
/// scroller has reported its own height.
const AUDIT_FIRST_VIEWPORT: f32 = 800.0;
/// Expanded/search audit rendering stays bounded to a short turn page.
const AUDIT_TURNS_PER_PAGE: usize = 20;
/// Calls in each expanded turn are revealed in bounded increments.
const AUDIT_CALLS_PER_TURN: usize = 50;

/// The band of the collapsed page, and the exact height of the rows on either
/// side of it.
///
/// Pure so the geometry the spacers stand in at can be pinned without a frame.
/// `rows` is the number of turn rows, each exactly `row_h` tall.
fn audit_band(rows: usize, row_h: f32, view_top: f32, view_h: f32) -> (usize, usize, f32, f32) {
    let band_top = view_top - AUDIT_OVERDRAW;
    let band_bottom = view_top + view_h + AUDIT_OVERDRAW;
    let mut top = AUDIT_LIST_PAD_Y;
    let mut first: Option<usize> = None;
    let mut end = 0usize;
    for index in 0..rows {
        if first.is_none() && top + row_h > band_top {
            first = Some(index);
        }
        if first.is_some() && top < band_bottom {
            end = index + 1;
        }
        top += row_h;
    }
    let first = first.unwrap_or(rows);
    // A view past the end (the page just shrank under a scrolled reader) has no
    // row in its band: give the leading spacer every row so the list keeps its
    // height, and the layout pass clamps the offset.
    let end = end.max(first);
    (
        first,
        end,
        first as f32 * row_h,
        (rows - end) as f32 * row_h,
    )
}

/// A row kept at its exact height without building its contents.
fn audit_spacer(height: f32) -> AnyElement {
    div()
        .flex_none()
        .flex_shrink_0()
        .w_full()
        .h(px(height.max(0.0)))
        .into_any_element()
}

// How many turn rows the last frame actually built, for the performance test.
#[cfg(test)]
thread_local! {
    static AUDIT_ROWS_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The audit turn rows built so far on this thread.
#[cfg(test)]
pub(crate) fn audit_rows_built() -> usize {
    AUDIT_ROWS_BUILT.with(|built| built.get())
}

#[cfg(test)]
mod tests {
    // Named imports: a glob would pull in gpui's re-export of the test attribute
    // macro, which shadows the builtin one its own expansion emits.
    use crate::model::{ChatMessage, ToolEvent};
    use crate::state::{AppState, Tab};
    use crate::store::Store;
    use crate::ui::shell::Shell;
    use gpui_kit::{
        AppContext as _, Context as UiContext, Entity, IntoElement, Render, TestAppContext,
        VisualTestContext, Window,
    };

    /// The page's rows are all the same height, so the window's band and the two
    /// spacers must reproduce the whole page's height exactly.
    #[test]
    fn the_window_band_and_spacers_reproduce_the_whole_page() {
        let row = 38.0f32;
        for (view_top, view_h, first, end) in [
            (0.0f32, 300.0f32, 0usize, 21usize),
            (1900.0, 300.0, 37, 71),
            (3500.0, 300.0, 79, 100),
        ] {
            let (got_first, got_end, leading, trailing) =
                super::audit_band(100, row, view_top, view_h);
            assert_eq!(
                (got_first, got_end),
                (first, end),
                "the band for a view at {view_top}"
            );
            assert_eq!(leading, first as f32 * row);
            assert_eq!(trailing, (100 - end) as f32 * row);
            let built = (got_end - got_first) as f32 * row;
            assert_eq!(
                leading + built + trailing,
                3800.0,
                "the spacers keep the whole page's height"
            );
        }
        // A view past the end (or above the top) keeps the page's height with
        // one spacer and builds nothing.
        assert_eq!(
            super::audit_band(100, row, 10_000.0, 300.0),
            (100, 100, 3800.0, 0.0)
        );
        assert_eq!(
            super::audit_band(100, row, -10_000.0, 300.0),
            (0, 0, 0.0, 3800.0)
        );
        assert_eq!(super::audit_band(0, row, 0.0, 300.0), (0, 0, 0.0, 0.0));
    }

    /// Holds the shell so the window can render an entity the test also keeps.
    struct Harness {
        shell: Entity<Shell>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut UiContext<Self>) -> impl IntoElement {
            self.shell.clone()
        }
    }

    /// Boot the kit and bind bsh's own keys, the way `ui::run` does.
    fn init_test_app(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        cx.update(crate::ui::bind_global_actions);
    }

    /// A store rooted in the build directory: a test never touches the home.
    fn scratch_store(label: &str) -> Store {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-audit-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open_at(dir)
    }

    /// A session of `turns` turns, each with one call whose result is the ~2 KB
    /// JSON the measurements used.
    fn long_session(label: &str, turns: usize) -> AppState {
        let mut state = AppState::with_store(scratch_store(label));
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            for turn in 0..turns {
                session
                    .messages
                    .push(ChatMessage::user(format!("turn {turn}")));
                session.messages.push(ChatMessage::assistant());
                let call = ToolEvent {
                    name: "bash".to_string(),
                    args: r#"{"command":"cargo test -p bsh"}"#.to_string(),
                    output: Some(format!(
                        r#"{{"stdout":"{}","stderr":"","sandbox":"seatbelt","sandbox_reason":"workspace","network":"direct","network_diagnostic":"ok"}}"#,
                        "log line ".repeat(200)
                    )),
                    ms: Some(40),
                    at: Some(1_700_000_000_000 + turn as u64),
                };
                session
                    .messages
                    .last_mut()
                    .expect("the assistant row just pushed")
                    .tools
                    .push(call);
            }
        }
        state
    }

    /// Boot the Audit tab over a session of `turns` turns.
    fn boot<'a>(
        cx: &'a mut TestAppContext,
        label: &'static str,
        turns: usize,
    ) -> (Entity<Shell>, &'a mut VisualTestContext) {
        init_test_app(cx);
        let (harness, cx) = cx.add_window_view(move |window, cx| {
            let state = long_session(label, turns);
            let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
            Harness { shell }
        });
        let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
        // The tab is set on the SHELL: `assemble` restores the session's own tab,
        // so a value set on the state handed to the builder is lost.
        cx.update(|_, cx| {
            shell.update(cx, |this, cx| {
                this.state.tab = Tab::Audit;
                cx.notify();
            })
        });
        (shell, cx)
    }

    /// Draw `count` frames and return their median milliseconds.
    fn median_frame(cx: &mut VisualTestContext, count: usize) -> f64 {
        let mut frames: Vec<f64> = Vec::new();
        for _ in 0..count {
            let started = std::time::Instant::now();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            frames.push(started.elapsed().as_secs_f64() * 1e3);
        }
        frames.sort_by(|a, b| a.partial_cmp(b).expect("finite frame times"));
        frames[frames.len() / 2]
    }

    /// A long audit page paints a window, and its view model survives a repaint.
    ///
    /// MEASURED on a debug build before the fix: 242 ms a frame at the restored
    /// 256-message window and 1 971 ms at 2 000, 88% of it rebuilding the view
    /// model (95% of THAT the secret scan).
    #[gpui::test]
    fn a_long_audit_page_paints_a_bounded_window(cx: &mut TestAppContext) {
        let (_, cx) = boot(cx, "window-small", 4);
        for _ in 0..4 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let small_ms = median_frame(cx, 12);

        let (shell, cx) = boot(cx, "window-long", 400);
        for _ in 0..4 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let builds_before = crate::audit::audit_builds();
        let rows_before = super::audit_rows_built();
        let long_ms = median_frame(cx, 12);
        let built = (super::audit_rows_built() - rows_before) as f64 / 12.0;
        eprintln!("AUDIT WINDOW small={small_ms:.2}ms long={long_ms:.2}ms built={built:.1}/frame");
        assert!(
            built < 80.0,
            "a 400-turn page built {built:.0} rows a frame: the window is not bounded"
        );
        assert_eq!(
            crate::audit::audit_builds(),
            builds_before,
            "a repaint that changes nothing must reuse the view model"
        );
        assert!(
            long_ms <= small_ms * 3.0 + 20.0,
            "a 400-turn page painted {long_ms:.2}ms a frame against {small_ms:.2}ms for four"
        );

        // The rows keep the exact 38px stride the spacers stand in at.
        let second = cx
            .debug_bounds("audit-turn-2")
            .expect("the second row is built");
        let first = cx
            .debug_bounds("audit-turn-1")
            .expect("the first row is built");
        let stride = f32::from(second.origin.y) - f32::from(first.origin.y);
        assert!(
            (stride - Shell::TURN_ROW_H).abs() < 0.5,
            "the rows keep their 38px stride: {stride:.1}px"
        );

        // The tail is reachable, and the top of the page is a spacer there.
        let turns = cx.update(|_, cx| shell.read(cx).audit_view().turns.len());
        let last_offset = super::AUDIT_LIST_PAD_Y + (turns - 1) as f32 * Shell::TURN_ROW_H;
        cx.update(|_, cx| {
            shell.update(cx, |this, _| {
                this.audit_scroll.set_offset(gpui_kit::point(
                    gpui_kit::px(0.0),
                    gpui_kit::px(-last_offset),
                ));
            })
        });
        for _ in 0..3 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let last: &'static str = Box::leak(format!("audit-turn-{turns}").into_boxed_str());
        assert!(
            cx.debug_bounds(last).is_some(),
            "the last turn is built at the tail"
        );
        assert!(
            cx.debug_bounds("audit-turn-1").is_none(),
            "the top of the page is a spacer while the reader is at the tail"
        );
        assert_eq!(
            crate::audit::audit_builds(),
            builds_before,
            "scrolling must not rebuild the view model"
        );
    }
}
