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
use std::cell::Cell;

/// The 16 colour tokens the views read.
///
/// One value per token, kept together so a sync swaps the whole palette in one
/// store instead of leaving half of it on the previous theme.
#[derive(Clone, Copy)]
struct Palette {
    page: u32,
    sidebar_bg: u32,
    selected_row: u32,
    new_session_fill: u32,
    hover: u32,
    text: u32,
    muted: u32,
    faint: u32,
    border_soft: u32,
    border: u32,
    border_strong: u32,
    accent: u32,
    error: u32,
    success: u32,
    warn: u32,
    user_bubble: u32,
}

impl Palette {
    /// The shipped light palette, which is also a thread's initial value.
    const SHIPPED: Self = Self {
        page: 0xFFFFFF,
        sidebar_bg: 0xE8E9EB,
        selected_row: 0xDDDEE1,
        new_session_fill: 0xF2F3F5,
        hover: 0xE1E3E7,
        text: 0x0F1115,
        muted: 0x6B7280,
        faint: 0x8A90A0,
        border_soft: 0xEBEBEB,
        border: 0xD8DBE2,
        border_strong: 0xB3B8C4,
        accent: 0x4176E6,
        error: 0xD23F3F,
        success: 0x2F9E63,
        warn: 0xB8860B,
        user_bubble: 0xF2F3F5,
    };
}

thread_local! {
    /// The palette most recently synced ON THIS THREAD.
    ///
    /// A GPUI App is not Send and paints on one thread, so in the app a thread's
    /// palette is the window's palette. Keeping it per-thread is what makes it
    /// per-CONTEXT for the test binary: cargo runs each case on its own thread,
    /// and a process-global copy let one test's `theme::apply` repaint another
    /// test's frame - the race this file's own test note warns about.
    static PALETTE: Cell<Palette> = const { Cell::new(Palette::SHIPPED) };
}

/// Read one token from the current thread's palette.
macro_rules! palette_token {
    ($name:ident, $field:ident) => {
        /// The colour token, from the palette last synced on this thread.
        pub(crate) fn $name() -> u32 {
            PALETTE.with(|cell| cell.get().$field)
        }
    };
}

palette_token!(page, page);
palette_token!(sidebar_bg, sidebar_bg);
palette_token!(selected_row, selected_row);
palette_token!(new_session_fill, new_session_fill);
palette_token!(hover, hover);
palette_token!(text, text);
palette_token!(muted, muted);
palette_token!(faint, faint);
palette_token!(border_soft, border_soft);
palette_token!(border, border);
palette_token!(border_strong, border_strong);
palette_token!(accent, accent);
palette_token!(error, error);
palette_token!(success, success);
palette_token!(warn, warn);
palette_token!(user_bubble, user_bubble);

/// The reference's plan-chip fill, `--dsw-alias-state-business-tertiary`.
///
/// The kit exposes no "business" state family, so it is derived the way the
/// reference's own token is: the business primary (bsh's accent) mixed 12% into
/// the page. The chip's text is the business primary itself.
pub(crate) fn accent_soft() -> u32 {
    blend(accent(), page(), 0.88)
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
/// The status-dot column every session row leads with.
///
/// The live rows put their streaming dot here and archived rows a same-width
/// spacer, so both kinds start their title at the same x. A row that skipped the
/// column (the archived list did) reads as a different rank of thing, which is
/// not what "archived" means - it is the same session, put away.
pub(crate) const TREE_ROW_LEAD: f32 = 6.0;

/// The gap the workspace tree keeps below its last row.
///
/// A scroller whose content ends exactly on the pinned floor's divider renders the
/// last row FLUSH against that line, which reads as the row being sliced by the
/// pinned element. The reference leaves the list a little air at the bottom; this
/// is that air.
pub(crate) const TREE_BOTTOM_INSET: f32 = 32.0;

/// The brand mark's BOX: the row height the reference measures (32).
///
/// The mantis-shrimp emblem is square and carries its own padding, so drawing it
/// at 32 left it looking smaller than the reference's wide fish glyph. The box
/// keeps the measured row and the emblem is drawn larger inside it, centred - a
/// child that overflows its box adds no layout height, which is what lets the
/// artwork take the optical size without moving the headline.
pub(crate) const HERO_MARK: f32 = 32.0;

/// The size the emblem itself is drawn at inside [`HERO_MARK`].
pub(crate) const HERO_MARK_DRAWN: f32 = 40.0;
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

/// The left pad the header needs while the sidebar is folded, so the folded
/// state's two controls clear the window's traffic lights, which sit at x=14
/// and run about 52px wide. The sidebar's own strip uses the same figure for
/// the same reason.
pub(crate) const SIDEBAR_LIGHT_PAD: f32 = 76.0;

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

/// The right panel's store default width.
///
/// The reference's panel is wider than the sidebar because it holds an editor
/// beside a file list, and its own default lands at this figure; a drag
/// replaces it the same way.
pub(crate) const RIGHT_PANEL_W: f32 = 480.0;

/// The contract range a right-panel drag cannot leave.
pub(crate) const RIGHT_PANEL_MIN: f32 = 320.0;
pub(crate) const RIGHT_PANEL_MAX: f32 = 760.0;

/// The panel's own width: the dragged preference, or the default, clamped and
/// rounded the way the sidebar's is.
pub(crate) fn resolve_right_panel_width(stored: Option<f32>) -> f32 {
    stored
        .unwrap_or(RIGHT_PANEL_W)
        .round()
        .clamp(RIGHT_PANEL_MIN, RIGHT_PANEL_MAX)
}

/// The share of the viewport the panel is allowed to eat, so a maximised
/// panel still leaves the transcript readable beside it.
pub(crate) const RIGHT_PANEL_MAX_SHARE: f32 = 0.5;

/// The resolved panel width for a viewport: the stored width, or the larger
/// of the panel default and half the viewport when the reader asked for the
/// panel to fill what it can.
///
/// The share is of what is LEFT for the panel, which the caller has already
/// taken the sidebar out of. Capping against the WINDOW let the panel and the
/// sidebar add up to more than the window - a 480 panel and a 280 sidebar
/// inside a 700 window overflowed the row on the right, so the panel's own
/// right edge (and the editor drawn to it) landed off screen. The share still
/// means what it meant: the panel never eats more than half of its own space.
pub(crate) fn right_panel_width_at(available: f32, stored: Option<f32>) -> f32 {
    resolve_right_panel_width(stored).min((available * RIGHT_PANEL_MAX_SHARE).max(RIGHT_PANEL_MIN))
}

/// The right panel's row height, matching the sidebar's own rows so a
/// switching reader never re-measures the list.
pub(crate) const RIGHT_ROW_H: f32 = 28.0;

/// The Start card's measured min-height: the reference's own `min-height:56px`
/// on its guide entry, which is what sets the card's box before its 14/20
/// padding adds to it.
pub(crate) const RIGHT_CARD_H: f32 = 56.0;

/// The Start card's measured width. The reference pins its guide entry to
/// 380px inside a centred column, so the cards read as a stack of plates
/// rather than as rows filling whatever the panel happens to be.
pub(crate) const RIGHT_CARD_W: f32 = 380.0;

/// The panel's header and tab-bar height.
pub(crate) const RIGHT_BAR_H: f32 = 40.0;

/// The padding the panel draws around its own body.
pub(crate) const RIGHT_PAD: f32 = 12.0;

/// The mono size the Start cards draw their shortcut hint at, matching the
/// reference's own 11px description and hint figures.
pub(crate) const RIGHT_HINT_SIZE: f32 = 11.0;

/// The 38px header height the reference's own FilesBody measures.
pub(crate) const FILES_HEADER_H: f32 = 38.0;

/// The floor of the reference's chat column, centred in the main area.
///
/// Measured: the column is `clamp(680px, conversation-column * 0.64, 920px)`
/// where the conversation column is the viewport less the sidebar; 680 is its
/// floor, the width bsh draws before a frame has been measured. The live width
/// How far a step row sits in from the message it belongs to, and how tall its
/// expanded body may get before it clips.
///
/// The trace nests: a message's steps hang off a rail one step in, and their
/// bodies hang one more. 18 is that step at the secondary size.
/// How close to the transcript's top a reader gets before the window above it
/// is paged in. A screen's worth of distance, so the rows are there before the
/// eye arrives.
/// Windows of messages a paged session keeps resident.
///
/// Four is a screen or two in each direction around the reader: enough that
/// scrolling back finds what they just passed, far short of a whole chat.
pub(crate) const WINDOWS_KEPT: usize = 4;

pub(crate) const PAGE_TRIGGER: f32 = 400.0;

pub(crate) const TOOL_INDENT: f32 = 18.0;
/// The ceiling on an expanded step body, so one long output cannot own the
/// transcript.
pub(crate) const TOOL_BODY_MAX: f32 = 220.0;

/// is `chat_width_at`.
pub(crate) const CHAT_W: f32 = 680.0;
/// The empty band the column keeps on each side of the transcript.
///
/// The reference has no such constant - its clamp leaves whatever .64 and a
/// 920 cap imply, which on a wide window is a quarter of the page each side.
/// A fixed 32 per side is what a reader needs to see that the transcript
/// ENDS, and the drag handles still move the edge.
pub(crate) const CHAT_GUTTER: f32 = 32.0;
/// The conversation column's share of the chat width.
///
/// The reference takes .64; .9 gives the transcript nearly the whole column,
/// because the empty band either side was the first thing a wide window
/// showed and it read as a layout fault rather than as whitespace.
pub(crate) const CHAT_SHARE: f32 = 0.9;

/// How much of the left band the transcript keeps, measured against the band
/// the right side still holds.
///
/// The chat is not centred any more: the right edge stays where the turn rail
/// and the composer expect it, and the left edge moves out to 40% of the band,
/// because the empty strip beside the text was the widest thing on a wide
/// window and only the LEFT side can give without moving the composer.
pub(crate) const CHAT_LEFT_SHARE: f32 = 0.4;

/// The left inset the transcript keeps for a column and a chat width.
pub(crate) fn chat_left_inset(column: f32, chat: f32) -> f32 {
    // Rounded to a whole pixel: the handle parks 24 outside this edge, and a
    // fractional inset painted as a snapped pad put the strip 1.2px off it.
    (((column - chat) / 2.0).max(0.0) * CHAT_LEFT_SHARE).round()
}

/// How far the token readout sits left of centre, as a share of the half-width
/// of the column it lives in.
pub(crate) fn chat_width_at(conversation_w: f32) -> f32 {
    // The share fills the column; the gutter is the only ceiling, so a wide
    // window cannot leave the bands the reference clamp leaves.
    let ceiling = (conversation_w - 2.0 * CHAT_GUTTER).max(CHAT_W);
    let ideal = (conversation_w * CHAT_SHARE).clamp(CHAT_W, ceiling);
    // A column of zero is not a narrow column, it is a frame that has not been
    // measured yet, and that keeps the floor it has always had.
    if conversation_w <= 0.0 {
        return ideal;
    }
    // Otherwise the floor exists so the chat is READABLE, not so it overflows
    // the column it sits in. An open right panel takes a laptop's column below
    // 680, and the floor then hung the composer and the hero over the panel's
    // edge - clipped, half their text unreadable. The composer card is this
    // width plus the measured widen, so the cap leaves that much room.
    ideal.min((conversation_w - COMPOSER_WIDEN).max(0.0))
}

/// The floor a DRAGGED content width takes, measured 640. The clamp's own floor
/// is 680, so a drag may pull the column 40px narrower than the default.
pub(crate) const CONTENT_MIN_DRAG: f32 = 640.0;
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
    // The drag tops out at the same gutter the default width keeps, so the two
    // rules cannot disagree: a default that the drag bound would then shorten
    // made the first pointer move pull the column IN, which is what a 176 budget
    // beside a .9 share did.
    let max = (column - 2.0 * CHAT_GUTTER).max(CONTENT_MIN_DRAG);
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

/// The wash behind an added line of a step's patch.
///
/// A diff tint has to stay legible over the page in both palettes, so it is the
/// success tone at low alpha rather than a hard fill.
pub(crate) fn diff_add() -> u32 {
    let rgb = success() & 0x00FF_FFFF;
    (rgb << 8) | 0x24
}

/// The wash behind a removed line of a step's patch.
pub(crate) fn diff_del() -> u32 {
    let rgb = error() & 0x00FF_FFFF;
    (rgb << 8) | 0x24
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

/// The code face.
///
/// Measured from the reference's own stack (`toolCallNameTypeface`):
/// `Menlo, Consolas, Liberation Mono, PingFang SC, Microsoft YaHei`, exposed as
/// `--ds-font-family-code`. A tool call and its result are written in it, the
/// way the reference renders those rows.
pub(crate) const CODE_FONT: &str = "Menlo";
/// Copy the active kit theme into this thread's token block.
pub(crate) fn sync(cx: &App) {
    let t = cx.theme();
    let fg = t.foreground;
    let bg = t.background;
    PALETTE.with(|cell| {
        cell.set(Palette {
            page: hx(t.background),
            sidebar_bg: hx(t.sidebar),
            selected_row: hx(t.sidebar_accent),
            new_session_fill: hx(t.secondary),
            hover: hx(t.secondary_hover),
            text: hx(fg),
            muted: hx(t.muted_foreground),
            faint: hx(t.muted_foreground),
            border_soft: hx(t.border),
            border: mix(fg, bg, 0.16),
            border_strong: mix(fg, bg, 0.32),
            accent: hx(t.primary),
            error: hx(t.danger),
            success: hx(t.success),
            warn: hx(t.warning),
            user_bubble: hx(t.secondary),
        });
    });
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

fn hx(color: Hsla) -> u32 {
    let c = Rgba::from(color);
    let ch = |v: f32| ((v.clamp(0.0, 1.0)) * 255.0).round() as u32;
    (ch(c.r) << 16) | (ch(c.g) << 8) | ch(c.b)
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

    /// A palette synced on one thread cannot repaint another.
    ///
    /// The tokens were 16 process-global atomics, and cargo runs every case on
    /// its own thread, so one case's `theme::apply` rewrote the colours of a
    /// frame another case was asserting on. Each thread now carries its own
    /// block, which is the test binary's per-context value and the app's real
    /// value too (a GPUI App paints on one thread).
    #[test]
    fn the_palette_is_per_thread() {
        let shipped = super::Palette::SHIPPED.page;
        super::PALETTE.with(|cell| {
            let mut palette = super::Palette::SHIPPED;
            palette.page = 0x010203;
            cell.set(palette);
        });
        let other = std::thread::spawn(super::page).join().expect("the thread");
        assert_eq!(super::page(), 0x010203, "this thread keeps its own palette");
        assert_eq!(other, shipped, "a new thread starts from the shipped one");
    }

    #[test]
    fn the_theme_set_parses_and_carries_both_modes() {
        let set: ThemeSet = serde_json::from_str(THEME_SET).expect("theme set parses");
        let names: Vec<String> = set.themes.iter().map(|t| t.name.to_string()).collect();
        assert!(names.contains(&"BSH Light".to_string()), "{names:?}");
        assert!(names.contains(&"BSH Dark".to_string()), "{names:?}");
    }
}
