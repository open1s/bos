//! The terminal's painting vocabulary: what a row is *about*, and how it is drawn.
//!
//! The hues are the concept's own (`docs/tui/assets/bsh-tui-workspace.svg`):
//! the navy page, the muted metadata, the four status inks and the violet brand
//! mark, read cell for cell into [`ink`]. A terminal that reports true colour
//! paints exactly those hexes; every other terminal gets the nearest of its own
//! sixteen slots, and every distinct meaning also carries a shape - a bold
//! label, dim metadata, a filled selection - so a terminal without colour loses
//! the hue and nothing else.
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
//!
//! The activity spinner lives here for the same reason the tones do: a row that
//! is *running* is a meaning every renderer has to draw the same way, and the
//! ten braille frames are that meaning's animation.

// The vocabulary is complete before every consumer lands: the shell pane paints
// through this module, and the frame's header/footer rows switch to it one at a
// time. Until they do, part of it is unused. The allow stays scoped to this
// file; code outside it still has to be wired to be accepted.
#![allow(dead_code)]

use std::borrow::Cow;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// The concept's ink, cell for cell.
///
/// These are the mock's own hexes - the navy page, the raised panel, the fill a
/// chosen row wears, the muted metadata and the four status hues - so the
/// terminal and the desktop concept are the same palette rather than two
/// approximations of one idea.
pub(crate) mod ink {
    use ratatui::style::Color;

    /// The page, and the fill a chosen row wears on it.
    pub(crate) const PAGE: Color = Color::Rgb(0x0b, 0x14, 0x22);
    #[allow(dead_code)]
    pub(crate) const PANEL: Color = Color::Rgb(0x0d, 0x17, 0x26);
    pub(crate) const RAISED: Color = Color::Rgb(0x16, 0x3d, 0x68);
    /// Text, and the metadata that recedes beside it.
    pub(crate) const TEXT: Color = Color::Rgb(0xdc, 0xe7, 0xf7);
    pub(crate) const MUTED: Color = Color::Rgb(0x82, 0x94, 0xad);
    /// The four status hues the concept paints with.
    pub(crate) const BLUE: Color = Color::Rgb(0x59, 0xa8, 0xff);
    /// The send button fill from the composer concept.
    pub(crate) const BUTTON: Color = Color::Rgb(0x16, 0x78, 0xe8);
    /// The composer outline from the workspace concept.
    pub(crate) const COMPOSER_BORDER: Color = Color::Rgb(0x28, 0x79, 0xc8);
    pub(crate) const GREEN: Color = Color::Rgb(0x35, 0xd6, 0xa1);
    pub(crate) const AMBER: Color = Color::Rgb(0xff, 0xb4, 0x54);
    /// The brand mark's stroke, and the violet a reader's own turn wears.
    pub(crate) const BRAND: Color = Color::Rgb(0x43, 0xb8, 0xff);
    pub(crate) const VIOLET: Color = Color::Rgb(0x8d, 0x6c, 0xff);
    /// The indigo strokes used by inactive navigation icons.
    pub(crate) const NAV_ICON: Color = Color::Rgb(0x7c, 0x93, 0xff);
    /// The cyan used by the selected navigation icon.
    pub(crate) const NAV_ICON_ACTIVE: Color = Color::Rgb(0x37, 0xcc, 0xe0);
    /// A failure. The concept draws no failing sample, so this is the one hue it
    /// does not name: the same rigour as its amber, one step past it, because a
    /// failure that reads as "waiting" teaches the reader the wrong thing.
    pub(crate) const RED: Color = Color::Rgb(0xff, 0x6b, 0x6b);
}

/// What one row of the frame is about.
///
/// These are meanings, not colours: [`Skin::style`] is the only place a tone
/// becomes a hue, so a theme change never has to walk the renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Assistant prose, pane bodies: the concept's text ink.
    Body,
    /// The frame's header and a document's title row.
    Heading,
    /// A fact that matters - the open session's name, a column's title - in the
    /// concept's text ink at weight. Not the heading, because the frame has
    /// exactly one of those.
    Strong,
    /// The brand mark the frame opens with, in the stroke the mock draws its own
    /// logo in; a reversed block on a terminal that cannot paint the hue.
    Chip,
    /// A label that names the row it opens: the composer's prompt glyph, and the
    /// destination the reader is on.
    Accent,
    /// Metadata, rules and hints: readable, never competing with content.
    Muted,
    /// The reader's own turn, in the violet the mock gives its avatar.
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
    /// The row the reader is on: the mock's raised fill under its text ink, and
    /// the reverse block every terminal has when the fill cannot be painted.
    Select,
    /// The active global tab: blue and underlined, without a selection block.
    TabActive,
    /// The composer's primary send control.
    Button,
    /// The composer's blue outline.
    ComposerBorder,
    /// An inactive destination icon in the sidebar.
    NavIcon,
    /// The active destination icon in the sidebar.
    NavIconActive,
}

/// Whether this terminal takes colour, and what each [`Tone`] draws in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Skin {
    colour: Colour,
}

/// How much colour the reader's terminal was promised.
///
/// The concept paints in hexes, so a terminal that says it takes twenty-four
/// bits paints them. One that only names slots gets the nearest of its sixteen,
/// and one that takes nothing keeps the shapes - which is the same ladder the
/// spec asks for, with the concept's palette at the top of it rather than a
/// guess at the reader's profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    /// `NO_COLOR`, or a terminal that would draw the escapes as text.
    None,
    /// The sixteen slots every profile already defines.
    Ansi,
    /// Twenty-four bits: the concept's own ink.
    True,
}

impl Skin {
    /// Detect the reader's preferences once, before the first frame.
    ///
    /// `NO_COLOR` wins over everything: its own rule is that any non-empty
    /// value asks for no colour. A `dumb` terminal draws escape sequences as
    /// text, so it is asked for none either. Below that, true colour is only
    /// taken when the terminal says so in `COLORTERM` - the one variable the
    /// ecosystem set for exactly this question - so a terminal that says
    /// nothing gets the slots its profile is certain to have.
    pub(crate) fn detect() -> Self {
        let read =
            |name: &str| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned());
        Self::from_environment(
            read("NO_COLOR").as_deref(),
            read("TERM").as_deref(),
            read("COLORTERM").as_deref(),
        )
    }

    /// Apply an explicit per-client theme, falling back to terminal detection.
    pub(crate) fn with_preference(theme: super::prefs::Theme) -> Self {
        use super::prefs::Theme;
        match theme {
            Theme::Auto => Self::detect(),
            Theme::Ansi => Self {
                colour: Colour::Ansi,
            },
            Theme::Mono => Self {
                colour: Colour::None,
            },
            Theme::Truecolor => Self {
                colour: Colour::True,
            },
        }
    }

    /// The design's page surface for the whole frame when this terminal can
    /// paint RGB. ANSI and monochrome profiles keep their chosen background.
    pub(crate) fn page_style(self) -> Style {
        match self.colour {
            Colour::True => Style::default().bg(ink::PAGE),
            Colour::Ansi | Colour::None => Style::default(),
        }
    }

    /// The plain skin: no hue, every shape kept. What a test draws with; the
    /// frame itself always asks [`Skin::detect`] what the profile accepts.
    #[cfg(test)]
    pub(crate) fn plain() -> Self {
        Self {
            colour: Colour::None,
        }
    }

    /// The concept's own ink, for the tests that check the palette against the
    /// mock rather than against whichever terminal is running the suite.
    #[cfg(test)]
    pub(crate) fn mock() -> Self {
        Self {
            colour: Colour::True,
        }
    }

    /// Resolve the three answers a capability probe has: whether the reader
    /// asked for no colour, what the terminal calls itself, and whether it
    /// claims twenty-four bits.
    fn from_environment(
        no_color: Option<&str>,
        term: Option<&str>,
        colorterm: Option<&str>,
    ) -> Self {
        let refused = no_color.is_some_and(|value| !value.trim().is_empty())
            || term.is_some_and(|term| term.trim().eq_ignore_ascii_case("dumb"));
        let colour = if refused {
            Colour::None
        } else if colorterm.is_some_and(|value| {
            value.trim().eq_ignore_ascii_case("truecolor")
                || value.trim().eq_ignore_ascii_case("24bit")
        }) {
            Colour::True
        } else {
            Colour::Ansi
        };
        Self { colour }
    }

    /// One hue, at the depth this terminal takes: the concept's hex, the nearest
    /// slot, or nothing at all.
    fn hue(&self, true_colour: Color, ansi: Color, base: Style) -> Style {
        match self.colour {
            Colour::True => base.fg(true_colour),
            Colour::Ansi => base.fg(ansi),
            Colour::None => base,
        }
    }

    /// One inline run's style: the row's tone plus what the run's mark named.
    ///
    /// `base` is the tone already resolved for the row, so emphasis inside a
    /// patch row or a failure stays the colour that row claimed.
    fn marked(&self, base: Style, mark: Inline) -> Style {
        match mark {
            Inline::Plain => base,
            Inline::Strong => base.add_modifier(Modifier::BOLD),
            Inline::Emphasis => base.add_modifier(Modifier::ITALIC),
            // Inline code takes the concept's violet, the one hue no meaning in
            // this vocabulary claims; a profile without colour keeps the weight
            // instead, so the distinction survives a monochrome screen.
            Inline::Code => match self.colour {
                Colour::None => base.add_modifier(Modifier::BOLD),
                _ => self.hue(ink::VIOLET, Color::Magenta, base),
            },
        }
    }

    /// The style one tone draws in.
    pub(crate) fn style(&self, tone: Tone) -> Style {
        let plain = Style::default();
        let bold = plain.add_modifier(Modifier::BOLD);
        match tone {
            // Prose is the concept's text ink. On the sixteen-slot path the
            // reader's own foreground is the closest thing their profile has to
            // it, so nothing is pinned.
            Tone::Body => match self.colour {
                Colour::True => plain.fg(ink::TEXT),
                _ => plain,
            },
            // A heading is the concept's text at weight, never a hue of its
            // own: the mock sets its titles in the same ink as its prose.
            Tone::Heading => self.hue(ink::TEXT, Color::Blue, bold),
            Tone::Strong => match self.colour {
                Colour::True => plain.fg(ink::TEXT).add_modifier(Modifier::BOLD),
                _ => bold,
            },
            // The brand mark is the header's first stroke: the mock's own blue,
            // and a reversed block on a terminal that cannot paint it.
            Tone::Chip => match self.colour {
                Colour::None => bold.add_modifier(Modifier::REVERSED),
                _ => self.hue(ink::BRAND, Color::Blue, bold),
            },
            Tone::Accent => self.hue(ink::BLUE, Color::Blue, bold),
            // Metadata recedes by ink on a colour terminal and by weight on a
            // plain one; dimming an already-muted hex would lose it entirely.
            Tone::Muted => match self.colour {
                Colour::True => plain.fg(ink::MUTED),
                Colour::Ansi => plain.fg(Color::Gray),
                Colour::None => plain.add_modifier(Modifier::DIM),
            },
            // The reader's own turn wears the mock's violet: the colour its
            // avatar carries, so a speaker is recognised before the label is
            // read.
            Tone::User => self.hue(ink::VIOLET, Color::Magenta, bold),
            Tone::Ok => self.hue(ink::GREEN, Color::Green, bold),
            Tone::Warn => self.hue(ink::AMBER, Color::Yellow, bold),
            Tone::Error => self.hue(ink::RED, Color::Red, bold),
            Tone::Add => self.hue(ink::GREEN, Color::Green, plain),
            Tone::Remove => self.hue(ink::RED, Color::Red, plain),
            // The row the reader is on is the mock's chosen nav row: text on
            // the raised fill. Without colour it is the reverse block every
            // terminal has, so the meaning never depends on the fill.
            Tone::Select => match self.colour {
                Colour::True => plain
                    .fg(ink::TEXT)
                    .bg(ink::RAISED)
                    .add_modifier(Modifier::BOLD),
                _ => plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
            },
            Tone::TabActive => match self.colour {
                Colour::True => plain
                    .fg(ink::BLUE)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                Colour::Ansi => plain
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                Colour::None => plain.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            },
            Tone::Button => match self.colour {
                Colour::True => plain
                    .fg(Color::White)
                    .bg(ink::BUTTON)
                    .add_modifier(Modifier::BOLD),
                Colour::Ansi => plain
                    .fg(Color::White)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                Colour::None => plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
            },
            Tone::ComposerBorder => match self.colour {
                Colour::True => plain.fg(ink::COMPOSER_BORDER),
                Colour::Ansi => plain.fg(Color::Blue),
                Colour::None => plain.add_modifier(Modifier::BOLD),
            },
            Tone::NavIcon => match self.colour {
                Colour::True => plain.fg(ink::NAV_ICON).add_modifier(Modifier::BOLD),
                Colour::Ansi => plain.fg(Color::Blue).add_modifier(Modifier::BOLD),
                Colour::None => plain.add_modifier(Modifier::BOLD),
            },
            Tone::NavIconActive => match self.colour {
                Colour::True => plain
                    .fg(ink::NAV_ICON_ACTIVE)
                    .bg(ink::RAISED)
                    .add_modifier(Modifier::BOLD),
                Colour::Ansi => plain
                    .fg(Color::Cyan)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                Colour::None => plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
            },
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

/// The frames of the activity spinner: one complete turn, ten cells of braille.
///
/// Braille is the terminal's densest dial - every glyph is exactly one cell, so
/// a spinner never changes a row's width - and the frame rate this client
/// already repaints at makes one turn a little under a second.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner's frame at `tick`, for every row that says "still running".
pub(crate) fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick as usize) % SPINNER.len()]
}

/// The width of a leading spinner frame and its space, when a row opens with
/// one.
///
/// A running call draws its verdict as the animation itself, so the row is the
/// warning tone the static `⚙` marker would have claimed rather than prose.
fn spinner_marker(text: &str) -> Option<usize> {
    SPINNER.iter().find_map(|frame| {
        text.strip_prefix(frame)
            .filter(|rest| rest.starts_with(' '))
            .map(|_| frame.chars().count() + 1)
    })
}

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
        if let Some(mark) = spinner_marker(text) {
            self.carry = Tone::Body;
            return self.spans(text, mark, Tone::Warn, Tone::Body);
        }
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
        Line::from(self.prose(text, self.skin.style(Tone::Body)))
    }

    /// One row assembled from several painted runs: the frame's header chips,
    /// a column's title, the footer's keys and its live facts.
    ///
    /// A run carries its own tone, so the caller decides what each part of the
    /// row *is* where it writes it, and the row itself carries no meaning.
    pub(crate) fn runs(&mut self, runs: &[(String, Tone)]) -> Line<'static> {
        self.carry = Tone::Body;
        let spans: Vec<Span<'static>> = runs
            .iter()
            .filter(|(text, _)| !text.is_empty())
            .map(|(text, tone)| Span::styled(flatten(text).into_owned(), self.skin.style(*tone)))
            .collect();
        Line::from(spans)
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
    ///
    /// The head is the row's own marker; the body is the row's content, so it
    /// is the body that carries its inline marks through [`Painter::prose`].
    fn spans(&self, text: &str, mark: usize, head: Tone, rest: Tone) -> Line<'static> {
        // A row is one row: text that arrived with a line break in it - a
        // session title taken from a message, a tool's own output - is drawn on
        // the row it was given rather than moving the terminal's cursor.
        let flat = flatten(text);
        let text: &str = flat.as_ref();
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
            // A block's own rows are the code as it was written, marks and
            // all: inside a fence a `*` is a star, not emphasis.
            if marks_up(text) {
                spans.extend(self.prose(body, rest_style));
            } else {
                spans.push(Span::styled(body.to_string(), rest_style));
            }
        }
        Line::from(spans)
    }

    /// One run of prose, with the inline marks it carries drawn as what they
    /// name rather than as the characters that wrote them.
    fn prose(&self, text: &str, base: Style) -> Vec<Span<'static>> {
        inline(text)
            .into_iter()
            .filter(|(text, _)| !text.is_empty())
            .map(|(text, mark)| Span::styled(text.to_string(), self.skin.marked(base, mark)))
            .collect()
    }
}

/// One row's text with the characters a row cannot carry.
///
/// A newline or a tab in a title would move the terminal's cursor and paint the
/// rest of the frame into the wrong cells, so each control character becomes a
/// space; every other character is kept exactly as written.
pub(crate) fn flatten(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect(),
    )
}

/// Whether a row may carry inline markdown: every row but a fenced block's own.
fn marks_up(text: &str) -> bool {
    !matches!(text.chars().next(), Some('│' | '╭' | '╰'))
}

/// How one run of a row is marked up.
///
/// These are the marks the desktop's renderer resolves into weight and a code
/// treatment, so the words between them are the content and the marks are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inline {
    /// No marks: the words are the row.
    Plain,
    /// `**strong**`.
    Strong,
    /// `*emphasised*`.
    Emphasis,
    /// `` `code` ``.
    Code,
}

/// Split one run of a row into the parts its inline marks name.
///
/// A terminal has weight and italic in every profile, and one hue to spare for
/// the code treatment, so markdown's three inline marks become three
/// distinctions here instead of three pairs of literal characters.
///
/// Only a matched pair counts, and (outside code) neither side may sit against
/// a space: `2 * 3 * 4` is arithmetic, not emphasis, and an unclosed mark stays
/// the text it was. Underscores are left alone on purpose - an answer about
/// `snake_case_names` must not lose a word to a rule its writer never meant.
fn inline(text: &str) -> Vec<(&str, Inline)> {
    let mut runs: Vec<(&str, Inline)> = Vec::new();
    // Where the run of unmarked text being collected starts.
    let mut plain_at = 0usize;
    let mut at = 0usize;
    while at < text.len() {
        let character = text[at..].chars().next().unwrap_or(' ');
        let mark = match character {
            '`' => Some(("`", Inline::Code)),
            '*' if star_run(text, at) == 2 => Some(("**", Inline::Strong)),
            '*' => Some(("*", Inline::Emphasis)),
            _ => None,
        };
        let Some((delimiter, kind)) = mark else {
            at += character.len_utf8();
            continue;
        };
        let body_at = at + delimiter.len();
        let found = closing(text, body_at, delimiter).filter(|end| {
            // Emphasis may not open or close on a space; code may, because a
            // code span's padding is part of how it was written.
            kind == Inline::Code || !bordered(text, body_at, *end)
        });
        let Some(end) = found else {
            // An unmatched mark is the text it was: skip its delimiter and keep
            // looking, leaving those characters in the plain run.
            at = body_at;
            continue;
        };
        if plain_at < at {
            runs.push((&text[plain_at..at], Inline::Plain));
        }
        runs.push((&text[body_at..end], kind));
        at = end + delimiter.len();
        plain_at = at;
    }
    if plain_at < text.len() {
        runs.push((&text[plain_at..], Inline::Plain));
    }
    if runs.is_empty() {
        runs.push((text, Inline::Plain));
    }
    runs
}

/// How many `*`s run from `at`, capped at the two this client reads.
fn star_run(text: &str, at: usize) -> usize {
    text[at..]
        .chars()
        .take_while(|character| *character == '*')
        .count()
        .min(2)
}

/// Whether the content between `from` and `end` opens or closes on a space.
fn bordered(text: &str, from: usize, end: usize) -> bool {
    let content = &text[from..end];
    content.is_empty()
        || content.starts_with(char::is_whitespace)
        || content.ends_with(char::is_whitespace)
}

/// Where the delimiter that closes the one opened before `from` sits.
fn closing(text: &str, from: usize, delimiter: &str) -> Option<usize> {
    let mut search = from;
    while let Some(found) = text[search..].find(delimiter) {
        let end = search + found;
        if !text[from..end].is_empty() {
            return Some(end);
        }
        search = end + delimiter.len();
    }
    None
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
    /// An approval is pending, and its card waits on a verdict.
    Approval,
    /// The `ask_user` card is on screen.
    Question,
    /// The completion popup is open.
    Menu,
    /// A list of choices is open: Up/Down walks it, and typing narrows it.
    Picker,
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
    "^P go",
    "@ files",
    "^D quit",
    "PgUp/PgDn scroll",
    "^N/^B sessions",
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
        // Idle leads with the two gestures a reader at rest needs: send, and
        // the way out. Ctrl-C is not bound to job cancellation; the quit chord
        // is Ctrl-D. Cancellation is explicit via /cancel [confirm].
        Footer::Idle => &[
            "↑↓ Navigate",
            "Enter Select",
            "Esc Close",
            "Alt+V Toggle sidebar",
        ],
        // The approval is the frame's own warning card; the footer answers
        // "and what do I press?" without repeating the target, and without
        // repeating the two chords the card already prints on its rows. A
        // refusal can carry the reason it was refused - the Host audits it
        // beside the decision - so the note is offered here, where the reader
        // decides.
        Footer::Approval => &[
            "↑↓ chooses",
            "Enter sends the choice",
            "/deny <why> notes the refusal",
        ],
        Footer::Question => &[
            "↑↓ walks options",
            "Enter answers",
            "Esc dismisses",
            "typing answers freely",
        ],
        Footer::Menu => &["Tab accepts", "Esc closes"],
        Footer::Picker => &[
            "↑↓ walks choices",
            "Enter picks",
            "click picks",
            "type to filter",
            "Esc closes",
        ],
        Footer::Busy => &["^C stops the turn", "type to queue a line"],
        Footer::Pane => &["Esc closes", "↑↓ scrolls"],
    }
}

/// The chord a footer group is introduced by, so two groups about the same
/// key can be recognised as one.
fn footer_key(part: &str) -> &str {
    part.split(' ').next().unwrap_or(part)
}

/// The footer for one state, filling `width` cells without cutting a group.
///
/// Groups are dropped from the tail, never split: a row that ends in half a
/// key name would teach the reader nothing and hide the group that was whole.
/// A width too small for even the first group still says the first thing, cut
/// with an ellipsis, because an empty footer is worse than a clipped one.
pub(crate) fn footer_text(footer: Footer, width: usize) -> String {
    let groups = footer_groups(footer);
    if footer == Footer::Idle {
        let mut text = String::new();
        for part in groups {
            let candidate = if text.is_empty() {
                (*part).to_string()
            } else {
                format!("{text}   {part}")
            };
            if candidate.chars().count() > width {
                break;
            }
            text = candidate;
        }
        if text.is_empty() && !groups.is_empty() && width > 0 {
            let part = groups[0];
            text = part.chars().take(width.saturating_sub(1)).collect();
            text.push('…');
        }
        return text;
    }
    let mut parts: Vec<&'static str> = groups.to_vec();
    // A state that already names a chord keeps its own wording: the common
    // keys repeat what the state's rows just said otherwise, and the row is
    // the scarce resource.
    for key in KEYS {
        if !groups
            .iter()
            .any(|group| footer_key(group) == footer_key(key))
        {
            parts.push(key);
        }
    }
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
        assert_eq!(
            Skin::from_environment(Some("1"), Some("xterm-256color"), None).colour,
            Colour::None
        );
        assert_eq!(
            Skin::from_environment(Some(""), Some("xterm-256color"), None).colour,
            Colour::Ansi
        );
        // A dumb terminal would draw the escapes as letters.
        assert_eq!(
            Skin::from_environment(None, Some("dumb"), None).colour,
            Colour::None
        );
        assert_eq!(
            Skin::from_environment(None, Some("DUMB"), Some("truecolor")).colour,
            Colour::None
        );
        assert_eq!(
            Skin::from_environment(None, Some("xterm-256color"), None).colour,
            Colour::Ansi
        );
        assert_eq!(
            Skin::from_environment(None, None, None).colour,
            Colour::Ansi
        );
        // True colour is taken only when the terminal says so, and its own two
        // spellings are the ones the ecosystem uses.
        for claim in ["truecolor", "24bit", " TrueColor "] {
            assert_eq!(
                Skin::from_environment(None, Some("xterm-256color"), Some(claim)).colour,
                Colour::True,
                "{claim:?} must take the concept's ink"
            );
        }
        // A terminal that says nothing is not guessed at: its profile's slots
        // are certain, a hex it cannot paint is not.
        assert_eq!(
            Skin::from_environment(None, Some("screen-256color"), Some("no")).colour,
            Colour::Ansi
        );
    }

    #[test]
    fn the_palette_is_the_concepts_own_ink() {
        // Every hue the mock paints with is painted here as the same hex, so
        // the terminal and the concept cannot drift apart unnoticed.
        let skin = Skin::mock();
        assert_eq!(skin.page_style().bg, Some(ink::PAGE));
        let cases = [
            (Tone::Body, ink::TEXT),
            (Tone::Heading, ink::TEXT),
            (Tone::Strong, ink::TEXT),
            (Tone::Chip, ink::BRAND),
            (Tone::Accent, ink::BLUE),
            (Tone::Muted, ink::MUTED),
            (Tone::User, ink::VIOLET),
            (Tone::Ok, ink::GREEN),
            (Tone::Warn, ink::AMBER),
            (Tone::Error, ink::RED),
            (Tone::Add, ink::GREEN),
            (Tone::Remove, ink::RED),
        ];
        for (tone, expected) in cases {
            assert_eq!(skin.style(tone).fg, Some(expected), "{tone:?}");
        }
        // The chosen row is the mock's raised fill under the text ink, which is
        // what makes a selection read as a surface rather than as a colour.
        let chosen = skin.style(Tone::Select);
        assert_eq!(chosen.fg, Some(ink::TEXT));
        assert_eq!(chosen.bg, Some(ink::RAISED));
        // The hexes themselves are the mock's, not this module's invention.
        assert_eq!(ink::TEXT, Color::Rgb(0xdc, 0xe7, 0xf7));
        assert_eq!(ink::MUTED, Color::Rgb(0x82, 0x94, 0xad));
        assert_eq!(ink::BLUE, Color::Rgb(0x59, 0xa8, 0xff));
        assert_eq!(ink::BUTTON, Color::Rgb(0x16, 0x78, 0xe8));
        assert_eq!(ink::COMPOSER_BORDER, Color::Rgb(0x28, 0x79, 0xc8));
        assert_eq!(skin.style(Tone::Button).bg, Some(ink::BUTTON));
        assert_eq!(
            skin.style(Tone::ComposerBorder).fg,
            Some(ink::COMPOSER_BORDER)
        );
        assert_eq!(ink::GREEN, Color::Rgb(0x35, 0xd6, 0xa1));
        assert_eq!(ink::AMBER, Color::Rgb(0xff, 0xb4, 0x54));
        assert_eq!(ink::VIOLET, Color::Rgb(0x8d, 0x6c, 0xff));
        assert_eq!(ink::RAISED, Color::Rgb(0x16, 0x3d, 0x68));
        assert_eq!(ink::PAGE, Color::Rgb(0x0b, 0x14, 0x22));
        assert_eq!(ink::PANEL, Color::Rgb(0x0d, 0x17, 0x26));
    }

    #[test]
    fn sixteen_slots_keep_the_meaning_a_hex_would_have_carried() {
        // A terminal without true colour still tells a success from a warning
        // from a failure: the concept's four status hues land on the four slots
        // every profile defines, and the reader's own turn keeps the violet
        // family as magenta.
        let skin = Skin::from_environment(None, Some("xterm-256color"), None);
        assert_eq!(skin.style(Tone::Ok).fg, Some(Color::Green));
        assert_eq!(skin.style(Tone::Warn).fg, Some(Color::Yellow));
        assert_eq!(skin.style(Tone::Error).fg, Some(Color::Red));
        assert_eq!(skin.style(Tone::Accent).fg, Some(Color::Blue));
        assert_eq!(skin.style(Tone::User).fg, Some(Color::Magenta));
        assert_eq!(skin.style(Tone::Muted).fg, Some(Color::Gray));
        // The chosen row keeps the reverse block here: a slot cannot paint the
        // mock's fill, and a row the reader is on must never be a guess.
        let chosen = skin.style(Tone::Select);
        assert_eq!(chosen.fg, None);
        assert_eq!(chosen.bg, None);
        assert!(chosen.add_modifier.contains(Modifier::REVERSED));
        // The same meanings are still apart from each other on that path.
        assert_ne!(skin.style(Tone::Ok).fg, skin.style(Tone::Warn).fg);
        assert_ne!(skin.style(Tone::Warn).fg, skin.style(Tone::Error).fg);
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
        // An approval leads with the keys that walk its own card, so they
        // survive even on a row that fits nothing else.
        let narrow = footer_text(Footer::Approval, 40);
        assert_eq!(narrow, "↑↓ chooses · Enter sends the choice");
        // A wider row keeps adding whole groups and stops before the next one.
        let wide = footer_text(Footer::Idle, 80);
        assert!(wide.chars().count() <= 80);
        assert!(wide.starts_with("↑↓ Navigate   Enter Select   Esc Close"));
        assert!(!wide.ends_with(" · "));
        assert!(!wide.ends_with('·'));
        // A row narrower than one group still says the first thing.
        let tiny = footer_text(Footer::Idle, 9);
        assert_eq!(tiny.chars().count(), 9);
        assert!(tiny.ends_with('…'));
    }

    /// A state that already leads with a chord does not get the common list
    /// repeating it: the footer's row is the scarce resource, and a chord
    /// printed twice teaches the reader nothing the first copy did not.
    #[test]
    fn the_footer_names_each_chord_once() {
        for footer in [
            Footer::Idle,
            Footer::Busy,
            Footer::Picker,
            Footer::Menu,
            Footer::Approval,
            Footer::Question,
            Footer::Pane,
        ] {
            let text = footer_text(footer, 200);
            let mut seen: Vec<&str> = Vec::new();
            for part in text.split(" · ") {
                let key = footer_key(part);
                assert!(!seen.contains(&key), "{footer:?} repeats `{key}`: {text:?}");
                seen.push(key);
            }
        }
        // The idle row uses the compact global navigation grammar.
        let idle = footer_text(Footer::Idle, 200);
        assert_eq!(idle.matches("Enter Select").count(), 1, "{idle:?}");
        assert!(idle.starts_with("↑↓ Navigate"), "{idle:?}");
    }

    #[test]
    fn a_running_row_animates_in_the_warning_tone() {
        let skin = Skin::plain();
        // The frame's own answer: a spinner frame is drawn exactly as the
        // static `⚙` marker is, so a call that is running never changes colour
        // as the animation turns.
        for tick in 0..20u64 {
            let marker = format!("{} a running call", spinner(tick));
            let mut painted = Painter::new(skin);
            let line = painted.row(&marker);
            assert_eq!(first(&line), skin.style(Tone::Warn), "tick {tick}");
            assert_eq!(line.spans.len(), 2, "tick {tick} keeps the marker one span");
            // The marker owns its trailing space, like every MARKERS entry, so
            // the row's columns stay where the static marker puts them.
            assert_eq!(line.spans[1].content, "a running call");
        }
        // A braille glyph that is not a spinner frame, or one not followed by
        // the space the writer puts there, is prose and stays prose.
        let mut painted = Painter::new(skin);
        assert_eq!(first(&painted.row("⣿ not a frame")), Style::default());
        let mut painted = Painter::new(skin);
        assert_eq!(first(&painted.row("⠋no space")), Style::default());
    }

    #[test]
    fn a_row_of_runs_paints_each_run_its_own_tone() {
        let skin = Skin::plain();
        let mut painter = Painter::new(skin);
        let line = painter.runs(&[
            (" bsh ".to_string(), Tone::Chip),
            ("tui".to_string(), Tone::Strong),
            (String::new(), Tone::Body),
        ]);
        // An empty run paints nothing: a chip with no text would still be a
        // block of colour the reader has to explain to themselves.
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].style, skin.style(Tone::Chip));
        assert_eq!(line.spans[1].style, skin.style(Tone::Strong));
        assert_eq!(line.spans[1].content, "tui");
    }

    #[test]
    fn a_chip_and_a_strong_fact_survive_a_monochrome_screen() {
        // The header must not depend on the reader's profile having blue: the
        // chip is a reverse block and the session's name is weight, both of
        // which every terminal that can draw text already has.
        let chip = Skin::plain().style(Tone::Chip);
        assert!(chip.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(
            Skin::plain().style(Tone::Strong),
            Style::default().add_modifier(Modifier::BOLD)
        );
    }

    #[test]
    fn a_row_draws_its_inline_marks_as_the_distinctions_they_name() {
        let skin = Skin::plain();
        let mut painter = Painter::new(skin);
        let line = painter.row("bsh › a **strong** word, *one* in `code`");
        let texts: Vec<&str> = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(
            texts,
            vec!["bsh › ", "a ", "strong", " word, ", "one", " in ", "code"],
            "the marks are not content: they are what the content is"
        );
        let body = skin.style(Tone::Body);
        assert_eq!(line.spans[2].style, body.add_modifier(Modifier::BOLD));
        assert_eq!(line.spans[4].style, body.add_modifier(Modifier::ITALIC));
        // A profile without colour keeps the code distinction as weight, so a
        // monochrome screen still says which words are code.
        assert_eq!(line.spans[6].style, body.add_modifier(Modifier::BOLD));
    }

    #[test]
    fn arithmetic_a_stray_mark_and_a_code_row_stay_the_text_they_were() {
        let skin = Skin::plain();
        let mut painter = Painter::new(skin);
        let line = painter.row("bsh › 2 * 3 * 4 and an *unclosed mark");
        let body: String = line
            .spans
            .iter()
            .skip(1)
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(body, "2 * 3 * 4 and an *unclosed mark");
        assert!(
            line.spans
                .iter()
                .skip(1)
                .all(|span| span.style == skin.style(Tone::Body)),
            "nothing there was emphasis, so nothing there is dressed as it"
        );
        // Inside a fence a star is a star: a block's rows are the code as it was
        // written, and reflowing its marks would be editing the answer.
        let code = painter.row("│ let x = *ptr * 2;");
        assert_eq!(code.spans[1].content, "let x = *ptr * 2;");
        assert_eq!(code.spans[1].style, skin.style(Tone::Body));
    }

    #[test]
    fn inline_code_takes_the_one_hue_no_meaning_claims() {
        let skin = Skin::from_environment(None, Some("xterm-256color"), Some("truecolor"));
        let mut painter = Painter::new(skin);
        let line = painter.row("bsh › run `bsh ext list` first");
        assert_eq!(line.spans[2].content, "bsh ext list");
        assert_eq!(line.spans[2].style.fg, Some(ink::VIOLET));
        assert_eq!(line.spans[1].content, "run ");
        assert_eq!(line.spans[3].content, " first");
    }

    #[test]
    fn every_footer_state_leads_with_the_keys_it_needs() {
        for footer in [
            Footer::Idle,
            Footer::Approval,
            Footer::Question,
            Footer::Menu,
            Footer::Picker,
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
