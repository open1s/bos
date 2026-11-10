//! The Review tab: exact changes, structural nodes and deterministic findings.
//!
//! The tab reads a prepared [crate::review::domain::ReviewSnapshot] from the
//! shell cache. A diff is built only when its change is clicked, so a repaint
//! never reads a blob or parses a file.

use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::*;

use super::shell::Shell;
use super::{icon, txt, DivExt, IconName};
use crate::i18n::{tr, tr_args};
use crate::review::domain::{
    CaptureMode, Change, ChangeKind, ChangeSet, Decision, FileDiff, ReviewSnapshot, ReviewStatus,
    RiskLevel, Severity,
};
use crate::theme;

/// The width a diff needs before the change list starts giving its own width
/// back. A diff is a row of gutter, mark and code, so it is the column that has
/// to stay legible; the list can shorten its paths instead.
const REVIEW_DIFF_MIN: f32 = 360.0;
/// The pane's own horizontal chrome around the two columns: 18 a side plus the
/// 12 between them.
const REVIEW_PANE_CHROME: f32 = 48.0;
/// The narrowest the change list goes before the diff takes the squeeze.
const REVIEW_LIST_MIN: f32 = 210.0;

/// The Review tab's cache key.
#[derive(Clone, PartialEq, Eq)]
struct ReviewKey {
    session: String,
    requested: Option<String>,
    epoch: u64,
}

/// The latest snapshot, kept until the session or review epoch changes.
#[derive(Default)]
pub(crate) struct ReviewCache {
    key: Option<ReviewKey>,
    snapshot: Option<ReviewSnapshot>,
}

impl Shell {
    /// The active session's newest review, read on a cache miss only.
    pub(crate) fn review_view(&self) -> Option<ReviewSnapshot> {
        let session = self.state.active_id.clone()?;
        let key = ReviewKey {
            session: session.clone(),
            requested: self.review_requested.clone(),
            epoch: self.state.review_epoch,
        };
        let stale = {
            let cache = self.review_cache.borrow();
            cache.key.as_ref() != Some(&key)
        };
        if stale {
            let snapshot = match self.review_requested.as_deref() {
                Some(change_set) => self.state.review_snapshot_for(change_set),
                None => self.state.review_snapshot(&session),
            };
            let mut cache = self.review_cache.borrow_mut();
            cache.snapshot = snapshot;
            cache.key = Some(key);
        }
        self.review_cache.borrow().snapshot.clone()
    }

    /// Render the Review tab.
    pub(crate) fn render_review(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let Some(snapshot) = self.review_view() else {
            return self.review_empty(
                if self.state.active_id.is_some() {
                    "review.empty.title"
                } else {
                    "review.noSession"
                },
                if self.state.active_id.is_some() {
                    Some("review.empty.hint")
                } else {
                    None
                },
                cx,
            );
        };
        let changeset = &snapshot.changeset;
        let review = &snapshot.review;
        let selected = self
            .review_selected
            .as_ref()
            .filter(|id| changeset.changes.iter().any(|change| &change.id == *id))
            .cloned();
        let selected_change = selected
            .as_ref()
            .and_then(|id| changeset.changes.iter().find(|change| &change.id == id));
        let diff = selected.as_ref().and_then(|id| {
            let guard = self.review_diff.borrow();
            guard
                .as_ref()
                .filter(|(cached, _)| cached == id)
                .map(|(_, diff)| diff.clone())
        });
        let counts = tr_args(
            lang,
            "review.counts",
            &[
                ("files", changeset.changes.len().to_string()),
                ("changes", changeset.revision_count().to_string()),
            ],
        );
        let stale = review.status == ReviewStatus::Stale;

        let actions = div()
            .debug_selector(|| "review-actions".to_string())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                Button::new("review-open-audit")
                    .ghost()
                    .h(px(30.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .child(txt(tr(lang, "review.openAudit"), 11.5, theme::text()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.tab = crate::state::Tab::Audit;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("review-request-changes")
                    .ghost()
                    .h(px(30.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .disabled(stale)
                    .child(txt(tr(lang, "review.requestChanges"), 11.5, theme::text()))
                    .on_click(cx.listener({
                        let change_set = changeset.id.clone();
                        move |this, _, _, cx| {
                            this.decide_review(&change_set, Decision::RequestChanges, cx);
                        }
                    })),
            )
            .child(
                Button::new("review-approve")
                    .h(px(30.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::accent()))
                    .disabled(stale)
                    .child(txt(tr(lang, "review.approve"), 11.5, 0xFFFFFF))
                    .on_click(cx.listener({
                        let change_set = changeset.id.clone();
                        move |this, _, _, cx| {
                            this.decide_review(&change_set, Decision::Approve, cx);
                        }
                    })),
            );

        let header = div()
            .flex()
            .flex_row()
            // A narrowed pane drops the decision buttons under the title rather
            // than letting them paint past the right edge.
            .flex_wrap()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .min_w_0()
                    .child(txt(tr(lang, "review.changes"), 16.0, theme::text()))
                    .child(txt(counts, 11.5, theme::faint()))
                    .when_some(scope_line(changeset, lang), |column, scope| {
                        column.child(txt(scope, 10.5, theme::faint()))
                    })
                    .child(
                        div()
                            .flex()
                            // The chips and the digest wrap before they run off
                            // the pane; the digest is the last thing to give.
                            .flex_wrap()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(status_chip(lang, review.status))
                            .child(risk_chip(lang, review.risk.level))
                            .child(capture_chip(lang, changeset.capture))
                            .when(stale, |row| {
                                row.child(status_chip(lang, ReviewStatus::Stale))
                            })
                            .child(txt(
                                format!(
                                    "{} {}",
                                    tr(lang, "review.digest"),
                                    short_digest(changeset.digest.as_deref())
                                ),
                                10.5,
                                theme::faint(),
                            )),
                    ),
            )
            .child(actions);

        // The list and the diff share the pane. The list keeps its designed 330
        // while the diff still gets the width a diff needs to be read; past
        // that the list is what gives, down to its own floor. An expanded right
        // panel used to leave the diff with the leftover - 151px at a half-width
        // column - because the list held its 330.
        let changes_w = if self.conversation_w > px(0.0) {
            let pane = self.conversation_w.as_f32();
            (pane - REVIEW_DIFF_MIN - REVIEW_PANE_CHROME).clamp(REVIEW_LIST_MIN, 330.0)
        } else {
            330.0
        };
        let mut changes = div()
            .debug_selector(|| "review-change-list".to_string())
            .id("review-change-list")
            .w(px(changes_w))
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .overflow_y_scroll()
            .border_r_1()
            .border_color(rgb(theme::border_soft()))
            .pr(px(10.0));
        for change in &changeset.changes {
            let active = selected.as_deref() == Some(change.id.as_str());
            let change_clone = change.clone();
            let mut tools: Vec<&str> = Vec::new();
            for revision in &change.revisions {
                if let Some(tool) = &revision.tool {
                    if !tools.contains(&tool.name.as_str()) {
                        tools.push(tool.name.as_str());
                    }
                }
            }
            let tool_label = tools.first().map(|tool| {
                if tools.len() > 1 {
                    format!("{tool} +{}", tools.len() - 1)
                } else {
                    (*tool).to_string()
                }
            });
            changes = changes.child(
                div()
                    .id(format!("review-change-{}", change.id))
                    .debug_selector(|| "review-change-row".to_string())
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(8.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .when(active, |row| row.bg(rgb(theme::selected_row())))
                    .hover(|row| row.bg(rgb(theme::hover())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let diff = this.state.review_diff(&change_clone);
                        this.review_selected = Some(change_clone.id.clone());
                        *this.review_diff.borrow_mut() = Some((change_clone.id.clone(), diff));
                        cx.notify();
                    }))
                    .child(
                        txt(change.path.workspace_relative.clone(), 11.5, theme::text())
                            .whitespace_nowrap()
                            .text_ellipsis(),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.0))
                            .child(txt(change_kind(lang, &change.kind), 10.0, theme::faint()))
                            .when_some(tool_label, |row, tool| {
                                row.child(txt(tool, 10.0, theme::faint()).min_w_0().text_ellipsis())
                            })
                            .child(txt(
                                format!("{} rev", change.revisions.len()),
                                10.0,
                                theme::faint(),
                            )),
                    )
                    .when(matches!(&change.kind, ChangeKind::Renamed { .. }), |row| {
                        let from = match &change.kind {
                            ChangeKind::Renamed { from } => from.workspace_relative.clone(),
                            _ => String::new(),
                        };
                        row.child(txt(
                            format!("{from} → {}", change.path.workspace_relative),
                            10.0,
                            theme::faint(),
                        ))
                    }),
            );
        }

        let findings = findings_section(&snapshot, lang, cx);
        let detail = self.review_detail(selected_change, diff.as_ref(), lang, cx);
        let body = div()
            .flex()
            .flex_row()
            .gap(px(12.0))
            .flex_1()
            .min_h_0()
            .child(changes)
            .child(
                div()
                    .debug_selector(|| "review-detail".to_string())
                    .id("review-detail")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .overflow_y_scroll()
                    .child(verifications_section(&snapshot, lang))
                    .child(findings)
                    .child(detail),
            );

        div()
            .debug_selector(|| "review".to_string())
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .h_full()
            .p(px(18.0))
            .gap(px(14.0))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn decide_review(&mut self, change_set: &str, decision: Decision, cx: &mut Context<Self>) {
        let lang = self.state.lang;
        match self
            .state
            .decide_review(change_set, decision, String::new())
        {
            Ok(_) => {
                self.review_cache.borrow_mut().key = None;
                cx.notify();
            }
            Err(error) => {
                let text = tr_args(lang, "review.decisionFailed", &[("error", error)]);
                self.state.notice(text);
                cx.notify();
            }
        }
    }

    fn review_detail(
        &self,
        change: Option<&Change>,
        diff: Option<&FileDiff>,
        lang: crate::i18n::Lang,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(diff) = diff else {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .py(px(40.0))
                .child(txt(tr(lang, "review.selectHint"), 12.0, theme::faint()))
                .into_any_element();
        };
        let show_open = change.is_some() && !diff.binary && !diff.lines.is_empty();
        let change_for_diff = change.cloned();
        let mut detail = div().flex().flex_col().gap(px(8.0));
        detail = detail.child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(txt(diff.path.clone(), 13.0, theme::text()))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(txt(
                            tr_args(
                                lang,
                                "review.diffStats",
                                &[
                                    ("added", diff.added.to_string()),
                                    ("removed", diff.removed.to_string()),
                                ],
                            ),
                            10.5,
                            theme::faint(),
                        ))
                        .when(diff.binary, |row| {
                            row.child(txt(tr(lang, "review.binary"), 10.5, theme::warn()))
                        })
                        .when(show_open, |row| {
                            row.child(
                                div()
                                    .debug_selector(|| "review-open-diff".to_string())
                                    .id("review-open-diff")
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(rgb(theme::border()))
                                    .cursor_pointer()
                                    .hover(|button| button.bg(rgb(theme::hover())))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let Some(change) = change_for_diff.as_ref() else {
                                            return;
                                        };
                                        match this.state.review_patch(change) {
                                            Some(patch) => this.open_diff_text(
                                                &change.path.workspace_relative,
                                                &patch,
                                                cx,
                                            ),
                                            None => {
                                                this.state
                                                    .notice(tr(lang, "review.diffUnavailable"));
                                                cx.notify();
                                            }
                                        }
                                    }))
                                    .child(txt(tr(lang, "review.openDiff"), 10.0, theme::text())),
                            )
                        }),
                ),
        );
        if let Some(note) = &diff.note {
            detail = detail.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(rgb(theme::hover()))
                    .child(txt(note.clone(), 11.0, theme::muted())),
            );
        }
        if !diff.structural.is_empty() {
            let parents: std::collections::HashMap<&str, &str> = diff
                .structural
                .iter()
                .filter_map(|node| {
                    node.parent
                        .as_deref()
                        .map(|parent| (node.id.as_str(), parent))
                })
                .collect();
            let mut structure = div().flex().flex_col().gap(px(3.0));
            structure = structure.child(txt(tr(lang, "review.structure"), 12.0, theme::muted()));
            for node in diff.structural.iter().take(200) {
                let mut depth = 0usize;
                let mut cursor = node.parent.as_deref();
                while let Some(parent) = cursor {
                    depth += 1;
                    if depth >= 4 {
                        break;
                    }
                    cursor = parents.get(parent).copied();
                }
                let color = match node.change.as_str() {
                    "added" => theme::success(),
                    "removed" => theme::error(),
                    "modified" => theme::warn(),
                    _ => theme::text(),
                };
                structure = structure.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .pl(px(8.0 + 8.0 * depth as f32))
                        .pr(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgb(theme::page()))
                        .child(
                            txt(node.kind.clone(), 10.0, theme::faint())
                                .font_family(theme::CODE_FONT),
                        )
                        .child(
                            txt(node.qualified_name.clone(), 11.5, color)
                                .font_family(theme::CODE_FONT),
                        )
                        .child(txt(node.change.clone(), 10.0, theme::faint())),
                );
            }
            detail = detail.child(structure);
        } else if !diff.binary {
            detail = detail.child(txt(tr(lang, "review.noStructure"), 10.5, theme::faint()));
        }
        if !diff.lines.is_empty() {
            let mut lines = div()
                .id("review-diff-lines")
                .flex()
                .flex_col()
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(theme::border_soft()))
                .overflow_hidden();
            for line in diff.lines.iter().take(1_200) {
                let color = match line.mark.as_str() {
                    "+" => theme::success(),
                    "-" => theme::error(),
                    _ => theme::muted(),
                };
                let number = line
                    .new_line
                    .or(line.old_line)
                    .map(|number| number.to_string())
                    .unwrap_or_default();
                lines = lines.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(8.0))
                        .px(px(8.0))
                        .py(px(1.0))
                        .bg(rgb(if line.mark == " " {
                            theme::page()
                        } else {
                            theme::hover()
                        }))
                        .child(txt(number, 10.0, theme::faint()).font_family(theme::CODE_FONT))
                        .child(txt(line.mark.clone(), 10.0, color).font_family(theme::CODE_FONT))
                        .child(
                            txt(line.text.clone(), 11.0, color)
                                .font_family(theme::CODE_FONT)
                                .whitespace_nowrap(),
                        ),
                );
            }
            if diff.lines.len() > 1_200 {
                lines = lines.child(div().px(px(8.0)).py(px(4.0)).child(txt(
                    tr_args(
                        lang,
                        "review.moreLines",
                        &[("count", (diff.lines.len() - 1_200).to_string())],
                    ),
                    10.5,
                    theme::faint(),
                )));
            }
            detail = detail.child(lines);
        }
        detail.into_any_element()
    }

    fn review_empty(
        &self,
        title: &'static str,
        hint: Option<&'static str>,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let mut body = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .h_full()
            .gap(px(8.0))
            .child(icon(IconName::GitCompareArrows, 28.0, theme::faint()))
            .child(txt(tr(lang, title), 14.0, theme::text()));
        if let Some(hint) = hint {
            body = body.child(txt(tr(lang, hint), 11.5, theme::faint()));
        }
        body.into_any_element()
    }
}

fn verifications_section(snapshot: &ReviewSnapshot, lang: crate::i18n::Lang) -> AnyElement {
    let count = snapshot.review.verifications.len();
    let title = if count == 0 {
        tr(lang, "review.verificationsTitle")
    } else {
        tr_args(
            lang,
            "review.verifications",
            &[("count", count.to_string())],
        )
    };
    let mut section = div()
        .debug_selector(|| "review-verifications".to_string())
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(txt(title, 12.0, theme::muted()));
    if count == 0 {
        return section
            .child(txt(
                tr(lang, "review.noVerifications"),
                11.0,
                theme::faint(),
            ))
            .into_any_element();
    }
    for verification in snapshot.review.verifications.iter().take(20) {
        let status = verification.status.to_ascii_lowercase();
        let color =
            if status.contains("pass") || status.contains("ok") || status.contains("success") {
                theme::success()
            } else if status.contains("fail") || status.contains("error") {
                theme::error()
            } else {
                theme::warn()
            };
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(9.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .bg(rgb(theme::page()))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(txt(verification.kind.clone(), 10.0, theme::faint()))
                    .child(txt(verification.status.clone(), 10.0, color))
                    .when_some(verification.exit_code, |line, code| {
                        line.child(txt(
                            tr_args(
                                lang,
                                "review.verification.exit",
                                &[("code", code.to_string())],
                            ),
                            10.0,
                            theme::faint(),
                        ))
                    }),
            )
            .child(txt(verification.summary.clone(), 10.5, theme::muted()));
        if let Some(command) = &verification.command {
            row = row.child(
                txt(command.clone(), 10.0, theme::faint())
                    .font_family(theme::CODE_FONT)
                    .whitespace_nowrap()
                    .text_ellipsis(),
            );
        }
        section = section.child(row);
    }
    section.into_any_element()
}

fn findings_section(
    snapshot: &ReviewSnapshot,
    lang: crate::i18n::Lang,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let mut section = div()
        .debug_selector(|| "review-findings".to_string())
        .flex()
        .flex_col()
        .gap(px(4.0));
    section = section.child(txt(tr(lang, "review.findings"), 12.0, theme::muted()));
    if snapshot.review.findings.is_empty() {
        section = section.child(txt(tr(lang, "review.noFindings"), 11.0, theme::faint()));
        return section.into_any_element();
    }
    for (index, finding) in snapshot.review.findings.iter().enumerate() {
        let color = severity_color(finding.severity);
        let target = finding.change_id.as_ref().and_then(|id| {
            snapshot
                .changeset
                .changes
                .iter()
                .find(|change| &change.id == id)
                .cloned()
        });
        let row = div()
            .id(format!("review-finding-{index}"))
            .debug_selector(|| "review-finding-row".to_string())
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(9.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .bg(rgb(theme::page()))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .when(target.is_some(), |row| {
                row.cursor_pointer()
                    .hover(|row| row.bg(rgb(theme::hover())))
            })
            .when_some(target, |row, change| {
                row.on_click(cx.listener(move |this, _, _, cx| {
                    let diff = this.state.review_diff(&change);
                    this.review_selected = Some(change.id.clone());
                    *this.review_diff.borrow_mut() = Some((change.id.clone(), diff));
                    cx.notify();
                }))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(txt(severity_label(lang, finding.severity), 9.5, color))
                    .child(txt(finding.title.clone(), 11.5, theme::text())),
            )
            .child(txt(finding.detail.clone(), 10.5, theme::muted()));
        let row = if finding.evidence.is_empty() {
            row
        } else {
            row.child(div().flex().flex_row().flex_wrap().gap(px(6.0)).children(
                finding.evidence.iter().take(4).map(|evidence| {
                    txt(evidence.label.clone(), 9.5, theme::faint()).font_family(theme::CODE_FONT)
                }),
            ))
        };
        section = section.child(row);
    }
    section.into_any_element()
}

fn status_chip(lang: crate::i18n::Lang, status: ReviewStatus) -> Div {
    let color = match status {
        ReviewStatus::Approved => theme::success(),
        ReviewStatus::Stale | ReviewStatus::Rejected => theme::error(),
        ReviewStatus::ChangesRequested => theme::warn(),
        _ => theme::accent(),
    };
    chip(tr(lang, status_key(status)), color)
}

fn risk_chip(lang: crate::i18n::Lang, risk: RiskLevel) -> Div {
    let color = match risk {
        RiskLevel::Low => theme::success(),
        RiskLevel::Medium => theme::warn(),
        RiskLevel::High | RiskLevel::Critical => theme::error(),
    };
    chip(
        tr(
            lang,
            match risk {
                RiskLevel::Low => "review.risk.low",
                RiskLevel::Medium => "review.risk.medium",
                RiskLevel::High => "review.risk.high",
                RiskLevel::Critical => "review.risk.critical",
            },
        ),
        color,
    )
}

fn capture_chip(lang: crate::i18n::Lang, capture: CaptureMode) -> Div {
    chip(
        tr(
            lang,
            match capture {
                CaptureMode::Exact => "review.capture.exact",
                CaptureMode::Observed => "review.capture.observed",
                CaptureMode::Mixed => "review.capture.mixed",
                CaptureMode::Legacy => "review.capture.legacy",
            },
        ),
        theme::faint(),
    )
}

fn chip(label: String, color: u32) -> Div {
    div()
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(rgb(theme::hover()))
        .child(txt(label, 10.0, color))
}

fn status_key(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Draft => "review.status.draft",
        ReviewStatus::InReview => "review.status.inReview",
        ReviewStatus::ChangesRequested => "review.status.changesRequested",
        ReviewStatus::Approved => "review.status.approved",
        ReviewStatus::Rejected => "review.status.rejected",
        ReviewStatus::Stale => "review.status.stale",
    }
}

fn severity_label(lang: crate::i18n::Lang, severity: Severity) -> String {
    tr(
        lang,
        match severity {
            Severity::Info => "review.severity.info",
            Severity::Low => "review.severity.low",
            Severity::Medium => "review.severity.medium",
            Severity::High => "review.severity.high",
            Severity::Critical => "review.severity.critical",
        },
    )
}

fn severity_color(severity: Severity) -> u32 {
    match severity {
        Severity::Info | Severity::Low => theme::faint(),
        Severity::Medium => theme::warn(),
        Severity::High | Severity::Critical => theme::error(),
    }
}

fn change_kind(lang: crate::i18n::Lang, kind: &crate::review::domain::ChangeKind) -> String {
    tr(
        lang,
        match kind {
            crate::review::domain::ChangeKind::Created => "review.change.created",
            crate::review::domain::ChangeKind::Modified => "review.change.modified",
            crate::review::domain::ChangeKind::Deleted => "review.change.deleted",
            crate::review::domain::ChangeKind::Renamed { .. } => "review.change.renamed",
            crate::review::domain::ChangeKind::ModeChanged { .. } => "review.change.mode",
            crate::review::domain::ChangeKind::Unchanged => "review.change.unchanged",
        },
    )
}

fn scope_line(changeset: &ChangeSet, lang: crate::i18n::Lang) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(turn) = changeset.turn.as_deref() {
        parts.push(tr_args(lang, "review.turn", &[("id", short_id(turn))]));
    }
    if let Some(task) = changeset.task.as_deref() {
        parts.push(tr_args(lang, "review.task", &[("id", short_id(task))]));
    }
    if let Some(attempt) = changeset.attempt.as_deref() {
        parts.push(tr_args(
            lang,
            "review.attempt",
            &[("id", short_id(attempt))],
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn short_digest(digest: Option<&str>) -> String {
    let Some(digest) = digest else {
        return "-".to_string();
    };
    digest.chars().take(19).collect()
}
