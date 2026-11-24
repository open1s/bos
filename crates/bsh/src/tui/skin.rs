//! The terminal's painting vocabulary: what a row is *about*, and how it is drawn.
//!
//! The desktop paints the kit's hex tokens ([`crate::theme`]); a terminal must
//! not, because its background belongs to the reader's profile rather than to
//! BSH's page. So the hues here are the sixteen ANSI slots that profile already
//! defines, and every distinct meaning also carries a shape - a bold label, dim
//! metadata, a reversed selection - so a terminal without colour loses the hue
//! and nothing else.
//!
//! [`Painter`] is the one place a row of text becomes styled spans. The markers
//! it classifies ([`MARKERS`]) are this client's own: a transcript row always
//! begins with the prefix, glyph or verdict the client wrote for it, never with
//! text a model or a tool printed, because every wrapped row writes its prefix
//! first. That is what makes "which rows are a tool call" a closed question
//! answered once here instead of guessed at each call site.
//!
//! The footer's own vocabulary lives here too ([`Footer`], [`footer_text`]): a
//! frame gives it one row, so it spends that row on the keys the reader needs
//! in the state they are in, and drops a group rather than cutting one in half.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// What one row of the frame is about.
///
/// These are meanings, not colours: [`Skin::style`] is the only place a tone
/// becomes a hue, so a theme change never has to walk the renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Assistant prose, pane bodies: the terminal's own foreground.
    Body,
    /// The frame's header and a document's title row.
    Heading,
    /// A label that names the row it opens: the composer's prompt glyph.
    Accent,
    /// Metadata, rules and hints: readable, never competing with content.
    Muted,
    /// The reader's own turn.
    User,
    /// A verdict that succeeded.
    Ok,
    /// Something is waiting: a running call, a pending question.
    Warn,
    /// Something failed, and the text beside it failed.
    Error,
    /// A row a patch added.
    Add,
    /// A row a patch removed.
    Remove,
    /// The row the reader is on. Reversed, so it survives a monochrome screen.
    Select,
}

/// Whether this terminal takes colour, and what each [`Tone`] draws in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Skin {
    colour: bool,
}

impl Skin {
    /// Detect the reader's preferences once, before the first frame.
    ///
    /// `NO_COLOR` wins over everything: its own rule is that any non-empty
    /// value asks for no colour. A `dumb` terminal draws escape sequences as
    /// text, so it is asked for none either. No other probe is needed - this
    /// client paints named ANSI slots, which every terminal that is not dumb
    /// already has, and asking for true colour would only be a guess about the
    /// reader's background.
    pub(crate) fn detect() -> Self {
        let no_color =
            std::env::var_os("NO_COLOR").map(|value| value.to_string_lossy().into_owned());
        let term = std::env::var_os("TERM").map(|value| value.to_string_lossy().into_owned());
        Self::from_environment(no_color.as_deref(), term.as_deref())
    }

    /// The plain skin: no hue, every shape kept. What a test draws with; the
    /// frame itself always asks [`Skin::detect`] what the profile accepts.
    #[cfg(test)]
    pub(crate) fn plain() -> Self {
        Self { colour: false }
    }

    /// Resolve the two answers a capability probe has: whether the reader
    /// asked for no colour, and what the terminal calls itself.
    fn from_environment(no_color: Option<&str>, term: Option<&str>) -> Self {
        let refused = no_color.is_some_and(|value| !value.trim().is_empty())
            || term.is_some_and(|term| term.trim().eq_ignore_ascii_case("dumb"));
        Self { colour: !refused }
    }

    /// The style one tone draws in.
    pub(crate) fn style(&self, tone: Tone) -> Style {
        let plain = Style::default();
        let bold = plain.add_modifier(Modifier::BOLD);
        // A named slot plus bold is the bright form of that slot on every
        // xterm-compatible terminal, which is what keeps the accent legible on
        // a dark background without pinning a hex value.
        let hue = |colour: Color, base: Style| {
            if self.colour { base.fg(colour) } else { base }
        };
        match tone {
            Tone::Body => plain,
            Tone::Heading => hue(Color::Blue, bold),
            Tone::Accent => hue(Color::Blue, bold),
            Tone::Muted => plain.add_modifier(Modifier::DIM),
            Tone::User => hue(Color::Cyan, bold),
            Tone::Ok => hue(Color::Green, bold),
            Tone::Warn => hue(Color::Yellow, bold),
            Tone::Error => hue(Color::Red, bold),
            Tone::Add => hue(Color::Green, plain),
            Tone::Remove => hue(Color::Red, plain),
            // Reverse and bold are the two attributes that mean "current row"
            // in every terminal, and neither depends on colour at all.
            Tone::Select => plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
        }
    }
}

/// The leading markers this client writes, with the tones they claim.
///
/// `(marker, head, rest)`: the marker's own span, then the row's remaining
/// span. A marker is matched at column zero only, in this order, and the
/// longest-looking entries come first because each one starts differently.
const MARKERS: &[(&str, Tone, Tone)] = &[
    // Speakers of the timeline. `App::transcript_lines` writes these first,
    // through `wrap`, so message text can never reach column zero.
    ("you › ", Tone::User, Tone::Body),
    ("bsh › ", Tone::Accent, Tone::Body),
    ("checkpoint › ", Tone::Muted, Tone::Muted),
    // A message's fenced block. The corners and the gutter recede so the code
    // between them stays in the terminal's own foreground: the gutter says
    // "this is one block", the code is the content. The bare gutter row is a
    // blank line inside the block, which is still inside it.
    ("╭ ", Tone::Muted, Tone::Muted),
    ("╭", Tone::Muted, Tone::Muted),
    ("╰", Tone::Muted, Tone::Muted),
    ("│ ", Tone::Muted, Tone::Body),
    ("│", Tone::Muted, Tone::Muted),
    // A tool call's verdict glyph, before the call's name.
    ("⚙ ", Tone::Warn, Tone::Body),
    ("✓ ", Tone::Ok, Tone::Body),
    ("✗ ", Tone::Error, Tone::Body),
    // A failed execution or a pending approval: the text is the verdict.
    ("! ", Tone::Error, Tone::Error),
    ("? ", Tone::Warn, Tone::Body),
    ("… ", Tone::Muted, Tone::Muted),
    // A landed patch's changed rows, and the row that counts a folded tail.
    ("  + ", Tone::Add, Tone::Add),
    ("  - ", Tone::Remove, Tone::Remove),
    ("  … ", Tone::Muted, Tone::Muted),
    // The plan block: untouched, being worked on, finished.
    ("plan ", Tone::Heading, Tone::Heading),
    ("  ○ ", Tone::Muted, Tone::Body),
    ("  ● ", Tone::Warn, Tone::Body),
    ("  ✓ ", Tone::Ok, Tone::Body),
    // The queue dock, the question card and its keys.
    ("queued ", Tone::Muted, Tone::Muted),
    ("questions · ", Tone::Heading, Tone::Heading),
    ("▸", Tone::Select, Tone::Select),
    ("↑↓", Tone::Muted, Tone::Muted),
];

/// One frame's painter: the skin, and the tone an indented row continues.
///
/// A wrapped row's continuation is two spaces and the text, so it has no
/// marker of its own. The painter remembers the tone the verdict above it
/// handed down - a failure's rows stay red, a patch's rows stay green - and
/// forgets it when the next row is a whole different part of the document.
pub(crate) struct Painter {
    skin: Skin,
    carry: Tone,
}

impl Painter {
    pub(crate) fn new(skin: Skin) -> Self {
        Self {
            skin,
            carry: Tone::Body,
        }
    }

    /// One document row, with the tone its own marker claims.
    pub(crate) fn row(&mut self, text: &str) -> Line<'static> {
        if let Some((marker, head, rest)) = MARKERS
            .iter()
            .find(|(marker, _, _)| text.starts_with(*marker))
        {
            self.carry = carried(*rest);
            return self.spans(text, marker.chars().count(), *head, *rest);
        }
        if is_rule(text) {
            self.carry = Tone::Body;
            return self.spans(text, 0, Tone::Muted, Tone::Muted);
        }
        if self.carry != Tone::Body && text.starts_with("  ") {
            return self.spans(text, 0, self.carry, self.carry);
        }
        self.carry = Tone::Body;
        Line::from(text.to_string())
    }

    /// One row whose tone the caller already knows: a frame's chrome, a
    /// document's title, a row a pane prints about itself.
    pub(crate) fn tone(&mut self, text: &str, tone: Tone) -> Line<'static> {
        self.carry = Tone::Body;
        self.spans(text, 0, tone, tone)
    }

    /// One row whose leading marker the caller names, when the same text
    /// could be two different things: the composer's `› ` is a prompt, the
    /// menu's `› ` is the row the reader is on.
    pub(crate) fn marker(
        &mut self,
        text: &str,
        marker: &str,
        head: Tone,
        rest: Tone,
    ) -> Line<'static> {
        self.carry = Tone::Body;
        self.spans(text, marker.chars().count(), head, rest)
    }

    /// A row with nothing on it.
    pub(crate) fn blank(&mut self) -> Line<'static> {
        self.carry = Tone::Body;
        Line::from(String::new())
    }

    /// Split `text` after `mark` characters and paint the two spans apart.
    fn spans(&self, text: &str, mark: usize, head: Tone, rest: Tone) -> Line<'static> {
        let head_style = self.skin.style(head);
        let rest_style = self.skin.style(rest);
        if head_style == rest_style && mark == 0 {
            return Line::from(Span::styled(text.to_string(), head_style));
        }
        let at = text
            .char_indices()
            .nth(mark)
            .map_or(text.len(), |(at, _)| at);
        if at == 0 {
            return Line::from(Span::styled(text.to_string(), head_style));
        }
        let (leading, body) = text.split_at(at);
        let mut spans = vec![Span::styled(leading.to_string(), head_style)];
        if !body.is_empty() {
            spans.push(Span::styled(body.to_string(), rest_style));
        }
        Line::from(spans)
    }
}

/// The tone a row hands down to its indented continuation.
///
/// Only the tones that own a whole block carry: a failure's text, a patch's
/// rows. A speaker's label does not, because the prose under it is the body
/// every other row is drawn in.
fn carried(rest: Tone) -> Tone {
    match rest {
        Tone::Error | Tone::Add | Tone::Remove => rest,
        _ => Tone::Body,
    }
}

/// Whether a row is one of the frame's horizontal rules.
fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| character == '─')
}

/// What the footer answers to: the state the reader is in.
///
/// The frame gives the footer one row. Which keys matter then depends entirely
/// on whether a turn is running, a question is waiting, or a pane has taken
/// the body - the same reason a desktop status bar swaps its hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Footer {
    /// Nothing is waiting: the ordinary editor.
    Idle,
    /// An approval is pending, and two chords answer it.
    Approval,
    /// The `ask_user` card is on screen.
    Question,
    /// The completion popup is open.
    Menu,
    /// A turn is running, so a line would be queued.
    Busy,
    /// A pane owns the body until the reader closes it.
    Pane,
}

/// The keys the footer offers, in the order it should spend its row.
///
/// Ordered by how often a reader needs them: the send keys, then the two
/// triggers, then the way out, then everything a reader reaches for once they
/// are at home in the interface. A frame narrower than this row keeps the
/// prefix that fits, so the order is the whole design.
const KEYS: &[&str] = &[
    "Enter send",
    "Shift-Enter newline",
    "/ commands",
    "@ files",
    "^C stop",
    "^D quit",
    "PgUp/PgDn scroll",
    "^N/^P sessions",
    "^T new session",
    "^Y approve",
    "^R reject",
    "Tab plan",
    "^L refresh",
    "^] shell",
];

/// The groups a state puts ahead of [`KEYS`].
fn footer_groups(footer: Footer) -> &'static [&'static str] {
    match footer {
        Footer::Idle => &[],
        // The approval is the frame's own warning line; the footer answers
        // "and what do I press?" without repeating the target.
        Footer::Approval => &["^Y approve", "^R reject"],
        Footer::Question => &[
            "↑↓ walks options",
            "Enter answers",
            "Esc dismisses",
            "typing answers freely",
        ],
        Footer::Menu => &["Tab accepts", "Esc closes"],
        Footer::Busy => &["^C stops the turn", "type to queue a line"],
        Footer::Pane => &["Esc closes", "↑↓ scrolls"],
    }
}

/// The footer for one state, filling `width` cells without cutting a group.
///
/// Groups are dropped from the tail, never split: a row that ends in half a
/// key name would teach the reader nothing and hide the group that was whole.
/// A width too small for even the first group still says the first thing, cut
/// with an ellipsis, because an empty footer is worse than a clipped one.
pub(crate) fn footer_text(footer: Footer, width: usize) -> String {
    let mut parts: Vec<&'static str> = footer_groups(footer).to_vec();
    parts.extend_from_slice(KEYS);
    let mut text = String::new();
    for part in &parts {
        let candidate = if text.is_empty() {
            (*part).to_string()
        } else {
            format!("{text} · {part}")
        };
        if candidate.chars().count() > width {
            break;
        }
        text = candidate;
    }
    if text.is_empty() {
        text = parts.first().copied().unwrap_or("").to_string();
        if width > 0 && text.chars().count() > width {
            text = text.chars().take(width - 1).collect();
            text.push('…');
        }
        return text;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(line: &Line<'_>) -> Style {
        line.spans
            .first()
            .map(|span| span.style)
            .unwrap_or_default()
    }

    #[test]
    fn colour_follows_the_readers_environment() {
        // The spec's own rule: any non-empty value is a refusal, and an empty
        // one is a value the reader forgot to clear.
        assert!(!Skin::from_environment(Some("1"), Some("xterm-256color")).colour);
        assert!(Skin::from_environment(Some(""), Some("xterm-256color")).colour);
        // A dumb terminal would draw the escapes as letters.
        assert!(!Skin::from_environment(None, Some("dumb")).colour);
        assert!(!Skin::from_environment(None, Some("DUMB")).colour);
        assert!(Skin::from_environment(None, Some("xterm-256color")).colour);
        assert!(Skin::from_environment(None, None).colour);
    }

    #[test]
    fn no_colour_keeps_every_shape_the_screen_relies_on() {
        let plain = Skin::plain().style(Tone::Body);
        assert_eq!(plain, Style::default());
        // Meaning survives the hue: a label is bold, metadata is dim, the
        // reader's row is reversed.
        for tone in [Tone::Heading, Tone::User, Tone::Ok, Tone::Warn, Tone::Error] {
            assert!(
                Skin::plain()
                    .style(tone)
                    .add_modifier
                    .contains(Modifier::BOLD),
                "{tone:?} lost its weight without colour"
            );
        }
        assert!(
            Skin::plain()
                .style(Tone::Muted)
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            Skin::plain()
                .style(Tone::Select)
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn every_marker_is_drawn_as_the_row_it_names() {
        let skin = Skin::plain();
        for (marker, head, rest) in MARKERS {
            let mut painter = Painter::new(skin);
            let line = painter.row(&format!("{marker}the row's own text"));
            assert_eq!(first(&line), skin.style(*head), "marker {marker:?} head");
            let second = line.spans.get(1).map(|span| span.style);
            assert_eq!(second, Some(skin.style(*rest)), "marker {marker:?} rest");
        }
    }

    #[test]
    fn a_failures_rows_stay_failed_and_a_label_does_not_carry() {
        let mut painter = Painter::new(Skin::plain());
        painter.row("! failed: the tool refused");
        let continuation = painter.row("  because the path is outside the workspace");
        assert_eq!(
            first(&continuation),
            Skin::plain().style(Tone::Error),
            "a failed execution's indented rows read as the failure"
        );
        // The next block is a title, and the body under it is a body: nothing
        // about a heading is inherited by the rows below it.
        painter.tone("memory · 4 record(s)", Tone::Heading);
        let body = painter.row("  • a remembered fact");
        assert_eq!(first(&body), Style::default());
    }

    #[test]
    fn a_patchs_wrapped_row_keeps_the_change_it_belongs_to() {
        let mut painter = Painter::new(Skin::plain());
        painter.row("  + fn added(a: u8) -> u8 {");
        let wrapped = painter.row("      a.saturating_add(1)");
        assert_eq!(first(&wrapped), Skin::plain().style(Tone::Add));
    }

    #[test]
    fn a_rule_reads_as_no_content_and_a_plain_row_as_prose() {
        let mut painter = Painter::new(Skin::plain());
        let rule = painter.row(&"─".repeat(20));
        assert_eq!(first(&rule), Skin::plain().style(Tone::Muted));
        let prose = painter.row("a sentence the model wrote");
        assert_eq!(first(&prose), Style::default());
    }

    #[test]
    fn the_composer_and_the_menu_can_spend_one_marker_two_ways() {
        let mut painter = Painter::new(Skin::plain());
        let prompt = painter.marker("› a draft", "› ", Tone::Accent, Tone::Body);
        assert_eq!(first(&prompt), Skin::plain().style(Tone::Accent));
        assert_eq!(
            prompt.spans.get(1).map(|span| span.style),
            Some(Style::default())
        );
        let active = painter.marker(
            "› /model  choose a provider",
            "› ",
            Tone::Select,
            Tone::Select,
        );
        assert_eq!(first(&active), Skin::plain().style(Tone::Select));
    }

    #[test]
    fn the_footer_spends_its_one_row_without_cutting_a_group() {
        // An approval leads with its own two chords, so they survive even on a
        // row that fits nothing else.
        let narrow = footer_text(Footer::Approval, 28);
        assert_eq!(narrow, "^Y approve · ^R reject");
        // A wider row keeps adding whole groups and stops before the next one.
        let wide = footer_text(Footer::Idle, 80);
        assert!(wide.chars().count() <= 80);
        assert!(wide.starts_with("Enter send · Shift-Enter newline"));
        assert!(!wide.ends_with(" · "));
        assert!(!wide.ends_with('·'));
        // A row narrower than one group still says the first thing.
        let tiny = footer_text(Footer::Idle, 9);
        assert_eq!(tiny.chars().count(), 9);
        assert!(tiny.ends_with('…'));
    }

    #[test]
    fn every_footer_state_leads_with_the_keys_it_needs() {
        for footer in [
            Footer::Idle,
            Footer::Approval,
            Footer::Question,
            Footer::Menu,
            Footer::Busy,
            Footer::Pane,
        ] {
            let text = footer_text(footer, 120);
            let expectation = footer_groups(footer).first().copied().unwrap_or(KEYS[0]);
            assert!(
                text.starts_with(expectation),
                "{footer:?} begins with {text:?}"
            );
        }
    }
}
