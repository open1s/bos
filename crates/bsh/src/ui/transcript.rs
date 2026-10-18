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
use super::todo_panel::spinner;
use super::{glyph, icon, txt, DivExt, IconName};
use crate::i18n::tr;
use crate::model::{ChatMessage, PlanStatus, Role, ToolEvent};
use crate::settings::TranscriptPolicy;
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

/// The kind word a tool row shows in place of its raw tool name.
///
/// The reference labels a step by what it DID - Code, Think, Bash, Read,
/// Write, Edit - rather than by the registry name of the tool that did it, so
/// two providers' differently-named file writers read as one kind.
pub(crate) fn tool_kind_key(name: &str) -> &'static str {
    match name {
        "bash" | "run_command" | "terminal" => "tool.kind.bash",
        "read_file" => "tool.kind.read",
        "write_file" => "tool.kind.write",
        "edit_file" | "apply_patch" => "tool.kind.edit",
        "list_dir" => "tool.kind.list",
        "load_skill" => "tool.kind.skill",
        "update_plan" => "tool.kind.plan",
        _ => "tool.kind.other",
    }
}

/// Whether a value carries anything worth showing as a summary.
fn meaningful(text: &str) -> bool {
    text.chars()
        .any(|ch| ch.is_alphanumeric() || ch == '/' || ch == '_')
}
/// The one-line summary a tool row carries beside its kind.
///
/// The arguments arrive as the JSON the model sent, so the useful line is the
/// first well-known field's value (a command, a path, a query) rather than the
/// whole object; anything else falls back to the first string value, and then
/// to the raw arguments. Newlines collapse and long values clip, because this
/// line is a summary and the body holds the whole thing.
pub(crate) fn tool_summary(args: &str) -> String {
    const KEYS: [&str; 7] = [
        "command",
        "path",
        "file_path",
        "query",
        "code",
        "instruction",
        "prompt",
    ];
    for key in KEYS {
        if let Some(value) = json_field(args, key) {
            return clip_summary(&value);
        }
    }
    match first_json_string(args) {
        Some(value) => clip_summary(&value),
        None if !meaningful(args) => String::new(),
        None => clip_summary(args),
    }
}

/// The exact path a file tool was given.
///
/// The row's summary is CLIPPED for display (96 chars and a ellipsis), so using
/// it to open a file breaks on any path longer than that - the panel then
/// resolves a path that does not exist. This reads the argument itself.
pub(crate) fn tool_path(args: &str) -> Option<String> {
    for key in ["file_path", "path", "file"] {
        if let Some(value) = json_field(args, key) {
            return Some(value);
        }
    }
    None
}

/// The string value of `key` in an object literal, ignoring escapes' contents.
fn json_field(args: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = args.find(&needle)? + needle.len();
    let rest = args[at..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    quoted_span(rest).map(|(value, _)| value)
}

/// The value of `key` with its escapes decoded EXACTLY, so a body keeps its
/// newlines - a summary flattens them, a patch must not.
fn json_field_exact(args: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = args.find(&needle)? + needle.len();
    let rest = args[at..].trim_start().strip_prefix(':')?.trim_start();
    quoted_exact(rest)
}

/// Read one quoted string, decoding `\n` and `\t` as themselves.
fn quoted_exact(text: &str) -> Option<String> {
    let mut chars = text.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return None,
    }
    let mut value = String::new();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 't')) => value.push('\t'),
                Some((_, other)) => value.push(other),
                None => break,
            },
            other => value.push(other),
        }
    }
    None
}
/// The first quoted VALUE in the arguments, whatever its key.
///
/// A key is also a quoted string, so a span followed by `:` is skipped: the
/// first span whose remainder is not a colon is a value.
fn first_json_string(args: &str) -> Option<String> {
    let mut rest = args;
    while let Some((open, _)) = rest.char_indices().find(|(_, c)| *c == '"') {
        let tail = &rest[open..];
        let (value, after) = quoted_span(tail)?;
        if !after.trim_start().starts_with(':') {
            return Some(value);
        }
        rest = after;
    }
    None
}

/// Read one quoted string off the front of `text`, with what follows it.
fn quoted_span(text: &str) -> Option<(String, &str)> {
    let mut chars = text.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return None,
    }
    let mut value = String::new();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '"' => return Some((value, &text[index + 1..])),
            '\\' => match chars.next() {
                Some((_, next)) => value.push(match next {
                    'n' | 't' => ' ',
                    other => other,
                }),
                None => break,
            },
            other => value.push(other),
        }
    }
    None
}
/// The language a step's body should be highlighted as, when the step carries
/// code at all.
///
/// A shell step's body IS the command, so it is bash; a file step's body is the
/// file it wrote or patched, so the language comes from that path's extension.
/// Nothing else claims a language, and a body with none renders as plain text
/// rather than pretending.
pub(crate) fn code_lang(name: &str, args: &str) -> Option<&'static str> {
    match name {
        "bash" | "run_command" | "terminal" => Some("bash"),
        "write_file" | "edit_file" | "apply_patch" | "read_file" => {
            let path = json_field(args, "path")
                .or_else(|| json_field(args, "file_path"))
                .unwrap_or_default();
            Some(match path.rsplit('.').next().unwrap_or_default() {
                "rs" => "rust",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" | "mjs" => "javascript",
                "py" => "python",
                "go" => "go",
                "json" => "json",
                "toml" => "toml",
                "yml" | "yaml" => "yaml",
                "md" => "markdown",
                "css" => "css",
                "html" => "html",
                "sh" => "bash",
                _ => {
                    // A file step with no language still has a body worth
                    // showing; markdown is the neutral fence that keeps it
                    // readable without claiming syntax it does not have.
                    "markdown"
                }
            })
        }
        _ => None,
    }
}

/// The bytes a step moved, for the size badge the reference puts on its rows.
///
/// It is the size of the payload the step carried - its output when it has one,
/// its arguments while it runs - not a disk size: bsh keeps no file handle, and
/// a badge that read the disk would be a second source of truth for the same
/// row.
pub(crate) fn tool_bytes(tool: &ToolEvent) -> usize {
    tool.output
        .as_deref()
        .map(str::len)
        .unwrap_or_else(|| tool.args.len())
}

/// A byte count the way a row badge shows it: 812 B, 7.2 KB.
pub(crate) fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f32 / 1024.0)
    }
}

/// A tool's output as the body should read it: the streams a command returned,
/// not the JSON envelope around them.
///
/// The shell tool answers `{stdout, stderr, exit_code, success}`, and showing
/// that object puts `\n` escapes in front of a reader who wants the text. A
/// failing command keeps its stderr in the same body - the red tone is the
/// row's own signal, and the body does not hide why it failed.
pub(crate) fn output_body(output: &str) -> String {
    let stdout = json_field_exact(output, "stdout");
    let stderr = json_field_exact(output, "stderr");
    if stdout.is_none() && stderr.is_none() {
        return output.to_string();
    }
    let mut text = stdout.unwrap_or_default();
    let err = stderr.unwrap_or_default();
    if !err.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    let trimmed = text.trim_end().to_string();
    if trimmed.is_empty() {
        // A silent success is still a result: say so rather than drawing an
        // empty box.
        return "(no output)".to_string();
    }
    trimmed
}
/// The patch a write or an edit asked for, as `-` / `+` lines.
///
/// This is the SHAPE of the change the model requested, not a disk diff: bsh
/// keeps the arguments and no before-image, so a write shows its content as
/// additions and an edit shows the string it replaced against the one it put
/// there. It says what the step did; it does not pretend to know what the file
/// held.
pub(crate) fn tool_diff(name: &str, args: &str) -> Option<Vec<(char, String)>> {
    match name {
        "write_file" => {
            let content = json_field_exact(args, "content")?;
            Some(
                content
                    .lines()
                    .map(|line| ('+', line.to_string()))
                    .collect(),
            )
        }
        "edit_file" | "apply_patch" => {
            let mut lines: Vec<(char, String)> = Vec::new();
            if let Some(old) = json_field_exact(args, "old_string") {
                lines.extend(old.lines().map(|line| ('-', line.to_string())));
            }
            if let Some(new) = json_field_exact(args, "new_string") {
                lines.extend(new.lines().map(|line| ('+', line.to_string())));
            }
            if lines.is_empty() {
                None
            } else {
                Some(lines)
            }
        }
        _ => None,
    }
}
/// Collapse a value to one clipped line.
fn clip_summary(text: &str) -> String {
    let one_line = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    const MAX: usize = 96;
    if one_line.chars().count() <= MAX {
        return one_line;
    }
    let clipped: String = one_line.chars().take(MAX).collect();
    format!("{clipped}…")
}

/// How much further a mouse-wheel notch travels than a trackpad's own delta.
///
/// A trackpad sends precise pixels and its hardware already carries momentum, so
/// those go through 1:1. A mouse wheel sends whole LINES, one coarse step per
/// detent - at 1:1 a long transcript takes dozens of notches to cross, which is
/// what "the scroll needs to be fast" was about. Two is the smallest gain that
/// makes one notch a comfortable step without landing somewhere unrelated.
const MOUSE_GAIN: f32 = 2.0;

/// One wheel event's vertical delta in pixels.
///
/// Both kinds are resolved against the window's line height - the same conversion
/// the scroller itself would have done - and only the coarse one is gained.
fn wheel_delta(event: &ScrollWheelEvent, window: &mut Window) -> f32 {
    wheel_pixels(&event.delta, window.line_height())
}

/// The gained pixel delta for one wheel delta, split out so a test can pin it.
pub(crate) fn wheel_pixels(delta: &ScrollDelta, line_height: Pixels) -> f32 {
    let pixels = f32::from(delta.pixel_delta(line_height).y);
    match delta {
        ScrollDelta::Lines(_) => pixels * MOUSE_GAIN,
        _ => pixels,
    }
}

/// The chat transcript's wheel handler, named so the element chain stays small.
fn chat_wheel(
    this: &mut Shell,
    event: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    let dy = wheel_delta(event, window);
    this.wheel_chat(dy, cx);
    cx.stop_propagation();
}

/// The trajectory tab's wheel handler.
pub(crate) fn traj_wheel(
    this: &mut Shell,
    event: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    let dy = wheel_delta(event, window);
    this.wheel_trajectory(dy, cx);
    cx.stop_propagation();
}

/// The presentation facts every row of one frame shares.
///
/// A transcript resolves its settings once per frame: the policy, whether the
/// usage figure is suppressed, the content font size and the markdown style all
/// describe the same repaint, and asking per row turned a long conversation into
/// hundreds of resolutions and style allocations per frame.
struct RowChrome<'a> {
    /// How much of a completed turn the transcript view shows.
    policy: TranscriptPolicy,
    /// Whether the Compact performance preference hides the footer's usage.
    compact_usage: bool,
    /// The content font size the row's type scale derives from.
    content_size: u32,
    /// The markdown style every text view in this frame copies.
    style: &'a TextViewStyle,
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
        let content = self.state.draft().global.general.content_font_size;
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
            // The reference writes a code block in its code face
            // (`--ds-font-family-code`), so the block asks for the same face the
            // trajectory's tool rows use instead of inheriting the UI one.
            code_block: StyleRefinement::default()
                .text_size(px(theme::CODE_BLOCK_SIZE))
                .line_height(px(theme::CODE_LINE))
                .font_family(theme::CODE_FONT),
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
        //
        // One settings resolution and one markdown style serve the whole frame:
        // `effective()` clones a resolved workspace, and the style allocates its
        // heading closure, so asking once per row made a long conversation pay
        // hundreds of clones on every repaint.
        let effective = self.state.settings.effective();
        let style = self.markdown_style();
        let chrome = RowChrome {
            policy: effective.transcript_view,
            compact_usage: effective.performance_usage == "compact",
            content_size: self.state.draft().global.general.content_font_size,
            style: &style,
        };
        let folded = chrome.policy.fold_completed_turns;
        let live = self
            .state
            .active_id
            .as_deref()
            .is_some_and(|id| self.state.is_streaming(id));
        let last = session.messages.len().saturating_sub(1);
        for (index, message) in session.messages.iter().enumerate() {
            let settled = folded && !(live && index == last);
            rows.push(
                self.message_row(index, message, settled, &chrome, cx)
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
        let last_ordinal = turns.len().saturating_sub(1);
        // Measured (TurnNavigator): a 28px frame parked 12px off the transcript's
        // right edge, capped at `min(band - 64, 420)` and scrolled when the marks
        // overflow. A mark is a 10px slot holding a 2px x 20px bar, right-aligned,
        // drawn at 60% while idle (border-l4), 90% on hover (label-tertiary) and
        // 100% in label-primary for the turn at the top.
        // The frame is exactly this tall, and the track with it: a max_h did not
        // cap the column (a flex child grew past it), so the marks were painted
        // below the frame's own box - and a mark outside its box cannot be
        // clicked. The height is computed here because the scroller needs it now.
        let marks_h = theme::TURN_RAIL_INSET * 2.0 + theme::TURN_PITCH * turns.len() as f32;
        let frame_h = marks_h.min(theme::TURN_RAIL_MAX_H);
        let track = div()
            .debug_selector(|| "turn-rail-track".to_string())
            .id("turn-rail-track")
            .flex_none()
            .h(px(frame_h))
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
                let is_last = turn == last_ordinal;
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
                        this.jump_to_turn(child, is_last);
                        cx.notify();
                    }))
            }));
        // Measured: the navigator overlays the column at `right: 12px`, so the
        // transcript keeps its full width underneath it. The frame is the
        // positioned box the marks scroll in and the preview card hangs off;
        // `TurnNavigator` renders both from the frame, so the card is never
        // clipped by the scroller.
        let offset = f32::from(self.rail_scroll.offset().y);
        // `rail_hover` holds the mark's ORDINAL in the rail (that is what the
        // hook stores), while `turns` holds MESSAGE indices. Searching `turns`
        // for an ordinal only ever matched even marks, so every odd mark - the
        // newest one always among them - drew no card at all.
        let preview = self.rail_hover.and_then(|ordinal| {
            let turn = *turns.get(ordinal)?;
            // The card tracks the hovered mark's centre within the frame
            // (`--turn-preview-center`).
            let centre = theme::TURN_RAIL_INSET
                + theme::TURN_PITCH * ordinal as f32
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
                    // The wheel is taken here rather than by the scroller: the
                    // delta becomes a target the frame loop eases towards, and
                    // taking it also stops the tail-follow from pinning the
                    // transcript back to the newest message.
                    .on_scroll_wheel(cx.listener(chat_wheel))
                    .on_scroll_wheel(cx.listener(|_this, _event, _window, cx| cx.notify()))
                    .px(px(theme::INSET))
                    .py(px(24.0))
                    .flex()
                    .flex_col()
                    .gap(px(22.0))
                    .children(rows),
            )
            .children(rail)
            // While a window of older rows is loading, the pane shows the
            // shared busy mark rather than a frame that looks idle. It is an
            // overlay, so it adds no child and the turn rail's child indices
            // keep naming the messages while it shows.
            .when(self.older.is_busy(), |frame| {
                frame.child(
                    div()
                        .debug_selector(|| "older-loading".to_string())
                        .absolute()
                        .top(px(6.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded_full()
                                .bg(rgb(theme::page()))
                                .child(spinner()),
                        ),
                )
            })
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
        // At the tail the turn in front of the reader is the NEWEST one, even
        // though its top sits below the viewport's: the newest turn is the one
        // the last mark names, and highlighting the turn above it left the last
        // mark permanently dim beside a chat that had already ended.
        if !turns.is_empty() {
            let max = self.transcript_scroll.max_offset().y;
            let at_bottom = max > px(0.0) && self.transcript_scroll.offset().y <= -max + px(1.0);
            if self.following_tail || at_bottom {
                return Some(turns.len() - 1);
            }
        }
        let viewport = self.transcript_scroll.bounds();
        let offset = self.transcript_scroll.offset().y;
        let mut visible = None;
        for (turn, index) in turns.iter().enumerate() {
            let Some(child) = self.transcript_scroll.bounds_for_item(1 + index) else {
                continue;
            };
            if child.top() + offset <= viewport.top() + px(8.0) {
                visible = Some(turn);
            } else {
                // Rows are laid out in order, so the first one whose top is
                // below the viewport's top ends the search: every later turn is
                // below it too. Without this the rail scanned the whole
                // transcript once per frame.
                break;
            }
        }
        visible.or_else(|| turns.first().map(|_| 0))
    }

    /// The plan and tool trajectory for the active session.
    pub(crate) fn render_trajectory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_trajectory_view(cx)
    }

    #[allow(dead_code)]
    fn render_trajectory_legacy(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        theme::body_size_at(self.state.draft().global.general.content_font_size),
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
                        theme::body_size_at(self.state.draft().global.general.content_font_size),
                        theme::text(),
                    ))
                    .into_any_element(),
            );
        }
        // Tool rows come from the Parquet trajectory log, paged: the trace of a
        // long run is scrollable without being resident. The in-memory rows are
        // only drawn while nothing is paged yet (a session older than the log).
        let style = self.markdown_style();
        let chrome = RowChrome {
            policy: self.state.settings.effective().transcript_view,
            compact_usage: self.state.settings.effective().performance_usage == "compact",
            content_size: self.state.draft().global.general.content_font_size,
            style: &style,
        };
        if self.trace_rows.is_empty() && session.messages.iter().any(|m| !m.tools.is_empty()) {
            for (index, message) in session.messages.iter().enumerate() {
                for (tool_index, tool) in message.tools.iter().enumerate() {
                    rows.push(
                        self.tool_row(index, tool_index, tool, false, &chrome, cx)
                            .into_any_element(),
                    );
                }
            }
        } else {
            for (index, row) in self.trace_rows.iter().enumerate() {
                rows.push(self.trace_row(index, row, &chrome, cx).into_any_element());
            }
        }
        if rows.is_empty() {
            return self.empty_state("empty.none").into_any_element();
        }
        div()
            .id("trajectory")
            .debug_selector(|| String::from("trajectory"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.traj_scroll)
            .on_scroll_wheel(cx.listener(traj_wheel))
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
    /// One trajectory row read from the log: the same step shape the transcript
    /// draws, keyed by its place in the paged trace rather than by a message.
    fn trace_row(
        &self,
        index: usize,
        row: &crate::history::ToolRow,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tool = ToolEvent {
            name: row.name.clone(),
            args: row.args.clone(),
            output: row.output.clone(),
            ms: row.ms,
            at: None,
        };
        // A trace row is history: one summary line, opened on a click.
        self.tool_row(usize::MAX - index, 0, &tool, true, chrome, cx)
    }

    fn message_row(
        &self,
        index: usize,
        message: &ChatMessage,
        folded: bool,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if message.role == Role::User {
            let size = theme::body_size_at(chrome.content_size);
            let line = theme::bubble_line_at(chrome.content_size);
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
            column = column.child(self.think_row(index, message, folded, chrome, cx));
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
                        theme::body_size_at(self.state.draft().global.general.content_font_size),
                        theme::muted(),
                    )),
            );
        } else {
            column = column.child(
                markdown(message.text.clone())
                    .on_link_click(open_link)
                    .style(chrome.style.clone())
                    .line_height(px(theme::body_line_at(chrome.content_size)))
                    .text_size(px(theme::body_size_at(chrome.content_size)))
                    .text_color(rgb(theme::text())),
            );
        }

        for (tool_index, tool) in message.tools.iter().enumerate() {
            column = column.child(self.tool_row(index, tool_index, tool, folded, chrome, cx));
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
            .child(self.message_footer(index, message, chrome.compact_usage, cx))
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
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = chrome.content_size;
        let row_h = theme::disclosure_row_at(content);
        let small = theme::small_size_at(content);
        let quiet = theme::faint();
        let hover = theme::muted();
        // The reference's preview is `!expanded && summary !== "" && (running ||
        // policy.settledReasoningPreview)`, so a policy without the preview shows
        // the settled block's title alone rather than a summary it drops.
        let open = self.think_open.contains(&index)
            || (folded && !chrome.policy.settled_reasoning_preview);
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
                            .style(chrome.style.clone())
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
        compact_usage: bool,
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
            .when(usage > 0 && !compact_usage, |row| {
                row.child(txt(
                    format!("{usage} {}", self.state.lang.tr("turn.tokens")),
                    theme::CHIP_SIZE,
                    theme::faint(),
                ))
            })
    }

    /// One collapsible step row, in the reference's trace shape.
    ///
    /// Collapsed it is one line: a chevron, the KIND the step is (Code, Bash,
    /// Read, Write, ...), a `·`, then the step's own single-line summary. Open,
    /// the arguments and the output follow in the same box, indented under the
    /// row's own rail. The kind comes from the tool, but the row is labelled by
    /// what the step DID, which is what two providers' different tool names
    /// share.
    fn tool_row(
        &self,
        message_index: usize,
        tool_index: usize,
        tool: &ToolEvent,
        folded: bool,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // `liveProcessDetail` governs the RUNNING turn: its steps stay open while
        // it works. A settled turn folds its process the way the reference's
        // `stepGrouping: collapsed` does - one summary line per step, opened on a
        // click - which is the shape a reader scans a finished trace in. Reading
        // the flag the other way round opened every body of every settled turn,
        // which is what made a long transcript a wall of arguments.
        let open = self.state.tool_open(message_index, tool_index)
            || (!folded && chrome.policy.live_process_detail);
        let lang = self.state.lang;
        let size = theme::small_size_at(chrome.content_size);
        let name = tool.name.clone();
        let kind = tr(lang, tool_kind_key(&name));
        let summary = tool_summary(&tool.args);
        let args = tool.args.clone();
        let output = tool.output.clone();
        let ms = tool.ms;
        let done = output.is_some();
        let mut row = div()
            // The nesting the trace shows: every step sits one rail in from the
            // message it belongs to.
            .debug_selector(move || format!("tool-{message_index}-{tool_index}"))
            .w_full()
            .pl(px(theme::TOOL_INDENT))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .border_l_1()
            .border_color(rgb(theme::border_soft()));
        // A file step's path inside the row is its own control, so the row-wide
        // fold handler is attached only when there is no link to protect.
        let opens_file = matches!(
            name.as_str(),
            "read_file" | "write_file" | "edit_file" | "apply_patch"
        );
        let head = div()
            .id(SharedString::from(format!(
                "tool-head-{message_index}-{tool_index}"
            )))
            .debug_selector({
                let selector = format!("tool-head-{message_index}-{tool_index}");
                move || selector.clone()
            })
            .ml(px(10.0))
            .py(px(4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            // The disclosure lives on the chevron and the label, NOT on the whole
            // row: a file step's path inside the row is its own control that opens
            // the file, and a row-wide handler swallowed it (the live click
            // unfolded the step instead of opening the file).
            .child(
                div()
                    .id(SharedString::from(format!(
                        "tool-fold-{message_index}-{tool_index}"
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        // The head below folds on any press too; without this the
                        // chevron would toggle and the head would toggle it back.
                        cx.stop_propagation();
                        this.state.toggle_tool(message_index, tool_index);
                        cx.notify();
                    }))
                    .child(icon(
                        if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        12.0,
                        theme::faint(),
                    ))
                    .child(icon(tool_icon(&name), 13.0, theme::muted()))
                    .child(
                        txt(kind, size, theme::text())
                            .font_weight(FontWeight::MEDIUM)
                            .flex_none(),
                    ),
            )
            .when(!summary.is_empty(), |head| {
                // A file step's summary IS a path, and the reference makes it a
                // link: pressing it opens that file in the right panel's editor
                // instead of leaving the reader to find it in the tree.
                head.child(txt("·", size, theme::faint()).flex_none())
                    .child(
                        div()
                            // A stateful cell so the link can take a click; a
                            // plain div cannot.
                            .id(SharedString::from(format!(
                                "open-file-{message_index}-{tool_index}"
                            )))
                            .debug_selector(move || {
                                format!("open-file-{message_index}-{tool_index}")
                            })
                            .min_w_0()
                            .overflow_hidden()
                            .flex_1()
                            .when(opens_file, |cell| {
                                cell.cursor_pointer().hover(|s| s.underline()).on_click(
                                    cx.listener(move |this, _event, window, cx| {
                                        // The row below folds the step on any press;
                                        // the link is its own target.
                                        cx.stop_propagation();
                                        this.open_in_editor(message_index, tool_index, window, cx);
                                    }),
                                )
                            })
                            .child(
                                txt(summary, size, theme::muted())
                                    .whitespace_nowrap()
                                    .text_ellipsis(),
                            ),
                    )
            })
            .child(
                txt(
                    if done { "" } else { "●" },
                    theme::CHIP_SIZE,
                    theme::accent(),
                )
                .flex_none(),
            )
            .child(
                txt(
                    format_bytes(tool_bytes(tool)),
                    theme::CHIP_SIZE,
                    theme::faint(),
                )
                .flex_none(),
            )
            .when_some(ms, |head, ms| {
                head.child(txt(format!("{ms} ms"), theme::CHIP_SIZE, theme::faint()).flex_none())
            })
            // The whole row folds, file step or not: the path cell inside a file
            // step stops propagation, so the link still wins where it sits.
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.state.toggle_tool(message_index, tool_index);
                cx.notify();
            }));
        row = row.child(head);
        // The reference's own link out of a step: pressing it opens the matching
        // line in the Trajectory tab, so a reader who wants the whole call - the
        // arguments, the full output, the timing - does not have to unfold the
        // transcript and hunt for it there.
        row = row.child(
            div()
                .debug_selector(move || format!("inspect-{message_index}-{tool_index}"))
                .id(SharedString::from(format!(
                    "inspect-{message_index}-{tool_index}"
                )))
                .ml(px(10.0))
                .mb(px(4.0))
                .px(px(7.0))
                .h(px(22.0))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.0))
                .rounded(px(6.0))
                // No border: a bordered box in a 22px row reads as a heavy frame.
                // The fill carries the shape, and it appears under the pointer.
                .cursor_pointer()
                .hover(|chip| chip.bg(rgb(theme::hover())))
                .child(icon(IconName::Code, 11.0, theme::muted()))
                .child(txt(
                    tr(lang, "chat.inspect"),
                    theme::CHIP_SIZE,
                    theme::muted(),
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.inspect_in_trajectory(message_index, tool_index);
                    cx.notify();
                })),
        );
        // A step with no recorded arguments and no output used to unfold to nothing,
        // so a press looked ignored. Say so instead.
        if open && args.trim().is_empty() && output.is_none() {
            row = row.child(
                div()
                    .debug_selector(move || format!("tool-empty-{message_index}-{tool_index}"))
                    .ml(px(10.0))
                    .mb(px(6.0))
                    .child(txt(
                        tr(lang, "chat.stepEmpty"),
                        theme::CHIP_SIZE,
                        theme::faint(),
                    )),
            );
        }
        if open {
            let lang_key = code_lang(&name, &args);
            // The patch wins over the raw arguments when the step wrote or
            // edited: `-`/`+` lines say what changed, where the JSON would make
            // the reader diff it in their head.
            if let Some(lines) = tool_diff(&name, &args) {
                let mut diff = div()
                    .ml(px(10.0))
                    .mb(px(6.0))
                    .rounded(px(8.0))
                    // The fill is the box; a hairline around a block this wide
                    // doubled every edge in the transcript.
                    .bg(rgb(theme::new_session_fill()))
                    .px(px(10.0))
                    .py(px(7.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .max_h(px(theme::TOOL_BODY_MAX))
                    .overflow_hidden();
                for (mark, line) in lines.into_iter().take(60) {
                    let (tone, fill) = if mark == '+' {
                        (theme::success(), theme::diff_add())
                    } else {
                        (theme::error(), theme::diff_del())
                    };
                    diff = diff.child(div().px(px(4.0)).rounded(px(3.0)).bg(rgb(fill)).child(txt(
                        format!("{mark} {line}"),
                        theme::CHIP_SIZE,
                        tone,
                    )));
                }
                row = row.child(diff);
            } else if !args.is_empty() {
                // A command step carries its command in the arguments, so the
                // body shows that rather than the JSON envelope around it.
                let in_text = if lang_key == Some("bash") {
                    json_field(&args, "command").unwrap_or_else(|| args.clone())
                } else {
                    args.clone()
                };
                row = row.child(self.step_block("IN", &in_text, lang_key, true, chrome, cx));
            }
            if let Some(output) = output.clone() {
                row = row.child(self.step_block(
                    "OUT",
                    &output_body(&output),
                    None,
                    false,
                    chrome,
                    cx,
                ));
            }
        }
        row
    }

    /// One labelled body of a step: the label, a copy control, and the text -
    /// highlighted as code when the step carries a language, plain otherwise.
    fn step_block(
        &self,
        label: &'static str,
        text: &str,
        lang: Option<&'static str>,
        is_input: bool,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let payload = text.to_string();
        let copy_id = format!("step-copy-{label}-{}-{is_input}", payload.len());

        let mut block = div()
            .ml(px(10.0))
            .mb(px(6.0))
            .rounded(px(8.0))
            .bg(rgb(theme::new_session_fill()))
            .px(px(10.0))
            .py(px(7.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(txt(label, theme::CHIP_SIZE, theme::faint()).flex_1())
                    .child(
                        div()
                            .id(SharedString::from(copy_id.clone()))
                            .debug_selector({
                                let selector = copy_id.clone();
                                move || selector.clone()
                            })
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(theme::hover())))
                            .child(icon(IconName::Copy, 11.0, theme::faint()))
                            .on_click(cx.listener(move |_this, _event, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(payload.clone()));
                            })),
                    ),
            );
        let body = match lang {
            Some(lang) => markdown(format!("```{lang}\n{text}\n```"))
                .style(chrome.style.clone())
                .text_size(px(theme::CHIP_SIZE))
                .max_h(px(theme::TOOL_BODY_MAX))
                .overflow_hidden()
                .into_any_element(),
            None => txt(
                text.to_string(),
                theme::CHIP_SIZE,
                if is_input {
                    theme::muted()
                } else {
                    theme::faint()
                },
            )
            .line_height(px(theme::stat_line_at(chrome.content_size)))
            .max_h(px(theme::TOOL_BODY_MAX))
            .overflow_hidden()
            .into_any_element(),
        };
        block = block.child(body);
        block.into_any_element()
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
                theme::body_size_at(self.state.draft().global.general.content_font_size),
                theme::muted(),
            ))
    }
}
