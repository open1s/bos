//! The conversation, the trajectory, and the empty states.

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::text::{markdown, TextViewStyle};

/// What a click on a link in a message does.
///
/// The reference routes this through its `linkOpening` preference: `new-tab`
/// hands the URL to the system browser and `sidebar` loads it in the in-app
/// sidebar browser. This shell has no in-app browser host - `gpui-pre` ships no
/// webview - so both values land on the external channel, which is the one link
/// seam `App::open_url` actually offers. Before this the text views carried no
/// handler at all, so a link was inert.
fn open_link(url: &SharedString, _event: &ClickEvent, _window: &mut Window, cx: &mut App) {
    cx.open_url(url);
}
use gpui_kit::*;
use std::sync::Arc;

use super::shell::Shell;
use super::{glyph, icon, txt, DivExt, IconName};
use crate::model::{ChatMessage, PlanStatus, Role, ToolEvent};
use crate::theme;

/// A reasoning block's collapsed summary: the first line, with the emphasis
/// markers stripped.
///
/// `ReasoningRow` uses `firstLine(text).replaceAll("**", "")` for a settled block
/// and the last completed paragraph's first line while it streams; bsh has no
/// per-paragraph streaming state, so the settled rule is the drawn one.
pub(crate) fn think_summary(text: &str) -> String {
    text.lines().next().unwrap_or_default().replace("**", "")
}

/// Cap one rail preview the way the reference does: join the parts with a
/// space, collapse every whitespace run, then clip at `limit` with a trailing
/// ellipsis (`turn-navigation.js::preview`, over `PROMPT_PREVIEW_LIMIT` and
/// `RESPONSE_PREVIEW_LIMIT`).
///
/// The reference counts UTF-16 units; bsh counts characters, which differs only
/// for astral text.
pub(crate) fn rail_preview(parts: &[String], limit: usize) -> String {
    let mut text = String::new();
    let mut unread = false;
    for part in parts {
        if text.chars().count() >= limit * 2 {
            unread = true;
            break;
        }
        let clipped = part.chars().count() > limit * 2;
        let chunk: String = if clipped {
            part.chars().take(limit * 2).collect()
        } else {
            part.clone()
        };
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&chunk);
        if clipped {
            unread = true;
            break;
        }
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > limit - 1 {
        let head: String = normalized.chars().take(limit - 1).collect();
        return format!("{}…", head.trim_end());
    }
    if unread {
        format!("{normalized}…")
    } else {
        normalized
    }
}

/// The line icon for a tool, by the name it registers under.
fn tool_icon(name: &str) -> IconName {
    match name {
        "bash" => IconName::SquareTerminal,
        "read_file" => IconName::FileText,
        "write_file" => IconName::FilePen,
        "list_dir" => IconName::Folder,
        "load_skill" => IconName::Sparkles,
        "update_plan" => IconName::ClipboardList,
        _ => IconName::Wrench,
    }
}

impl Shell {
    /// The reference's own markdown scale, measured from its theme tokens.
    ///
    /// Headings are additive there - h1 21px, h2 19px, h3 18px and h4 the
    /// body size, each plus `content - 14` - while table cells sit at the
    /// secondary size and code blocks keep the fixed 11px/19px the reference
    /// pins. Every other field keeps its default, which folds the active theme
    /// in; only the highlight theme and the `is_dark` flag are named here so a
    /// dark window does not get light code colours.
    pub(crate) fn markdown_style(&self) -> TextViewStyle {
        let content = self.state.settings.content_font_size;
        let dark = theme::is_dark();
        let table = |weight: Option<FontWeight>| {
            let cell = StyleRefinement::default()
                .text_size(px(theme::small_size_at(content)))
                .line_height(px(theme::table_line_at(content)));
            match weight {
                Some(weight) => cell.font_weight(weight),
                None => cell,
            }
        };
        TextViewStyle {
            heading_base_font_size: px(theme::body_size_at(content)),
            heading_font_size: Some(Arc::new(move |level, _base| {
                px(theme::heading_size_at(content, level))
            })),
            table: table(None),
            table_head: table(Some(FontWeight::MEDIUM)),
            table_cell: table(None),
            code_block: StyleRefinement::default()
                .text_size(px(theme::CODE_BLOCK_SIZE))
                .line_height(px(theme::CODE_LINE)),
            highlight_theme: if dark {
                HighlightTheme::default_dark()
            } else {
                HighlightTheme::default_light()
            },
            is_dark: dark,
            ..TextViewStyle::default()
        }
    }

    /// The scrolling conversation.
    pub(crate) fn render_transcript(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.state.active_session() else {
            return self.empty_state("empty.none").into_any_element();
        };
        if session.messages.is_empty() {
            // The reference draws its hero in the composer stack instead, so a
            // blank session leaves the transcript itself empty.
            return div().into_any_element();
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(txt(
                    crate::state::stamp(session.created_at),
                    theme::CHIP_SIZE,
                    theme::faint(),
                ))
                .child(
                    div()
                        .id("copy-transcript")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::Copy, 12.0, theme::faint()))
                        .child(txt(
                            self.state.lang.tr("msg.copy"),
                            theme::CHIP_SIZE,
                            theme::faint(),
                        ))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            let text = this.state.transcript_text();
                            if text.is_empty() {
                                return;
                            }
                            cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(text));
                            let copied = this.state.lang.tr("chat.copied");
                            this.state.notice(copied);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        // `foldCompletedTurns` folds every closed turn but never the one still
        // streaming, so the last turn stays open while it runs.
        let folded = self
            .state
            .settings
            .effective()
            .transcript_view
            .fold_completed_turns;
        let live = self
            .state
            .active_id
            .as_deref()
            .is_some_and(|id| self.state.is_streaming(id));
        let last = session.messages.len().saturating_sub(1);
        for (index, message) in session.messages.iter().enumerate() {
            let settled = folded && !(live && index == last);
            rows.push(
                self.message_row(index, message, settled, cx)
                    .into_any_element(),
            );
        }
        // The reference puts a turn-jump rail down the transcript's right edge.
        // The scroll handle records each child's painted bounds, so a tick can jump
        // to a measured row without knowing any row's height up front.
        let turns = self.turn_indices();
        // `if (items.length < 2) return null`, and `@container (width <= 900px)`
        // hides the frame. The container is the conversation column, measured
        // from the window; before the first frame it is not narrow.
        let narrow = self.conversation_w > px(0.0)
            && self.conversation_w <= px(theme::TURN_RAIL_MIN_CONTAINER);

        let active = self.visible_turn(&turns);
        // Measured (TurnNavigator): a 28px frame parked 12px off the transcript's
        // right edge, capped at `min(band - 64, 420)` and scrolled when the marks
        // overflow. A mark is a 10px slot holding a 2px x 20px bar, right-aligned,
        // drawn at 60% while idle (border-l4), 90% on hover (label-tertiary) and
        // 100% in label-primary for the turn at the top.
        let track = div()
            .debug_selector(|| "turn-rail-track".to_string())
            .id("turn-rail-track")
            .max_h(px(theme::TURN_RAIL_MAX_H))
            // `RAIL_INSET_PX`: the mark column pads each end of the frame.
            .py(px(theme::TURN_RAIL_INSET))
            .overflow_y_scroll()
            .track_scroll(&self.rail_scroll)
            .flex()
            .flex_col()
            .items_end()
            .children(turns.iter().enumerate().map(|(turn, index)| {
                let child = 1 + index; // child 0 is the timestamp and copy row
                let on = active == Some(turn);
                let group = SharedString::from(format!("turn-{turn}"));
                let bar_group = group.clone();
                div()
                    .debug_selector(|| format!("turn-tick-{turn}"))
                    .id(SharedString::from(format!("turn-tick-{turn}")))
                    .group(group)
                    // The reference positions its marks absolutely; in a flex
                    // column with 1 as the default shrink they would squeeze to
                    // fit the 420 cap instead of overflowing into the scroller.
                    .flex_shrink_0()
                    .w(px(theme::TURN_RAIL_W))
                    .h(px(theme::TURN_PITCH))
                    .flex()
                    .items_center()
                    .justify_end()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        // `TurnMark` previews on pointer move and the frame
                        // clears it on leave; a mark's own leave is the same
                        // observable here.
                        if *hovered {
                            this.rail_hover = Some(turn);
                        } else if this.rail_hover == Some(turn) {
                            this.rail_hover = None;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .debug_selector(|| format!("turn-bar-{turn}"))
                            .h(px(2.0))
                            .w(px(if on { 20.0 } else { 12.0 }))
                            .rounded(px(2.0))
                            .bg(rgb(if on { theme::text() } else { theme::border() }))
                            .when(!on, |bar| {
                                bar.group_hover(bar_group, |s| {
                                    s.w(px(18.0)).bg(rgb(theme::faint()))
                                })
                            }),
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        // Leaving the end on purpose: the tail must not pull the
                        // reader back down on the next frame.
                        this.following_tail = false;
                        this.transcript_scroll.scroll_to_top_of_item(child);
                        cx.notify();
                    }))
            }));
        // Measured: the navigator overlays the column at `right: 12px`, so the
        // transcript keeps its full width underneath it. The frame is the
        // positioned box the marks scroll in and the preview card hangs off;
        // `TurnNavigator` renders both from the frame, so the card is never
        // clipped by the scroller.
        let marks_h = theme::TURN_RAIL_INSET * 2.0 + theme::TURN_PITCH * turns.len() as f32;
        let frame_h = marks_h.min(theme::TURN_RAIL_MAX_H);
        let offset = f32::from(self.rail_scroll.offset().y);
        let preview = self.rail_hover.and_then(|turn| {
            let index = turns.iter().position(|item| *item == turn)?;
            // The card tracks the hovered mark's centre within the frame
            // (`--turn-preview-center`).
            let centre = theme::TURN_RAIL_INSET
                + theme::TURN_PITCH * index as f32
                + theme::TURN_PITCH / 2.0
                + offset;
            Some((turn, centre))
        });
        let frame = div()
            .debug_selector(|| "turn-rail-frame".to_string())
            .relative()
            .w(px(theme::TURN_RAIL_W))
            .flex()
            .flex_col()
            .child(track)
            .children(preview.map(|(turn, centre)| self.turn_preview(turn, centre, frame_h)));
        // A single turn has nothing to navigate between, and the reference hides
        // the whole frame under 900px of conversation width.
        let rail = (turns.len() >= 2 && !narrow).then(|| {
            div()
                .debug_selector(|| "turn-rail".to_string())
                .id("turn-rail")
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(theme::TURN_RAIL_RIGHT))
                .w(px(theme::TURN_RAIL_W))
                .flex()
                .flex_col()
                .items_end()
                .justify_center()
                .child(frame)
                .into_any_element()
        });
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_row()
            .relative()
            .child(
                div()
                    .debug_selector(|| "transcript".to_string())
                    .id("transcript")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.transcript_scroll)
                    .on_scroll_wheel(cx.listener(|_this, _event, _window, cx| cx.notify()))
                    .px(px(theme::INSET))
                    .py(px(24.0))
                    .flex()
                    .flex_col()
                    .gap(px(22.0))
                    .children(rows),
            )
            .children(rail)
            .into_any_element()
    }

    /// The rail's hover preview card, from `TurnNavigator .preview`: at most
    /// `min(300px, container - 120px)` wide and 100px tall, filled with
    /// `bg-layer-1` under `elevation-panel`, holding a one-line 13px/20px strong
    /// prompt over up to three 12px/18px label-caption response lines.
    fn turn_preview(&self, turn: usize, centre: f32, frame_h: f32) -> AnyElement {
        let (prompt, response) = self.turn_preview_texts(turn);
        let prompt = if prompt.is_empty() {
            self.state
                .lang
                .tr("chat.railTurn")
                .replace("{turn}", &turn.to_string())
        } else {
            prompt
        };
        // `top: clamp(0px, calc(var(--turn-preview-center) - 50px), calc(100% - 100px))`
        let top = (centre - theme::TURN_PREVIEW_H / 2.0)
            .clamp(0.0, (frame_h - theme::TURN_PREVIEW_H).max(0.0));
        // `width: min(300px, 100cqw - 120px)`. The frame only exists above a
        // 900px container, where `100cqw - 120px` already exceeds 300, so the
        // measured width is the 300px cap.
        let width = theme::TURN_PREVIEW_W;
        div()
            .debug_selector(|| "turn-preview".to_string())
            .id("turn-preview")
            .absolute()
            .top(px(top))
            // `right: calc(100% + 10px)`
            .right(px(theme::TURN_RAIL_W + theme::TURN_PREVIEW_GAP))
            .w(px(width))
            .max_h(px(theme::TURN_PREVIEW_H))
            .overflow_hidden()
            .rounded(px(theme::RADIUS_LG))
            .bg(rgb(theme::page()))
            .shadow_lg()
            .px(px(12.0))
            .py(px(10.0))
            .child(
                txt(prompt, 13.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(20.0))
                    .max_h(px(20.0))
                    .overflow_hidden(),
            )
            .when(!response.is_empty(), |card| {
                card.child(
                    txt(response, 12.0, theme::faint())
                        .line_height(px(18.0))
                        .max_h(px(54.0))
                        .mt(px(4.0))
                        .overflow_hidden(),
                )
            })
            .into_any_element()
    }

    /// The rail item's two previews (`turn-navigation.js`): the turn's user
    /// prompt and the last assistant text in the turn that has any - the
    /// reference takes `loaded.findLast((node) => responseText(node) !== "")`.
    fn turn_preview_texts(&self, turn: usize) -> (String, String) {
        let Some(session) = self.state.active_session() else {
            return (String::new(), String::new());
        };
        let end = self
            .turn_indices()
            .iter()
            .find(|index| **index > turn)
            .copied()
            .unwrap_or(session.messages.len());
        let prompt = session
            .messages
            .get(turn)
            .filter(|message| message.role == Role::User)
            .map(|message| {
                rail_preview(
                    std::slice::from_ref(&message.text),
                    theme::TURN_PREVIEW_PROMPT,
                )
            })
            .unwrap_or_default();
        let response = session
            .messages
            .get(turn..end)
            .unwrap_or_default()
            .iter()
            .rev()
            .find(|message| message.role == Role::Assistant && !message.text.is_empty())
            .map(|message| {
                rail_preview(
                    std::slice::from_ref(&message.text),
                    theme::TURN_PREVIEW_RESPONSE,
                )
            })
            .unwrap_or_default();
        (prompt, response)
    }

    /// The turn whose top is the last one at or above the transcript's top edge.
    pub(crate) fn visible_turn(&self, turns: &[usize]) -> Option<usize> {
        let viewport = self.transcript_scroll.bounds();
        let offset = self.transcript_scroll.offset().y;
        let mut visible = None;
        for (turn, index) in turns.iter().enumerate() {
            let Some(child) = self.transcript_scroll.bounds_for_item(1 + index) else {
                continue;
            };
            if child.top() + offset <= viewport.top() + px(8.0) {
                visible = Some(turn);
            }
        }
        visible.or_else(|| turns.first().map(|_| 0))
    }

    /// The plan and tool trajectory for the active session.
    pub(crate) fn render_trajectory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.state.active_session() else {
            return self.empty_state("empty.none").into_any_element();
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(goal) = &session.goal {
            rows.push(
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(
                        self.state.lang.tr("pane.goal"),
                        theme::CHIP_SIZE,
                        theme::muted(),
                    ))
                    .child(txt(
                        goal.objective.clone(),
                        theme::body_size_at(self.state.settings.content_font_size),
                        theme::text(),
                    ))
                    .into_any_element(),
            );
        }
        for item in &session.plan {
            let mark = match item.status {
                PlanStatus::Completed => "✓",
                PlanStatus::InProgress => "●",
                PlanStatus::Pending => "○",
            };
            rows.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(glyph(mark, 12.0, theme::muted()))
                    .child(txt(
                        item.text.clone(),
                        theme::body_size_at(self.state.settings.content_font_size),
                        theme::text(),
                    ))
                    .into_any_element(),
            );
        }
        for (index, message) in session.messages.iter().enumerate() {
            for (tool_index, tool) in message.tools.iter().enumerate() {
                rows.push(
                    self.tool_row(index, tool_index, tool, false, cx)
                        .into_any_element(),
                );
            }
        }
        if rows.is_empty() {
            return self.empty_state("empty.none").into_any_element();
        }
        div()
            .id("trajectory")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(theme::INSET))
            .py(px(24.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .children(rows)
            .into_any_element()
    }

    /// One message.
    /// `folded` is the policy a completed turn folds under: the reference's
    /// `foldCompletedTurns` reading, which suppresses a settled turn's live
    /// process while leaving the running turn's rows alone.
    fn message_row(
        &self,
        index: usize,
        message: &ChatMessage,
        folded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if message.role == Role::User {
            let size = theme::body_size_at(self.state.settings.content_font_size);
            let line = theme::bubble_line_at(self.state.settings.content_font_size);
            return div()
                .w_full()
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    // Measured (MessageItem): radius-xl 20 over `10px 16px`, the
                    // bubble fill, `line-height: 22px + delta`, capped at 82% of
                    // the row - and at `content * .702`, which only binds wider.
                    div()
                        .debug_selector(move || format!("user-bubble-{index}"))
                        // 82% of the transcript's content box. The reference
                        // also caps at `content * .702`, which only binds wider.
                        .max_w(px((self.chat_w() - 2.0 * theme::INSET) * 0.82))
                        .px(px(16.0))
                        .py(px(10.0))
                        .rounded(px(20.0))
                        .bg(rgb(theme::user_bubble()))
                        .child(
                            txt(message.text.clone(), size, theme::text()).line_height(px(line)),
                        ),
                )
                .into_any_element();
        }

        let mut column = div().w_full().flex().flex_col().gap(px(8.0));

        if !message.reasoning.is_empty() {
            column = column.child(self.think_row(index, message, folded, cx));
        }

        if message.text.is_empty() && message.error.is_none() && !folded {
            column = column.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(glyph("●", 9.0, theme::accent()))
                    .child(txt(
                        match self.state.turn_elapsed() {
                            Some(elapsed) => {
                                format!("{} · {elapsed}", self.state.lang.tr("msg.running"))
                            }
                            None => self.state.lang.tr("msg.running").to_string(),
                        },
                        theme::body_size_at(self.state.settings.content_font_size),
                        theme::muted(),
                    )),
            );
        } else {
            column = column.child(
                markdown(message.text.clone())
                    .on_link_click(open_link)
                    .style(self.markdown_style())
                    .line_height(px(theme::body_line_at(
                        self.state.settings.content_font_size,
                    )))
                    .text_size(px(theme::body_size_at(
                        self.state.settings.content_font_size,
                    )))
                    .text_color(rgb(theme::text())),
            );
        }

        for (tool_index, tool) in message.tools.iter().enumerate() {
            column = column.child(self.tool_row(index, tool_index, tool, folded, cx));
        }

        if let Some(error) = &message.error {
            column = column.child(
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::error()))
                    .px(px(10.0))
                    .py(px(8.0))
                    .child(txt(error.clone(), theme::CHIP_SIZE, theme::error())),
            );
        }

        column
            .child(self.message_footer(index, message, cx))
            .into_any_element()
    }

    /// The reasoning disclosure, measured from the shared `DisclosureRow` over
    /// `ReasoningRow`.
    ///
    /// Collapsed it is a `24px + delta` row: a `16px + delta` leading box holding a
    /// `14px + delta` glyph with a 6px right margin (swapped for a chevron while the
    /// row is hovered), the secondary-size title on the same 24px line, then a 2px
    /// dot with 8px margins and a nowrap, ellipsised summary whose line is
    /// `20px + secondary delta`. Open, the complete reasoning renders in a body
    /// indented `22px + delta`. The row colour is label-tertiary, label-secondary
    /// under the pointer.
    fn think_row(
        &self,
        index: usize,
        message: &ChatMessage,
        folded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = self.state.settings.content_font_size;
        let row_h = theme::disclosure_row_at(content);
        let small = theme::small_size_at(content);
        let quiet = theme::faint();
        let hover = theme::muted();
        // The reference's preview is `!expanded && summary !== "" && (running ||
        // policy.settledReasoningPreview)`, so a policy without the preview shows
        // the settled block's title alone rather than a summary it drops.
        let preview = self.state.settings.effective().transcript_view;
        let open =
            self.think_open.contains(&index) || (folded && !preview.settled_reasoning_preview);
        let summary = think_summary(&message.reasoning);
        let group = SharedString::from(format!("think-{index}"));
        let row = div()
            .debug_selector(move || format!("think-row-{index}"))
            .id(SharedString::from(format!("think-row-{index}")))
            .group(group.clone())
            .flex_shrink_0()
            .h(px(row_h))
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .cursor_pointer()
            .child(
                div()
                    .debug_selector(move || format!("think-leading-{index}"))
                    .relative()
                    .flex_none()
                    .w(px(theme::DISCLOSURE_LEADING))
                    .h(px(theme::DISCLOSURE_LEADING))
                    .mr(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(IconName::Lightbulb, theme::DISCLOSURE_GLYPH, quiet)
                            .group_hover(group.clone(), |style| style.opacity(0.0)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(theme::DISCLOSURE_LEADING))
                            .h(px(theme::DISCLOSURE_LEADING))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(0.0)
                            .group_hover(group.clone(), |style| style.opacity(1.0))
                            .child(icon(
                                if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme::DISCLOSURE_GLYPH,
                                hover,
                            )),
                    ),
            )
            .child(
                txt(self.state.lang.tr("msg.thinking"), small, quiet)
                    .debug_selector(move || format!("think-title-{index}"))
                    .flex_none()
                    .line_height(px(row_h))
                    .group_hover(group.clone(), |style| style.text_color(rgb(hover))),
            );
        // A settled summary is the first line with the emphasis markers stripped;
        // the reference hides the pair entirely when the summary is empty, or while
        // the block is open.
        let row = if open || summary.is_empty() {
            row
        } else {
            row.child(
                div()
                    .debug_selector(move || format!("think-dot-{index}"))
                    .flex_none()
                    .w(px(2.0))
                    .h(px(2.0))
                    .mx(px(8.0))
                    .rounded(px(1.0))
                    .bg(rgb(quiet)),
            )
            .child(
                div()
                    .debug_selector(move || format!("think-summary-{index}"))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .group_hover(group.clone(), |style| style.text_color(rgb(hover)))
                    .child(
                        txt(summary, small, quiet).line_height(px(theme::summary_line_at(content))),
                    ),
            )
        };
        let mut root = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(row.on_click(cx.listener(move |this, _event, _window, cx| {
                if !this.think_open.remove(&index) {
                    this.think_open.insert(index);
                }
                cx.notify();
            })));
        if open {
            root = root.child(
                div()
                    .debug_selector(move || format!("think-body-{index}"))
                    .min_w_0()
                    .pt(px(4.0))
                    .pb(px(4.0))
                    .pl(px(22.0 + theme::delta_at(content)))
                    .child(
                        markdown(message.reasoning.clone())
                            .on_link_click(open_link)
                            .style(self.markdown_style())
                            .line_height(px(theme::summary_line_at(content)))
                            .text_size(px(small))
                            .text_color(rgb(theme::muted())),
                    ),
            );
        }
        root
    }

    /// Good / bad / rewind, plus usage.
    fn message_footer(
        &self,
        index: usize,
        message: &ChatMessage,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let feedback = message
            .meta
            .as_ref()
            .and_then(|meta| meta.feedback.clone())
            .unwrap_or_default();
        let usage = message
            .meta
            .as_ref()
            .and_then(|meta| meta.usage.as_ref())
            .map(|usage| usage.total())
            .unwrap_or(0);
        let session_id = self.state.active_id.clone().unwrap_or_default();
        let good_id = session_id.clone();
        let bad_id = session_id.clone();
        let cut_id = session_id;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .id(SharedString::from(format!("good-{index}")))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(glyph(
                        "👍",
                        11.0,
                        if feedback == "good" {
                            theme::accent()
                        } else {
                            theme::faint()
                        },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.set_feedback(&good_id, index, "good");
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("bad-{index}")))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(glyph(
                        "👎",
                        11.0,
                        if feedback == "bad" {
                            theme::error()
                        } else {
                            theme::faint()
                        },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.set_feedback(&bad_id, index, "bad");
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("cut-{index}")))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::RefreshCw, 12.0, theme::faint()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.truncate_from(&cut_id, index);
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            // Performance & usage: the reference's `Compact` mode keeps the
            // per-turn usage reading to itself, so the footer's own token figure
            // is the row the preference governs.
            .when(
                usage > 0 && self.state.settings.effective().performance_usage != "compact",
                |row| {
                    row.child(txt(
                        format!("{usage} {}", self.state.lang.tr("turn.tokens")),
                        theme::CHIP_SIZE,
                        theme::faint(),
                    ))
                },
            )
    }

    /// One collapsible tool row.
    fn tool_row(
        &self,
        message_index: usize,
        tool_index: usize,
        tool: &ToolEvent,
        folded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // `liveProcessDetail` keeps the running turn's own steps open; the modes
        // that drop it (`compact`, `verbose`) fold a completed turn's tool bodies
        // the same way the reference's `stepGrouping: collapsed` does.
        let preview = self.state.settings.effective().transcript_view;
        let open = self.state.tool_open(message_index, tool_index)
            || (folded && preview.live_process_detail);
        let name = tool.name.clone();
        let args = tool.args.clone();
        let output = tool.output.clone();
        let ms = tool.ms;
        let done = output.is_some();
        let mut body = div()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::new_session_fill()))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "tool-{message_index}-{tool_index}"
                    )))
                    .px(px(10.0))
                    .py(px(7.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .child(icon(
                        if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        12.0,
                        theme::muted(),
                    ))
                    .child(icon(tool_icon(&name), 13.0, theme::muted()))
                    .child(
                        txt(
                            name,
                            theme::small_size_at(self.state.settings.content_font_size),
                            theme::text(),
                        )
                        .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        txt(
                            if done { "done" } else { "running" },
                            theme::CHIP_SIZE,
                            if done {
                                theme::success()
                            } else {
                                theme::accent()
                            },
                        )
                        .flex_1(),
                    )
                    .when_some(ms, |row, ms| {
                        row.child(txt(format!("{ms} ms"), theme::CHIP_SIZE, theme::faint()))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.state.toggle_tool(message_index, tool_index);
                        cx.notify();
                    })),
            );
        if open {
            body = body.child(
                div()
                    .px(px(10.0))
                    .pb(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(txt(args, theme::CHIP_SIZE, theme::muted()))
                    .when_some(output, |column, output| {
                        column.child(
                            div()
                                .border_t_1()
                                .border_color(rgb(theme::border_soft()))
                                .pt(px(6.0))
                                .child(txt(output, theme::CHIP_SIZE, theme::faint())),
                        )
                    }),
            );
        }
        body
    }

    /// Nothing to show.
    fn empty_state(&self, key: &'static str) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(txt(
                self.state.lang.tr(key),
                theme::body_size_at(self.state.settings.content_font_size),
                theme::muted(),
            ))
    }
}
