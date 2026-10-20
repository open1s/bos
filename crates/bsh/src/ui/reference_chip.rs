//! The composer's reference chips: an `@path` in the draft drawn the way the
//! reference draws it, instead of as plain text.
//!
//! The reference projects user text through its own `user-text` module
//! (`dsh-client-ui-primitives/lib/user-text.module.css`, extracted at
//! `docs/ui-mirror/reference/dsh-css-files/`): every `@token` becomes a
//! `ReferenceChip` carrying `appearance: "file" | "folder"`, a `label` (the
//! path's last segment) and a `title` (the raw token), and `.refChip` paints it
//! in the business-primary colour at weight 500 behind a leading 1em glyph.
//! Clicking it opens the file (`dsh-client-ui-reference/lib/client.js`,
//! `FileReference`).
//!
//! bsh draws the same three facts on the kit's inline-token seam
//! (`Textarea::token` / `on_token_click`), so a mention stays ONE run of text
//! for editing, undo and selection while painting as a chip. Typing around it,
//! backspacing through it and copying it keep working because the editor still
//! owns the text; only the paint is ours.
//!
//! The chip is DECORATION. A token the grammar cannot attach is left as plain
//! text rather than dropped, so failing to draw a chip never costs the reader a
//! character they typed.

use gpui_kit::component::input::{
    InlineToken, InlineTokenClickEvent, InlineTokenContext, InlineTokenError, InputContent,
};
use gpui_kit::*;

use super::{icon, IconName};
use crate::reference::{self, FileRef};
use crate::theme;

/// The measured `.refChip{margin:0 2px}` and `.refIcon{margin-right:4px}`.
const CHIP_MARGIN: f32 = 2.0;
const ICON_MARGIN_R: f32 = 4.0;

/// The chip's font size: `.refChip` inherits the consumer's size, and the
/// reference's own glyph rides at `1em` with `vertical-align:-0.125em`.
///
/// GPUI hands a token the line height rather than the font size, so the size is
/// derived from it. The 0.72 ratio is the reference's body line box (a 14px
/// setting over the measured 22px line), and the clamp keeps a large or small
/// setting from making the glyph absurd.
fn chip_size(line_height: f32) -> f32 {
    (line_height * 0.72).clamp(11.0, 16.0)
}

/// One chip's data, which is the reference's own `node.data`: the `label` it
/// prints and the `raw` token it titles the chip with.
///
/// The reference derives the label as
/// `path.split(/[\\/]/).filter(Boolean).at(-1)`, so `src/ui/composer.rs`
/// labels as `composer.rs`; a directory keeps its own name.
pub(crate) fn chip_data(reference: &FileRef) -> (String, String) {
    let label = reference
        .path
        .split(['/', '\\'])
        .rfind(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| reference.path.clone());
    // The token as the editor holds it, which is what a token must cover.
    let raw =
        reference::format_mention(&reference.path, reference.directory, reference.range, false)
            .unwrap_or_else(|| format!("@{}", reference.path));
    (label, raw)
}

/// The token one reference becomes.
///
/// Its `id` is the path, so a click opens the file without re-parsing the
/// draft; its `text` is the raw mention, because the kit requires a token to
/// cover exactly the characters it rides.
pub(crate) fn file_token(reference: &FileRef) -> InlineToken {
    let (label, raw) = chip_data(reference);
    InlineToken::new(reference.path.clone(), raw).with_label(label)
}

/// The draft as content the editor can show chips for.
///
/// Every `@reference` the grammar finds is attached to its own byte range. A
/// token the kit refuses (an overlapping hash name, a range that is not where
/// the scanner thought) is skipped and its text stays plain: the content is
/// always returned, so a chip that cannot be drawn never blocks a write.
pub(crate) fn content(text: &str) -> InputContent {
    let mut content = InputContent::new(text);
    for reference in reference::parse_references(text) {
        let (_, raw) = chip_data(&reference);
        let Some(start) = find_token(text, &raw) else {
            continue;
        };
        match content
            .clone()
            .with_token(start..start + raw.len(), file_token(&reference))
        {
            Ok(next) => content = next,
            Err(InlineTokenError::OverlappingTokens) => continue,
            Err(_) => continue,
        }
    }
    content
}

/// Where the token sits in the draft.
///
/// The scanner works in `&str` terms and the formatter re-quotes, so the two
/// can disagree for a hand-typed path. The search is bounded to the token
/// itself: a miss leaves that reference plain rather than attaching a chip to
/// somebody else's bytes.
fn find_token(text: &str, raw: &str) -> Option<usize> {
    let quote = raw.starts_with("@\"");
    // A quoted token is unique by its leading `@"`; a bare one must sit on a
    // token boundary, which is the same rule the scanner used to find it.
    let mut from = 0usize;
    while let Some(index) = text[from..].find(raw) {
        let start = from + index;
        let boundary = start == 0
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if boundary || quote {
            return Some(start);
        }
        from = start + 1;
    }
    None
}

/// The chip the editor paints for a token.
///
/// The reference's own comment on `.refChip` is the shape to keep: the chip is
/// plain inline rather than a flex container, because a container takes its
/// baseline from its first item - the glyph, which has no text baseline - and
/// the label then rides above the line. GPUI places an inline token as one box
/// inside the text run, so the glyph and label are a single horizontal run and
/// the glyph is dropped by the measured `-0.125em` to sit on the optical
/// baseline.
pub(crate) fn chip(context: &InlineTokenContext) -> impl IntoElement {
    let size = chip_size(f32::from(context.line_height()));
    let directory = context.token().id().ends_with('/');
    let glyph = if directory {
        IconName::Folder
    } else {
        IconName::File
    };
    let accent = theme::accent();
    div()
        .flex()
        .flex_row()
        .items_center()
        .mx(px(CHIP_MARGIN))
        .text_size(px(size))
        // `color:var(--dsw-alias-state-business-primary)` at `font-weight:500`.
        .text_color(rgb(accent))
        .font_weight(FontWeight::MEDIUM)
        .min_w(px(0.0))
        .child(
            div()
                .flex_none()
                .mr(px(ICON_MARGIN_R))
                .child(icon(glyph, size, accent)),
        )
        .child(
            div()
                .min_w(px(0.0))
                // `overflow-wrap:anywhere`, so a long path wraps instead of
                // pushing the composer past its column.
                .text_ellipsis()
                .child(context.token().label().clone()),
        )
}

/// Which file a chip click opens: the token's own id, which is the path the
/// chip was built from.
pub(crate) fn clicked_path(event: &InlineTokenClickEvent) -> String {
    event.token().id().to_string()
}
#[cfg(test)]
mod tests {
    use super::{chip_data, content, find_token};
    use crate::reference::parse_references;

    /// The label is the reference's own last path segment, and the raw token is
    /// what the editor holds - so a chip can only ever cover the text it names.
    #[test]
    fn a_reference_labels_with_its_last_segment() {
        let refs = parse_references("@src/ui/composer.rs");
        assert_eq!(refs.len(), 1);
        let (label, raw) = chip_data(&refs[0]);
        assert_eq!(label, "composer.rs");
        assert_eq!(raw, "@src/ui/composer.rs");
    }

    /// A directory keeps its own name and its trailing slash, so the chip reads
    /// as a folder rather than as a file called `ui`.
    #[test]
    fn a_directory_labels_with_its_own_name() {
        let refs = parse_references("@src/ui/");
        assert_eq!(refs.len(), 1);
        let (label, raw) = chip_data(&refs[0]);
        assert_eq!(label, "ui");
        assert_eq!(raw, "@src/ui/");
    }

    /// Content attaches one token per finished reference, at the reference's
    /// own bytes.
    #[test]
    fn content_attaches_a_token_per_reference() {
        let text = "look at @src/ui/composer.rs and @README.md please";
        let built = content(text);
        assert_eq!(built.text().as_ref(), text);
        let spans: Vec<(usize, usize)> = built
            .tokens()
            .iter()
            .map(|span| (span.range().start, span.range().end))
            .collect();
        assert_eq!(spans.len(), 2, "both references chip");
        for (start, end) in &spans {
            assert!(
                text[*start..*end].starts_with('@'),
                "a token covers its own @token"
            );
        }
    }

    /// Prose with no reference produces no tokens and keeps its text exactly.
    #[test]
    fn prose_produces_no_tokens() {
        let text = "no references here, just words";
        let built = content(text);
        assert!(built.tokens().is_empty());
        assert_eq!(built.text().as_ref(), text);
    }

    /// An `@` inside a word is not a reference, which is the scanner's own rule
    /// and what keeps an email address from becoming a chip.
    #[test]
    fn an_at_inside_a_word_is_not_a_reference() {
        let built = content("mail me at someone@example.com");
        assert!(built.tokens().is_empty());
    }

    /// The finder only matches on a token boundary, so a path that also appears
    /// inside a longer word does not get a chip over the wrong bytes.
    #[test]
    fn the_finder_needs_a_token_boundary() {
        assert_eq!(find_token("x@src", "@src"), None);
        assert_eq!(find_token("see @src now", "@src"), Some(4));
        assert_eq!(find_token("@src", "@src"), Some(0));
    }
}
