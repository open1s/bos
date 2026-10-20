//! The @path reference grammar: the token a reader types, the text a pick
//! inserts, and the guidance the model receives for it.
//!
//! The grammar is the reference's own (dsh-file-reference/lib/types/grammar.js):
//! an @ at the start of input or after whitespace opens a reference; whitespace
//! is only allowed inside the quoted form `@"path with spaces"`; a directory
//! keeps a trailing slash. This build adds one suffix the reference's grammar
//! does not carry yet: `#L12-L20` (or `#L12`) names a line range inside the
//! file, so a reader can link an excerpt rather than a whole file.
//!
//! The guidance constant is the reference's FILE_REFERENCE_PROMPT, verbatim
//! apart from one sentence describing the range suffix. It is what tells the
//! model that an @token is a path the reader pointed at and that the file must
//! be read before the model may claim to have inspected it.

/// Model guidance for path-only references selected by the reader.
///
/// The first sentence through the quoted-path rule is the reference's own
/// FILE_REFERENCE_PROMPT; the range and session sentences are this build's
/// additions.
pub(crate) const FILE_REFERENCE_PROMPT: &str = "Tokens prefixed with @ are paths the user explicitly referenced. Relative paths resolve from the workspace root; absolute paths identify files or directories on the host. A trailing slash marks a directory: list it when its contents matter. Anything else is a file: use the read tool when its contents are needed, and do not claim to have inspected it before reading. @\"...\" quotes a path containing spaces. A #L<start>-L<end> suffix (or #L<line> for one line) names a line range inside that file: read the file and treat only those lines as the referenced excerpt. A token of the form @[title](dsh-session:id) names another conversation, not a file.";

/// One parsed reference: the path, whether it names a directory, and the line
/// range when the token carries one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct FileRef {
    /// The path, unquoted.
    pub(crate) path: String,
    /// Whether the token ends with the directory separator.
    pub(crate) directory: bool,
    /// The inclusive line range, if the token carries one.
    pub(crate) range: Option<(u32, u32)>,
}

/// Split a `#L12-L20` / `#L12` suffix off a query.
///
/// Returns the path part and the range. A malformed suffix (no digits, a zero
/// line) is left in the path so a file whose name really contains a hash still
/// matches.
pub(crate) fn split_range(query: &str) -> (&str, Option<(u32, u32)>) {
    let Some(index) = query.rfind('#') else {
        return (query, None);
    };
    let suffix = &query[index + 1..];
    let Some(rest) = suffix
        .strip_prefix('L')
        .or_else(|| suffix.strip_prefix('l'))
    else {
        return (query, None);
    };
    let parse =
        |text: &str| -> Option<u32> { text.trim().parse::<u32>().ok().filter(|line| *line > 0) };
    let range = match rest.split_once('-') {
        Some((start, end)) => {
            let start = start
                .strip_prefix('L')
                .or_else(|| start.strip_prefix('l'))
                .unwrap_or(start);
            let end = end
                .strip_prefix('L')
                .or_else(|| end.strip_prefix('l'))
                .unwrap_or(end);
            match (parse(start), parse(end)) {
                (Some(start), Some(end)) => Some((start.min(end), start.max(end))),
                _ => None,
            }
        }
        None => parse(rest).map(|line| (line, line)),
    };
    match range {
        Some(range) => (&query[..index], Some(range)),
        None => (query, None),
    }
}

/// Format a picked candidate as the canonical reference text.
///
/// This is the reference formatter plus the range suffix: whitespace quotes the
/// path, a directory keeps its trailing slash, and a range follows the closed
/// quote. `None` means the path cannot be represented safely (a control
/// character or an embedded quote).
pub(crate) fn format_mention(
    path: &str,
    directory: bool,
    range: Option<(u32, u32)>,
    preserve_quote: bool,
) -> Option<String> {
    let path = if directory {
        format!("{path}/")
    } else {
        path.to_string()
    };
    if path
        .chars()
        .any(|character| character.is_control() || character == '"')
    {
        return None;
    }
    let quoted = preserve_quote || path.contains(char::is_whitespace);
    let mut text = if !quoted {
        format!("@{path}")
    } else if directory {
        // A quoted directory leaves the quote open so completion can descend.
        format!("@\"{path}")
    } else {
        format!("@\"{path}\"")
    };
    if let Some((start, end)) = range {
        if start == end {
            text.push_str(&format!("#L{start}"));
        } else {
            text.push_str(&format!("#L{start}-L{end}"));
        }
    }
    Some(text)
}

/// Every reference in a line of text, in source order.
///
/// The scanner mirrors the reference's `activeAtToken` rule: an @ opens a
/// reference at the start of the text or after whitespace, and an @ inside
/// another token (an email address) is not one.
pub(crate) fn parse_references(text: &str) -> Vec<FileRef> {
    let mut found: Vec<FileRef> = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'@' {
            index += 1;
            continue;
        }
        let starts = index == 0
            || text[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if !starts {
            index += 1;
            continue;
        }
        let rest = &text[index + 1..];
        // A labelled mention is a session reference, not a path.
        if rest.starts_with('[') {
            let end = rest.find(')').map(|close| close + 1).unwrap_or(rest.len());
            index += 1 + end;
            continue;
        }
        let (raw, consumed) = if let Some(inner) = rest.strip_prefix('"') {
            match inner.find('"') {
                Some(end) => (&inner[..end], end + 2),
                None => (inner, rest.len()),
            }
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (&rest[..end], end)
        };
        let (path, range) = split_range(raw);
        let directory = path.ends_with('/');
        let path = path.trim_end_matches('/').to_string();
        if !path.is_empty() {
            found.push(FileRef {
                path,
                directory,
                range,
            });
        }
        index += 1 + consumed;
    }
    found
}

/// Whether a line carries at least one reference.
pub(crate) fn has_references(text: &str) -> bool {
    !parse_references(text).is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        format_mention, has_references, parse_references, split_range, FILE_REFERENCE_PROMPT,
    };

    #[test]
    fn a_pick_formats_the_reference_spelling() {
        assert_eq!(
            format_mention("src/main.rs", false, None, false).as_deref(),
            Some("@src/main.rs")
        );
        assert_eq!(
            format_mention("my notes/a.md", false, None, false).as_deref(),
            Some("@\"my notes/a.md\"")
        );
        assert_eq!(
            format_mention("src", true, None, false).as_deref(),
            Some("@src/")
        );
        assert_eq!(
            format_mention("src", true, None, true).as_deref(),
            Some("@\"src/")
        );
        assert_eq!(
            format_mention("src/lib.rs", false, Some((12, 20)), false).as_deref(),
            Some("@src/lib.rs#L12-L20")
        );
        assert_eq!(
            format_mention("src/lib.rs", false, Some((7, 7)), false).as_deref(),
            Some("@src/lib.rs#L7")
        );
        assert_eq!(
            format_mention("bad\"name", false, None, false),
            None,
            "an embedded quote cannot be represented"
        );
    }

    #[test]
    fn a_range_suffix_splits_off_the_query() {
        assert_eq!(
            split_range("src/main.rs#L10-L20"),
            ("src/main.rs", Some((10, 20)))
        );
        assert_eq!(split_range("a.rs#L4"), ("a.rs", Some((4, 4))));
        assert_eq!(split_range("a.rs#l9-L3"), ("a.rs", Some((3, 9))));
        assert_eq!(
            split_range("a.rs#L0"),
            ("a.rs#L0", None),
            "a zero line is not a range"
        );
        assert_eq!(split_range("a.rs"), ("a.rs", None));
        assert_eq!(split_range("dir#file"), ("dir#file", None));
    }

    #[test]
    fn references_parse_with_quotes_directories_and_ranges() {
        let text = "look at @src/main.rs and @\"docs/a b.md\" then @src/#L3-L8";
        let refs = parse_references(text);
        assert_eq!(refs.len(), 3, "{refs:?}");
        assert_eq!(refs[0].path, "src/main.rs");
        assert_eq!(refs[0].range, None);
        assert_eq!(refs[1].path, "docs/a b.md");
        assert!(refs[2].directory);
        assert_eq!(refs[2].range, Some((3, 8)));
        assert!(
            !has_references("mail me at a@b.com"),
            "an @ inside a token is not a reference"
        );
        assert!(has_references("@a"), "a leading @ is a reference");
        assert!(!has_references("plain text"));
        assert!(
            parse_references("@[Another chat](dsh-session:abc)").is_empty(),
            "a labelled session mention is not a file reference"
        );
        assert_eq!(
            parse_references("@[Another chat](dsh-session:abc) and @a.rs")[0].path,
            "a.rs",
            "and scanning continues after it"
        );
    }

    #[test]
    fn the_guidance_tells_the_model_to_read_first() {
        assert!(FILE_REFERENCE_PROMPT.contains("do not claim to have inspected it before reading"));
        assert!(FILE_REFERENCE_PROMPT.contains("#L<start>-L<end>"));
        assert!(FILE_REFERENCE_PROMPT.contains("quotes a path containing spaces"));
    }
}
