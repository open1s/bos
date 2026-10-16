//! The palette and layout metrics.
//!
//! Colours are BSH tokens resolved from the active gpui-kit theme, which owns
//! the light and dark palettes (see <https://gpui-kit.com/component/theme>).
//! The accessors hand the current values to the views; [`sync`] refreshes them
//! from the kit after the theme changes, and [`toggle`] flips the mode. Sizes
//! and layout metrics stay constant.
use gpui_kit::component::{
    theme::{Theme, ThemeMode, ThemeRegistry},
    ActiveTheme as _,
};
use gpui_kit::*;
use std::sync::atomic::{AtomicU32, Ordering};
static PAGE_C: AtomicU32 = AtomicU32::new(0xFFFFFF);
static SIDEBAR_BG_C: AtomicU32 = AtomicU32::new(0xE8E9EB);
static SELECTED_ROW_C: AtomicU32 = AtomicU32::new(0xDDDEE1);
static NEW_SESSION_FILL_C: AtomicU32 = AtomicU32::new(0xF2F3F5);
static HOVER_C: AtomicU32 = AtomicU32::new(0xE1E3E7);
static TEXT_C: AtomicU32 = AtomicU32::new(0x0F1115);
static MUTED_C: AtomicU32 = AtomicU32::new(0x6B7280);
static FAINT_C: AtomicU32 = AtomicU32::new(0x8A90A0);
static BORDER_SOFT_C: AtomicU32 = AtomicU32::new(0xEBEBEB);
static BORDER_C: AtomicU32 = AtomicU32::new(0xD8DBE2);
static BORDER_STRONG_C: AtomicU32 = AtomicU32::new(0xB3B8C4);
static ACCENT_C: AtomicU32 = AtomicU32::new(0x4176E6);
static ERROR_C: AtomicU32 = AtomicU32::new(0xD23F3F);
static SUCCESS_C: AtomicU32 = AtomicU32::new(0x2F9E63);
static WARN_C: AtomicU32 = AtomicU32::new(0xB8860B);
static USER_BUBBLE_C: AtomicU32 = AtomicU32::new(0xF2F3F5);
/// The page background.
pub(crate) fn page() -> u32 {
    PAGE_C.load(Ordering::Relaxed)
}
/// The sidebar background.
pub(crate) fn sidebar_bg() -> u32 {
    SIDEBAR_BG_C.load(Ordering::Relaxed)
}
/// The highlighted session row.
pub(crate) fn selected_row() -> u32 {
    SELECTED_ROW_C.load(Ordering::Relaxed)
}
/// The New Session button fill.
pub(crate) fn new_session_fill() -> u32 {
    NEW_SESSION_FILL_C.load(Ordering::Relaxed)
}
/// The hovered row fill.
pub(crate) fn hover() -> u32 {
    HOVER_C.load(Ordering::Relaxed)
}
/// Primary text.
pub(crate) fn text() -> u32 {
    TEXT_C.load(Ordering::Relaxed)
}
/// Secondary text.
pub(crate) fn muted() -> u32 {
    MUTED_C.load(Ordering::Relaxed)
}
/// The faintest text.
pub(crate) fn faint() -> u32 {
    FAINT_C.load(Ordering::Relaxed)
}
/// Hairline borders.
pub(crate) fn border_soft() -> u32 {
    BORDER_SOFT_C.load(Ordering::Relaxed)
}
/// Field and card borders.
pub(crate) fn border() -> u32 {
    BORDER_C.load(Ordering::Relaxed)
}
/// The strongest border, for dialogs and outer frames.
pub(crate) fn border_strong() -> u32 {
    BORDER_STRONG_C.load(Ordering::Relaxed)
}
/// The action colour.
pub(crate) fn accent() -> u32 {
    ACCENT_C.load(Ordering::Relaxed)
}
/// Errors.
pub(crate) fn error() -> u32 {
    ERROR_C.load(Ordering::Relaxed)
}
/// Successes.
pub(crate) fn success() -> u32 {
    SUCCESS_C.load(Ordering::Relaxed)
}
/// Warnings.
pub(crate) fn warn() -> u32 {
    WARN_C.load(Ordering::Relaxed)
}
/// The user bubble fill.
pub(crate) fn user_bubble() -> u32 {
    USER_BUBBLE_C.load(Ordering::Relaxed)
}
/// How much wider the composer card is than the chat column.
///
/// Measured: `--dsh-composer-card-max-width` is the chat content width plus
/// 32px, so the card reaches past the transcript column on both sides.
pub(crate) const COMPOSER_WIDEN: f32 = 32.0;
/// The composer editor's own floor, measured: `.input{min-height:36px}`.
pub(crate) const COMPOSER_INPUT_MIN_H: f32 = 36.0;
/// The composer editor's ceiling, measured: `--dsh-composer-text-max-height`
/// is 336px and the wrapping `.scroll` clips and scrolls past it.
pub(crate) const COMPOSER_INPUT_MAX_H: f32 = 336.0;
/// The padding the kit's own multi-line field draws inside itself.
///
/// Measured, not read: a one-draft composer field comes out 36 tall over a
/// 20px line box, so the kit's own field draws 8px of padding a side where the
/// reference's `.input`/`.editor` carry their padding on the box. The wrappers
/// below therefore carry only the reference's padding less this, which is what
/// makes the resting composer land on the measured 36px floor.
pub(crate) const FIELD_INTERIOR_PAD: f32 = 8.0;
/// The width the `.scroll` wrapper gives up on the right: `margin-right:4px`
/// over the input's own 8px right padding.
pub(crate) const COMPOSER_INPUT_MR: f32 = 4.0;
/// The editor's own padding, measured: `.input{padding:4px 8px 0 14px}`.
///
/// The wrapper carries only what the kit's own interior does not already draw.
/// Its measured 4px top pad sits inside that interior and cannot be taken back,
/// so a draft rests 4px lower here than the reference's.
pub(crate) const COMPOSER_INPUT_PAD_L: f32 = 14.0;
pub(crate) const COMPOSER_INPUT_PAD_R: f32 = 8.0;
/// The row ceiling the editor's own state carries before the measured 336px
/// cap governs.
///
/// The reference caps in pixels only, so this is only a backstop: it is set
/// past the row count that reaches 336, and the wrapper's `max_h` is what
/// actually clamps, exactly as the reference's `.scroll` does. A bsh line
/// measures 20 tall inside the kit's field where the reference's is 24, so
/// bsh reaches the ceiling a line or two later.
pub(crate) const COMPOSER_INPUT_MAX_ROWS: usize = 20;
/// The sidebar label size.
pub(crate) const SIDEBAR_SIZE: f32 = 14.0;
/// The transcript body size.
pub(crate) const BODY_SIZE: f32 = 14.0;
/// The chip and annotation size.
pub(crate) const CHIP_SIZE: f32 = 12.0;

/// The low end of the reference Font size stepper, in px.
pub(crate) const CONTENT_MIN: u32 = 10;
/// The high end of the reference Font size stepper, in px.
pub(crate) const CONTENT_MAX: u32 = 22;
/// Conversation base text at a given content size, which the reference Font
/// size setting moves directly, clamped to the range its stepper accepts.
pub(crate) fn body_size_at(px: u32) -> f32 {
    px.clamp(CONTENT_MIN, CONTENT_MAX) as f32
}

/// The reference's content size anchor in px: its `--dsh-content-font-delta`
/// is `content - 14px`, so every conversation size is additive.
pub(crate) const CONTENT_DEFAULT: u32 = 14;

/// The additive delta every measured conversation size is built from.
pub(crate) fn delta_at(px: u32) -> f32 {
    body_size_at(px) - CONTENT_DEFAULT as f32
}

/// Flow-row titles, summaries and table cells at a given content size.
///
/// The reference's `--dsh-content-font-size-secondary` is
/// `min(size - 1px, max(13px, size - 2px))`, so the gap widens to two steps
/// once the body passes 15px. Small text and code keep their fixed sizes.
pub(crate) fn small_size_at(px: u32) -> f32 {
    let size = body_size_at(px);
    (size - 1.0).min((size - 2.0).max(13.0))
}

/// A markdown heading's size: the reference's h1 21px, h2 19px, h3 18px and
/// h4 the body size, each plus the delta (`--dsw-font-markdown-h*`). The
/// reference defines no h5/h6, so those keep the h4 size.
pub(crate) fn heading_size_at(px: u32, level: u8) -> f32 {
    let base = match level {
        1 => 21.0,
        2 => 19.0,
        3 => 18.0,
        _ => CONTENT_DEFAULT as f32,
    };
    base + delta_at(px)
}

/// The conversation body line height: the reference's 24px plus the delta.
pub(crate) fn body_line_at(px: u32) -> f32 {
    24.0 + delta_at(px)
}

/// A message bubble's line height: the reference's 22px plus the delta.
///
/// `MessageItem .bubble` carries `line-height: calc(22px + var(--dsh-content-
/// font-delta, 0px))` - the same additive delta as the body, over 22 rather than
/// the body's 24.
pub(crate) fn bubble_line_at(px: u32) -> f32 {
    22.0 + delta_at(px)
}

/// A table cell's line height: the reference's 22px plus the secondary delta
/// (`--dsh-content-font-delta-secondary`).
pub(crate) fn table_line_at(px: u32) -> f32 {
    22.0 + (small_size_at(px) - 13.0)
}
/// The shared disclosure row height: the reference's 24px plus the delta.
///
/// `DisclosureRow .row` owns it (and the same value is the title's line height),
/// and `ReasoningRow` collapses to exactly this height.
pub(crate) fn disclosure_row_at(px: u32) -> f32 {
    24.0 + delta_at(px)
}

/// A disclosure summary's line height: 20px plus the secondary delta
/// (`ReasoningRow .summary`, a different value from a table cell's 22).
pub(crate) fn summary_line_at(px: u32) -> f32 {
    20.0 + (small_size_at(px) - 13.0)
}

/// The disclosure leading box: `16px + delta` (the delta is added at the call
/// site, since the box and its glyph move together).
pub(crate) const DISCLOSURE_LEADING: f32 = 16.0;
/// The glyph inside a disclosure leading box: 14px.
pub(crate) const DISCLOSURE_GLYPH: f32 = 14.0;

/// The `--dsw-radius-lg` token, measured from the reference's base sheet.
pub(crate) const RADIUS_LG: f32 = 16.0;

/// The `--dsw-radius-sm` token from the same sheet: the meter trigger's corner.
pub(crate) const RADIUS_SM: f32 = 8.0;

/// The turn rail's geometry, from `TurnNavigator`: a 10px mark pitch
/// (`TURN_SPACING_PX`), a 6px inset at each end (`RAIL_INSET_PX`), a 28px frame
/// parked 12px off the transcript's right edge and capped at 420.
pub(crate) const TURN_PITCH: f32 = 10.0;
pub(crate) const TURN_RAIL_INSET: f32 = 6.0;
pub(crate) const TURN_RAIL_W: f32 = 28.0;
pub(crate) const TURN_RAIL_RIGHT: f32 = 12.0;
pub(crate) const TURN_RAIL_MAX_H: f32 = 420.0;
/// The container width under which the rail hides
/// (`@container (width <= 900px) { .frame { display: none } }`).
pub(crate) const TURN_RAIL_MIN_CONTAINER: f32 = 900.0;
/// The rail's preview card: a 100px cap (`--turn-preview-height`), a
/// `min(300px, 100cqw - 120px)` width and a 10px gap from the frame
/// (`right: calc(100% + 10px)`).
pub(crate) const TURN_PREVIEW_H: f32 = 100.0;
pub(crate) const TURN_PREVIEW_W: f32 = 300.0;
pub(crate) const TURN_PREVIEW_GAP: f32 = 10.0;
/// The preview's two text budgets (`PROMPT_PREVIEW_LIMIT`,
/// `RESPONSE_PREVIEW_LIMIT` in `turn-navigation.js`).
pub(crate) const TURN_PREVIEW_PROMPT: usize = 50;
pub(crate) const TURN_PREVIEW_RESPONSE: usize = 120;

/// The stat pills under the composer, from `StatsPills.module.css`: a 12px row
/// gap, a 1/8 padded pill at a 999px radius, a 14px glyph and a 6px inner gap.
pub(crate) const STAT_GAP: f32 = 12.0;
pub(crate) const STAT_PILL_PAD_X: f32 = 8.0;
pub(crate) const STAT_PILL_PAD_Y: f32 = 1.0;
pub(crate) const STAT_PILL_ICON: f32 = 14.0;
pub(crate) const STAT_PILL_GAP: f32 = 6.0;
/// The pill's inner `·` separator (`.sep`, `margin: 0 6px`).
pub(crate) const STAT_SEP_MARGIN: f32 = 6.0;
/// The stat dialog panel, from `stat-dialog.module.css`: `max-content` between
/// a 300px floor and a 440px ceiling over 16px of padding at radius 16, opening
/// 8px above its trigger (the reference also keeps a 12px viewport margin, which
/// a fixed panel inside the centred column never needs), on the prominent
/// elevation (drawn as a `.5px` `border-l1` hairline).
pub(crate) const STAT_PANEL_MIN_W: f32 = 300.0;
pub(crate) const STAT_PANEL_MAX_W: f32 = 440.0;
pub(crate) const STAT_PANEL_PAD: f32 = 16.0;
pub(crate) const STAT_PANEL_RADIUS: f32 = 16.0;
pub(crate) const STAT_PANEL_GAP: f32 = 8.0;
/// The panel's title row: a 14px glyph, a 16px gap and 8px under the row, with
/// its own rule 10px under that.
pub(crate) const STAT_TITLE_ICON: f32 = 14.0;
pub(crate) const STAT_TITLE_GAP: f32 = 16.0;
pub(crate) const STAT_TITLE_MB: f32 = 8.0;
pub(crate) const STAT_RULE_MB: f32 = 10.0;
/// The panel's own type: 12px over 18px, fixed rather than derived.
pub(crate) const STAT_PANEL_SIZE: f32 = 12.0;
pub(crate) const STAT_PANEL_LINE: f32 = 18.0;
/// The panel's detail grid: `minmax(76px, auto) minmax(0, 1fr)` over a
/// `6px 16px` gap.
pub(crate) const STAT_GRID_LABEL_MIN: f32 = 76.0;
pub(crate) const STAT_GRID_GAP_X: f32 = 16.0;
pub(crate) const STAT_GRID_GAP_Y: f32 = 6.0;

/// The composer's context meter, from `ContextMeter.module.css`: a 14px ring
/// (a 5.5px radius and a 2px stroke) beside the percentage in a 1/8 padded
/// trigger, opening a `min(264px, 100vw - 24px)` panel 8px above it over 12px
/// of padding at radius 16, with a 4px stacked bar (a 10px top and 12px bottom
/// margin, 1px between segments, each at least 2px wide) and a legend whose
/// 8x8 swatch sits 6px before its label.
pub(crate) const METER_RING: f32 = 14.0;
pub(crate) const METER_RING_RADIUS: f32 = 5.5;
pub(crate) const METER_STROKE: f32 = 2.0;
pub(crate) const METER_PAD_X: f32 = 8.0;
pub(crate) const METER_PAD_Y: f32 = 1.0;
pub(crate) const METER_GAP: f32 = 6.0;
/// The trigger sits 4px under the composer card, which is what the reference's
/// `dock` adds before the meter's own row.
pub(crate) const METER_TOP_GAP: f32 = 4.0;
pub(crate) const METER_PANEL_W: f32 = 264.0;
pub(crate) const METER_PANEL_PAD: f32 = 12.0;
pub(crate) const METER_PANEL_GAP: f32 = 8.0;
pub(crate) const METER_PANEL_SIZE: f32 = 12.0;
pub(crate) const METER_PANEL_LINE: f32 = 20.0;
pub(crate) const METER_HEADER_GAP: f32 = 6.0;
pub(crate) const METER_BAR_H: f32 = 4.0;
pub(crate) const METER_BAR_TOP: f32 = 10.0;
pub(crate) const METER_BAR_BOTTOM: f32 = 12.0;
pub(crate) const METER_SEG_GAP: f32 = 1.0;
pub(crate) const METER_SEG_MIN_W: f32 = 2.0;
pub(crate) const METER_ROWS_TOP: f32 = 6.0;
pub(crate) const METER_ROW_PAD_Y: f32 = 2.0;
pub(crate) const METER_ROW_GAP: f32 = 12.0;
pub(crate) const METER_SWATCH: f32 = 8.0;
pub(crate) const METER_SWATCH_GAP: f32 = 6.0;

// The reference's hero, measured from `HeroShell.module.css` and the hero
// phase of `ConversationRoot.module.css`: the composer stack is one
// 8px-gap column `min(card + 32, 100%)` wide, 32px off the bottom, holding
// the headline, the workspace row and the composer.
/// The hero root's side padding.
pub(crate) const HERO_PAD_X: f32 = 24.0;
/// The gap under the headline's stack (the reference's empty body).
pub(crate) const HERO_STACK_GAP: f32 = 12.0;
/// The headline's size.
pub(crate) const HERO_HEADLINE_SIZE: f32 = 26.0;
/// The headline's line height.
pub(crate) const HERO_HEADLINE_LINE: f32 = 32.0;
/// The headline row's gap.
pub(crate) const HERO_HEADLINE_GAP: f32 = 10.0;
/// The title group's gap.
pub(crate) const HERO_TITLE_GAP: f32 = 7.0;
/// The brand mark's width.
pub(crate) const HERO_MARK: f32 = 34.0;
/// The preview badge's size.
pub(crate) const HERO_BADGE_SIZE: f32 = 12.0;
/// The preview badge's line height.
pub(crate) const HERO_BADGE_LINE: f32 = 18.0;
/// The preview badge's side padding.
pub(crate) const HERO_BADGE_PAD_X: f32 = 7.0;
/// The preview badge's top padding.
pub(crate) const HERO_BADGE_PAD_Y: f32 = 1.0;
/// The preview badge's top margin.
pub(crate) const HERO_BADGE_TOP: f32 = 2.0;
/// The clearance the hero stack adds around the composer card.
pub(crate) const HERO_CLEARANCE: f32 = 16.0;
/// The composer stack's gap in the hero phase.
pub(crate) const HERO_GROUP_GAP: f32 = 8.0;
/// The composer stack's bottom padding in the hero phase.
pub(crate) const HERO_GROUP_PB: f32 = 32.0;
/// The workspace row's gap.
pub(crate) const HERO_WS_ROW_GAP: f32 = 2.0;
/// The workspace row's top margin.
pub(crate) const HERO_WS_ROW_TOP: f32 = 4.0;
/// The workspace row's left padding.
pub(crate) const HERO_WS_ROW_PAD_L: f32 = 20.0;
/// The workspace row's right padding.
pub(crate) const HERO_WS_ROW_PAD_R: f32 = 16.0;
/// The workspace chip's minimum height.
pub(crate) const HERO_WS_MIN_H: f32 = 28.0;
/// The workspace chip's side padding.
pub(crate) const HERO_WS_PAD_X: f32 = 8.0;
/// The workspace chip's gap.
pub(crate) const HERO_WS_GAP: f32 = 4.0;
/// The workspace chip's label size.
pub(crate) const HERO_WS_SIZE: f32 = 13.0;
/// The workspace chip's label line height.
pub(crate) const HERO_WS_LINE: f32 = 20.0;
/// The workspace chip's maximum width.
pub(crate) const HERO_WS_MAX_W: f32 = 360.0;
/// The workspace chip's folder glyph.
pub(crate) const HERO_WS_ICON: f32 = 16.0;
/// The workspace chip's chevron.
pub(crate) const HERO_WS_CHEVRON: f32 = 12.0;

/// The meter's three composition tints. The reference draws these from its
/// `static` palette, which no theme step moves, so they are fixed here too.
pub(crate) const METER_SYSTEM: u32 = 0xADB2B8;
pub(crate) const METER_TOOLS: u32 = 0xA78BFA;
pub(crate) const METER_MESSAGES: u32 = 0x4D93F8;

/// The to-do panel's inner side inset.
///
/// Measured: the panel is its container less `2 * (side-clearance + 2 *
/// dock-inset)` = 64px, so it sits 32px inside the composer card's edges.
pub(crate) const TODO_SIDE_INSET: f32 = 32.0;
/// The panel's corner radius (`radius-lg`).
pub(crate) const TODO_RADIUS: f32 = 16.0;
/// The panel body's padding and column gap.
pub(crate) const TODO_PAD_X: f32 = 12.0;
pub(crate) const TODO_PAD_Y: f32 = 6.0;
pub(crate) const TODO_GAP: f32 = 8.0;
/// The header's icon-to-label gap, and the item row's glyph gap.
pub(crate) const TODO_HEAD_GAP: f32 = 10.0;
/// The title's size and line box (13/24 at weight 500).
pub(crate) const TODO_TITLE: f32 = 13.0;
pub(crate) const TODO_TITLE_LINE: f32 = 24.0;
/// The progress summary's and the item rows' line box (13/20).
pub(crate) const TODO_LINE: f32 = 20.0;
/// The item row's glyph slot, the dot it may paint, and its core.
pub(crate) const TODO_GLYPH: f32 = 16.0;
pub(crate) const TODO_DOT: f32 = 10.0;
pub(crate) const TODO_DOT_CORE: f32 = 6.0;
/// The running step's spinner diameter.
pub(crate) const TODO_SPINNER: f32 = 14.0;
/// The spinner's stroke and radius, scaled from the reference's 24-unit
/// viewBox: a 9.5 radius with a 2-wide stroke, drawn at 14px.
pub(crate) const TODO_SPINNER_STROKE: f32 = 1.17;
pub(crate) const TODO_SPINNER_RADIUS: f32 = 5.54;
/// The header chevron.
pub(crate) const TODO_CHEVRON: f32 = 14.0;
/// The expanded list's cap before it scrolls.
pub(crate) const TODO_LIST_MAX_H: f32 = 180.0;
/// The gap between the composer stack's children (`--dsh-composer-stack-gap`).
pub(crate) const COMPOSER_STACK_GAP: f32 = 6.0;

/// The queue dock's own metrics, measured from QueueDock.module.css.
///
/// The dock box is the card less `--dsh-composer-side-clearance` twice (32) and
/// `--dsh-composer-dock-inset` twice (16), and the panel it pads another 16, so
/// the visible surface is 64 narrower than the card - the same inset the to-do
/// panel uses.
pub(crate) const QUEUE_SIDE_INSET: f32 = 32.0;
pub(crate) const QUEUE_RADIUS: f32 = 16.0;
/// The panel's own vertical padding (2px 0).
pub(crate) const QUEUE_PAD_Y: f32 = 2.0;
/// The count header: a 36-tall button padded 4/12 at gap 10.
pub(crate) const QUEUE_HEAD_H: f32 = 36.0;
pub(crate) const QUEUE_HEAD_PAD_X: f32 = 12.0;
pub(crate) const QUEUE_HEAD_PAD_Y: f32 = 4.0;
pub(crate) const QUEUE_HEAD_GAP: f32 = 10.0;
/// The count's 13/24 line at weight 500, and the header chevron.
pub(crate) const QUEUE_COUNT: f32 = 13.0;
pub(crate) const QUEUE_COUNT_LINE: f32 = 24.0;
pub(crate) const QUEUE_CHEVRON: f32 = 14.0;
/// The header's and a lone row's leading queue glyph.
pub(crate) const QUEUE_GLYPH: f32 = 16.0;
/// A row: 36 minimum, radius 12, padded 4/5/4/12 at gap 10.
pub(crate) const QUEUE_ROW_MIN_H: f32 = 36.0;
pub(crate) const QUEUE_ROW_RADIUS: f32 = 12.0;
pub(crate) const QUEUE_ROW_GAP: f32 = 10.0;
pub(crate) const QUEUE_ROW_PAD_L: f32 = 12.0;
pub(crate) const QUEUE_ROW_PAD_R: f32 = 5.0;
pub(crate) const QUEUE_ROW_PAD_Y: f32 = 4.0;
/// The list's cap before it scrolls, and the preview's character budget.
pub(crate) const QUEUE_LIST_MAX_H: f32 = 180.0;
pub(crate) const QUEUE_PREVIEW_CHARS: usize = 200;
/// A row action: a 28px round hit box at gap 10 holding a 14px glyph.
pub(crate) const QUEUE_ACTION: f32 = 28.0;
pub(crate) const QUEUE_ACTION_GAP: f32 = 10.0;
pub(crate) const QUEUE_ACTION_GLYPH: f32 = 14.0;
/// The row editor's own box: min-height 28, radius 6, padded 3/8.
pub(crate) const QUEUE_EDITOR_MIN_H: f32 = 28.0;
/// The queue editor's ceiling, measured: `.editor{max-height:127px}`, with the
/// wrapper scrolling past it like the composer's own.
pub(crate) const QUEUE_EDITOR_MAX_H: f32 = 127.0;
/// How many lines the queue editor draws before that 127px ceiling governs.
pub(crate) const QUEUE_EDITOR_MAX_ROWS: usize = 5;
pub(crate) const QUEUE_EDITOR_RADIUS: f32 = 6.0;
pub(crate) const QUEUE_EDITOR_PAD_X: f32 = 8.0;
/// How far the dock tucks its bottom edge under the composer card.
pub(crate) const QUEUE_OVERLAP: f32 = 3.0;
/// The dock's bottom margin: the reference's `calc(0 - stack-gap - 3px)`, which
/// pulls the card up over the panel's square bottom corners.
pub(crate) const QUEUE_BOTTOM_MARGIN: f32 = -(QUEUE_OVERLAP + COMPOSER_STACK_GAP);

/// A completed step's dot.
///
/// Measured: `state-success-primary` is the static `green-500` (#22C55E),
/// which no theme step moves, so it is fixed here like the meter tints.
pub(crate) const TODO_DONE: u32 = 0x22C55E;

/// A pending step's dot.
///
/// Measured: `state-idle-primary` is `static-neutral-300` (#D4D4D4) in light
/// and `-600` (#545557) in dark.
pub(crate) fn todo_idle() -> u32 {
    if is_dark() {
        0x545557
    } else {
        0xD4D4D4
    }
}

/// The hero preview badge's fill.
///
/// Measured: the reference's `state-business-tertiary` is
/// `--dsw-static-deepseek-100` (#E4EDFD) in light and `-800` (#34415B) in
/// dark.
pub(crate) fn hero_badge_bg() -> u32 {
    if is_dark() {
        0x34415B
    } else {
        0xE4EDFD
    }
}

/// The hero preview badge's text.
///
/// Measured: `label-primary-bluish` is `--dsw-static-blue-900` (#0E3074) in
/// light and `--dsw-static-neutral-bluish-50` (#F9FAFB) in dark.
pub(crate) fn hero_badge_fg() -> u32 {
    if is_dark() {
        0xF9FAFB
    } else {
        0x0E3074
    }
}

/// The hero preview badge's `.5px` ring.
///
/// Measured: `interactive-bg-hover`, a 6% ink overlay (`#2631480f` over the
/// page), which GPUI can only draw as an opaque mix.
pub(crate) fn hero_badge_border() -> u32 {
    blend(hero_badge_fg(), hero_badge_bg(), 0.94)
}

/// A caption glyph's colour.
///
/// Measured: `label-caption` is `--dsw-static-neutral-bluish-400` (#ADB2B8)
/// in light and `-600` (#81858C) in dark.
pub(crate) fn caption() -> u32 {
    if is_dark() {
        0x81858C
    } else {
        0xADB2B8
    }
}

/// A stat pill's text size: the reference's `secondary - 1px`.
pub(crate) fn stat_size_at(px: u32) -> f32 {
    small_size_at(px) - 1.0
}

/// A stat pill's line height: the reference's `20px + secondary delta`.
pub(crate) fn stat_line_at(px: u32) -> f32 {
    20.0 + (small_size_at(px) - 13.0)
}

/// The reference's fixed code-block text size, which no content step moves.
pub(crate) const CODE_BLOCK_SIZE: f32 = 11.0;
/// The reference's fixed code line height.
pub(crate) const CODE_LINE: f32 = 19.0;

/// Whether the applied palette is dark, read back from the page colour.
///
/// The kit owns the mode and BSH keeps no copy of it; the page colour is the
/// token that always tracks it (BSH Light is #FFFFFF, BSH Dark #0A0A0A).
pub(crate) fn is_dark() -> bool {
    let page = page();
    let sum = ((page >> 16) & 0xFF) + ((page >> 8) & 0xFF) + (page & 0xFF);
    sum < 3 * 128
}
/// The reference's sidebar store default: AppFrame keeps layoutInfo.sidebar
/// and falls back to 280 when it holds none, so this is a default and not a
/// fixed width (a drag replaces it).
pub(crate) const SIDEBAR_W: f32 = 280.0;

/// The contract range of the reference's own setSidebar,
/// clampWidth(px, 264, 420), so a drag cannot leave the column outside it.
pub(crate) const SIDEBAR_MIN: f32 = 264.0;
pub(crate) const SIDEBAR_MAX: f32 = 420.0;

/// The reference's sidebar DragHandle: an 8px col-resize strip with
/// margin-left:-4px, so it straddles the seam.
pub(crate) const SIDEBAR_HANDLE_W: f32 = 8.0;

/// The reference's SIDEBAR_AUTO_COLLAPSE: below this viewport width the
/// sidebar folds to its collapsed width unless the reader expanded it over
/// the squeezed centre. The comparison is strict, so exactly 1024 is not narrow.
pub(crate) const SIDEBAR_AUTO_COLLAPSE: f32 = 1024.0;

/// Whether the sidebar is folded, exactly as the reference resolves it:
/// narrow ? !narrowExpanded : storedCollapsed. The stored flag is the
/// layoutInfo.sidebar === 0 sentinel and the narrow flag is runtime only, so a
/// window that widens again falls back to what the reader stored.
pub(crate) fn sidebar_folded(viewport: f32, stored_collapsed: bool, narrow_expanded: bool) -> bool {
    if viewport < SIDEBAR_AUTO_COLLAPSE {
        !narrow_expanded
    } else {
        stored_collapsed
    }
}

/// The sidebar column width: the dragged preference, or the reference's 280
/// default, clamped and rounded exactly as clampWidth does. The reference's 0
/// sentinel means collapsed and is not a width, so bsh keeps collapse in
/// AppState::sidebar_collapsed instead.
pub(crate) fn resolve_sidebar_width(stored: Option<f32>) -> f32 {
    stored
        .unwrap_or(SIDEBAR_W)
        .round()
        .clamp(SIDEBAR_MIN, SIDEBAR_MAX)
}

/// The floor of the reference's chat column, centred in the main area.
///
/// Measured: the column is `clamp(680px, conversation-column * 0.64, 920px)`
/// where the conversation column is the viewport less the sidebar; 680 is its
/// floor, the width bsh draws before a frame has been measured. The live width
/// is `chat_width_at`.
pub(crate) const CHAT_W: f32 = 680.0;
/// The cap of that clamp, measured 920px.
pub(crate) const CHAT_MAX_W: f32 = 920.0;
/// The conversation column's measured share of the chat width, `.64`.
pub(crate) const CHAT_SHARE: f32 = 0.64;

/// The reference's own chat content width, straight from its sheet:
/// `--dsh-chat-content-width: var(--dsh-chat-user-width, clamp(680px,
/// calc(var(--dsh-conversation-column-width, 0px) * .64), 920px))`, with the
/// composer card at `calc(that + 32px)` ([COMPOSER_WIDEN]). A column that has
/// not been measured yet is zero, which lands on the floor - the width bsh drew
/// before this was live.
pub(crate) fn chat_width_at(conversation_w: f32) -> f32 {
    (conversation_w * CHAT_SHARE).clamp(CHAT_W, CHAT_MAX_W)
}

/// The floor a DRAGGED content width takes, measured 640. The clamp's own floor
/// is 680, so a drag may pull the column 40px narrower than the default.
pub(crate) const CONTENT_MIN_DRAG: f32 = 640.0;
/// The room the reference reserves at each edge for the width handles and
/// their safe zones, measured 176: a dragged width never exceeds column - 176.
pub(crate) const CONTENT_EDGE_BUDGET: f32 = 176.0;
/// The width handle's own box, measured min(10px, (column - content) / 2 - 48).
pub(crate) const HANDLE_STRIP_W: f32 = 10.0;
/// The measured clearance between the content column's edge and its handle.
pub(crate) const HANDLE_OFFSET: f32 = 24.0;

/// The reference's own resolveContentWidth(columnWidth, preference):
/// const max = Math.max(CONTENT_MIN, columnWidth - CONTENT_EDGE_BUDGET); if
/// (preference !== null) return Math.min(Math.max(preference, CONTENT_MIN), max);
/// return Math.max(680, Math.min(columnWidth * .64, 920)) - a stored
/// preference wins over the clamp, and with none the column is
/// [chat_width_at].
pub(crate) fn resolve_content_width(column: f32, preference: Option<f32>) -> f32 {
    let max = (column - CONTENT_EDGE_BUDGET).max(CONTENT_MIN_DRAG);
    match preference {
        Some(preference) => preference.clamp(CONTENT_MIN_DRAG, max),
        None => chat_width_at(column),
    }
}

/// The measured handle strip: min(10px, calc((100% - content) / 2 - 24px -
/// 24px)) - the gutter it sits in, capped at ten.
pub(crate) fn handle_width_at(column: f32, content: f32) -> f32 {
    ((column - content) / 2.0 - 2.0 * HANDLE_OFFSET).clamp(0.0, HANDLE_STRIP_W)
}

/// The width handle's bar colour: the reference paints it
/// --dsw-alias-scrollbar-bg-l1, measured as --dsw-static-neutral-200
/// (#e5e5e5) in light and --dsw-static-neutral-700 (#3c3c3d) in dark. RGBA,
/// since the fades need the same colour at zero alpha.
pub(crate) fn scrollbar_bar() -> u32 {
    if is_dark() {
        0x3C3C3DFF
    } else {
        0xE5E5E5FF
    }
}

/// The measured bar: a 2px column, 72 tall, whose solid 16px band sits
/// between two 28px fades - the reference's transparent calc(y - 36px) ->
/// scrollbar-bg-l1 calc(y - 8px) -> scrollbar-bg-l1 calc(y + 8px) ->
/// transparent calc(y + 36px).
pub(crate) const HANDLE_BAR_W: f32 = 2.0;
pub(crate) const HANDLE_BAR_H: f32 = 72.0;
pub(crate) const HANDLE_BAR_FADE: f32 = 28.0;
pub(crate) const HANDLE_BAR_BAND: f32 = 16.0;
/// The session row height.
pub(crate) const ROW_H: f32 = 44.0;
/// The transcript side inset.
pub(crate) const INSET: f32 = 64.0;
/// Copy the active kit theme into the token accessors.
pub(crate) fn sync(cx: &App) {
    let t = cx.theme();
    let fg = t.foreground;
    let bg = t.background;
    put(&PAGE_C, t.background);
    put(&SIDEBAR_BG_C, t.sidebar);
    put(&SELECTED_ROW_C, t.sidebar_accent);
    put(&NEW_SESSION_FILL_C, t.secondary);
    put(&HOVER_C, t.secondary_hover);
    put(&TEXT_C, fg);
    put(&MUTED_C, t.muted_foreground);
    put(&FAINT_C, t.muted_foreground);
    put(&BORDER_SOFT_C, t.border);
    put_u32(&BORDER_C, mix(fg, bg, 0.16));
    put_u32(&BORDER_STRONG_C, mix(fg, bg, 0.32));
    put(&ACCENT_C, t.primary);
    put(&ERROR_C, t.danger);
    put(&SUCCESS_C, t.success);
    put(&WARN_C, t.warning);
    put(&USER_BUBBLE_C, t.secondary);
}
/// Switch between the kit's light and dark palettes.
pub(crate) fn toggle(window: &mut Window, cx: &mut App) {
    let next = if cx.theme().mode.is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(next, Some(window), cx);
    sync(cx);
}
/// Resolve a saved mode name to "light" or "dark".
///
/// "system" asks the kit for the system appearance and keeps the answer; every
/// other name is already one of the two. A system that reports nothing leaves
/// the light default in place rather than guessing.
fn resolve(name: &str, window: Option<&mut Window>, cx: &mut App) -> String {
    if name != "system" {
        return name.to_string();
    }
    Theme::sync_system_appearance(window, cx);
    cx.theme().mode.name().to_string()
}

/// The colour mode a saved name names.
fn theme_mode(name: &str) -> ThemeMode {
    if name == "dark" {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}
/// Apply a saved mode name ("light" or "dark") and refresh the tokens.
pub(crate) fn apply(name: &str, window: &mut Window, cx: &mut App) {
    let resolved = resolve(name, Some(&mut *window), cx);
    // Create the theme global before its configs are swapped, then apply
    // them: change reloads the theme of the current mode even when the mode
    // did not change, which is what makes the swap take effect.
    Theme::change(theme_mode(&resolved), Some(&mut *window), cx);
    install(cx);
    Theme::change(theme_mode(&resolved), Some(&mut *window), cx);
    sync(cx);
}
/// Apply a saved mode name when the caller has no window (the settings pane).
pub(crate) fn apply_here(name: &str, cx: &mut App) {
    let resolved = resolve(name, None, cx);
    Theme::change(theme_mode(&resolved), None, cx);
    install(cx);
    Theme::change(theme_mode(&resolved), None, cx);
    sync(cx);
}
/// The BSH theme set: the kit's palettes with BSH's colours layered on.
const THEME_SET: &str = include_str!("assets/bsh-theme.json");
/// Point the kit at the BSH light and dark palettes.
///
/// The set is derived from the kit's own themes, so every key the kit reads is
/// present and only the colours BSH cares about differ. A set that fails to
/// load leaves the kit defaults in place rather than a half-applied palette;
/// the unit test below keeps the asset parseable.
fn install(cx: &mut App) {
    let pair = {
        let registry = ThemeRegistry::global_mut(cx);
        if registry.load_themes_from_str(THEME_SET).is_err() {
            return;
        }
        let themes = registry.themes();
        match (
            themes.get(&SharedString::from("BSH Light")),
            themes.get(&SharedString::from("BSH Dark")),
        ) {
            (Some(light), Some(dark)) => Some((light.clone(), dark.clone())),
            _ => None,
        }
    };
    if let Some((light, dark)) = pair {
        Theme::update(cx, |theme| {
            theme.light_theme = light;
            theme.dark_theme = dark;
        });
    }
}

/// The active mode name: "light" or "dark".
pub(crate) fn mode(cx: &App) -> &'static str {
    cx.theme().mode.name()
}

fn put_u32(cell: &AtomicU32, value: u32) {
    cell.store(value, Ordering::Relaxed);
}

fn put(cell: &AtomicU32, color: Hsla) {
    let c = Rgba::from(color);
    let ch = |v: f32| ((v.clamp(0.0, 1.0)) * 255.0).round() as u32;
    cell.store(
        (ch(c.r) << 16) | (ch(c.g) << 8) | ch(c.b),
        Ordering::Relaxed,
    );
}

/// Blend two packed colours in RGB space; `t` is the share of `b`.
fn blend(a: u32, b: u32, t: f32) -> u32 {
    mix(rgb(a).into(), rgb(b).into(), t)
}

/// Mix two colours in RGB space; `t` is the share of `b`.
fn mix(a: Hsla, b: Hsla, t: f32) -> u32 {
    let a = Rgba::from(a);
    let b = Rgba::from(b);
    let ch = |x: f32, y: f32| (((x + (y - x) * t).clamp(0.0, 1.0)) * 255.0).round() as u32;
    (ch(a.r, b.r) << 16) | (ch(a.g, b.g) << 8) | ch(a.b, b.b)
}

#[cfg(test)]
mod tests {
    use super::THEME_SET;
    use gpui_kit::component::theme::ThemeSet;

    #[test]
    fn the_content_size_moves_the_conversation_text() {
        assert_eq!(super::body_size_at(18), 18.0, "the body follows the step");
        assert_eq!(
            super::small_size_at(18),
            16.0,
            "flow rows sit under the body"
        );
        assert_eq!(
            super::body_size_at(99),
            22.0,
            "the reference range tops out at 22"
        );
        assert_eq!(super::body_size_at(1), 10.0, "and starts at 10");
        assert_eq!(super::body_size_at(14), 14.0, "the reference default step");
    }

    #[test]
    fn the_measured_type_scale_is_additive() {
        // --dsh-content-font-delta is content - 14px.
        assert_eq!(super::delta_at(14), 0.0, "the default step has no delta");
        assert_eq!(super::delta_at(22), 8.0, "a step moves every size by it");
        // --dsh-content-font-size-secondary = min(size - 1, max(13, size - 2)).
        assert_eq!(super::small_size_at(14), 13.0, "13px at the default");
        assert_eq!(super::small_size_at(22), 20.0, "two under past 15px");
        assert_eq!(super::small_size_at(10), 9.0, "one under below 14px");
        // --dsw-font-markdown-h1..h4 and the body line height.
        assert_eq!(
            super::heading_size_at(14, 1),
            21.0,
            "h1 21px at the default"
        );
        assert_eq!(super::heading_size_at(14, 2), 19.0, "h2 19px");
        assert_eq!(super::heading_size_at(14, 3), 18.0, "h3 18px");
        assert_eq!(super::heading_size_at(14, 4), 14.0, "h4 is the body size");
        assert_eq!(
            super::heading_size_at(22, 1),
            29.0,
            "headings move by delta"
        );
        assert_eq!(super::body_line_at(14), 24.0, "the body line height");
        assert_eq!(super::body_line_at(22), 32.0, "and it moves with the delta");
        assert_eq!(super::table_line_at(22), 29.0, "22px + the secondary delta");
    }

    #[test]
    fn the_theme_set_parses_and_carries_both_modes() {
        let set: ThemeSet = serde_json::from_str(THEME_SET).expect("theme set parses");
        let names: Vec<String> = set.themes.iter().map(|t| t.name.to_string()).collect();
        assert!(names.contains(&"BSH Light".to_string()), "{names:?}");
        assert!(names.contains(&"BSH Dark".to_string()), "{names:?}");
    }
}
