//! The Audit tab.
//!
//! This is the deterministic view over the persisted tool events and the
//! policy denial ring: one row per turn, expandable to the calls it made and the
//! refusals it met. It does not call a model and generates no prose - every
//! figure comes from a count over the records.
//!
//! What it cannot see is stated on the page itself: a shell command's output is
//! recorded but the files it creates are not individually audited, and the only
//! network the audit can observe is tool/MCP egress - a shell's own sockets are
//! not intercepted. Those are limits of the data, not of this view.
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
    self, AuditAllows, AuditCall, AuditOutcome, AuditPolicy, AuditRefusal, AuditTurn, AuditView,
};
use crate::i18n::tr_args;
use crate::state::Tab;
use crate::theme;

impl Shell {
    /// The audit view for the active session, built from persisted records.
    ///
    /// The denial snapshot is the policy pane's own source of truth, so the two
    /// surfaces cannot disagree about what was refused. The live MCP namespaces
    /// let a namespaced tool classify as MCP rather than Other.
    pub(crate) fn audit_view(&self) -> AuditView {
        let Some(session) = self.state.active_session() else {
            return AuditView::default();
        };
        let denials = self
            .state
            .policy_audit
            .as_ref()
            .map(|audit| audit.denials.as_slice())
            .unwrap_or_default();
        let mut mcp: Vec<String> = self
            .state
            .mcp
            .tools()
            .map(|tool| tool.namespace.clone())
            .collect();
        mcp.sort();
        mcp.dedup();
        AuditView::build(session, denials, &mcp).with_policy(self.audit_policy())
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
            policy.read_scope = data.preset_read_roots.len();
            policy.write_scope = data.preset_write_roots.len();
            policy.deny_roots = data.preset_deny_roots.len();
            policy.egress_allowlist = data.egress_allowlist.len();
            policy.egress_restricted = data.preset_egress_restricted;
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
        let view = self.audit_view();
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
    /// The turn row's own height.
    const TURN_ROW_H: f32 = 38.0;
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
                (
                    "preset",
                    if policy.preset.is_empty() {
                        lang.tr("audit.policy.noPreset")
                    } else {
                        policy.preset.clone()
                    },
                ),
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let open = self.audit_turn_open.contains(&turn.index);
        let share = view.share(turn, self.audit_mode);
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
                            (
                                "preset",
                                if policy.preset.is_empty() {
                                    lang.tr("audit.policy.noPreset")
                                } else {
                                    policy.preset.clone()
                                },
                            ),
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

    /// The page's own statement of what it cannot see.
    fn audit_limits(&self) -> Div {
        let lang = self.state.lang;
        div()
            .debug_selector(|| "audit-limits".to_string())
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(8.0))
            .py(px(6.0))
            .flex_none()
            .border_t_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::new_session_fill()))
            .child(txt(lang.tr("audit.limits.shell"), 11.0, theme::faint()))
            .child(txt(lang.tr("audit.limits.network"), 11.0, theme::faint()))
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
        let filtering = !query.trim().is_empty();
        let mut rows: Vec<AnyElement> = Vec::new();
        for turn in &view.turns {
            if !audit::matches_turn(turn, &query) {
                continue;
            }
            rows.push(self.audit_turn_row(&view, turn, cx));
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
            for (index, call) in turn.calls.iter().enumerate() {
                if !audit::matches_call(call, &query) {
                    continue;
                }
                rows.push(self.audit_call_row(turn.index, index, call, &view.policy, cx));
            }
            for (index, refusal) in turn.refusals.iter().enumerate() {
                if !audit::matches_refusal(refusal, &query) {
                    continue;
                }
                rows.push(self.audit_refusal_row(turn, index, refusal));
            }
        }
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
            .child(self.audit_limits())
            .into_any_element()
    }
}
