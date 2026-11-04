//! The reader's `/command` gesture for an extension-contributed command.
//!
//! An extension command is a prompt TEMPLATE, not draft text. Choosing its row
//! writes the annotation `/name ` into the composer - the reference's command
//! "claim": the token stays visible, the caret parks after it, and the reader
//! keeps typing the command's input - and the SEND composes the template with
//! those words into the one message the model reads.
//!
//! That split is deliberate:
//!
//! - The composer holds what the READER wrote, so the line they see is the line
//!   the transcript replays. Pasting a template there made a one-row choice
//!   look like a paragraph the reader had typed, and left no room for the words
//!   the command was meant to take.
//! - The template is model-facing text: it heads the request in an
//!   `<extension_command name="...">` block, the reader's own words follow the
//!   divider, and the leading annotation leaves the request once its block is
//!   attached - the model already has the instruction, and a literal `/name`
//!   only invites it to narrate the invocation instead of doing the work. The
//!   block's name is also the one fact [`visible_text`] needs to show a child's
//!   row back as the annotation the reader typed.
//!
//! The gesture is the LEADING token only, and its name ends at whitespace or
//! end of input, exactly the reference's command grammar. A command name is the
//! qualified, sanitized contribution id (`namespace__contribution`), which is
//! what the palette draws and what the annotation carries, so the draft and the
//! resolver cannot disagree about the spelling.
//!
//! Resolution needs the commands the build currently offers: a package switched
//! off after its annotation was written composes nothing, and the token travels
//! as ordinary prose rather than inventing a body.

use react::llm::{Content, ContentPart};

use crate::extension::CommandSurface;

/// The divider between a command's template and the reader's own words.
///
/// The same `---` the skill gesture uses ([crate::skill_invocation]): one
/// shape for "the harness put this in front of your question".
pub(crate) const COMMAND_DIVIDER: &str = "---";

/// The opening of the model-facing block a command's template rides in.
///
/// The template is not the reader's prose, and the block says so - the same
/// framing the skill gesture uses. It also carries the command's name, which is
/// what lets a child's row be shown back as the `/name` annotation the reader
/// typed ([`visible_text`]).
pub(crate) const BLOCK_OPEN: &str = "<extension_command name=\"";

/// The closing tag of the model-facing command block.
pub(crate) const BLOCK_CLOSE: &str = "</extension_command>";

/// The draft text one chosen extension command leaves in the composer.
///
/// The trailing space ends the token and leaves the caret after it, so the
/// reader's next keystroke is the command's input rather than more of its name.
pub(crate) fn draft(name: &str) -> String {
    format!("/{name} ")
}

/// The model request one submitted draft carries, when its leading token names
/// a command this build still offers.
///
/// `None` means the draft is not an invocation - ordinary prose, an unknown
/// name, or a name whose package is no longer loaded - and the caller sends the
/// reader's text unchanged.
pub(crate) fn compose(commands: &[CommandSurface], text: &str) -> Option<String> {
    let (name, body) = leading_name(text)?;
    let command = commands.iter().find(|command| command.name == name)?;
    let composed = merge(&command.name, &command.template, body);
    // A command that contributes no body to a draft with no words would send an
    // empty request; the caller keeps the reader's line instead.
    (!composed.is_empty()).then_some(composed)
}

/// The leading token when it has the extension-command SHAPE but no loaded
/// command answers for it.
///
/// The `__` separator belongs to the qualified contribution id, and no skill
/// name can contain it (`skill_invocation`'s grammar is hyphen-only), so a
/// leading `/ns__local` that does not resolve is a command this build does not
/// have - a typo, or a package that is not loaded. The caller tells the reader
/// instead of letting the line travel as prose that looks like a command.
///
/// A token WITHOUT `__` is not reported: it is ordinary prose (`/usr/bin`) or a
/// skill gesture, which has its own notice.
pub(crate) fn unavailable<'a>(commands: &[CommandSurface], text: &'a str) -> Option<&'a str> {
    let (name, _) = leading_name(text)?;
    if !name.contains("__") {
        return None;
    }
    (!commands.iter().any(|command| command.name == name)).then_some(name)
}

/// The leading command token as its name and the body after it.
///
/// The name ends at whitespace or end of input: `/greet.json` and
/// `/greet,next` name no command, exactly as `skill_name_end` refuses a path or
/// a fraction.
fn leading_name(text: &str) -> Option<(&str, &str)> {
    let base = text.len() - text.trim_start().len();
    let rest = text[base..].strip_prefix('/')?;
    let end = rest.bytes().take_while(|byte| is_name_byte(*byte)).count();
    if end == 0 {
        return None;
    }
    let after = &rest[end..];
    if after
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return None;
    }
    Some((&rest[..end], after.trim_start()))
}

/// Whether a byte can appear in a command name.
///
/// Qualified names are sanitized (ASCII alphanumeric, `-` and `_`, lowercased),
/// so this is the whole alphabet: anything else ends the token.
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// The template rendered in its block, then the reader's words after the
/// divider - without a divider when one half is empty, so a bare annotation is
/// just its template.
fn merge(name: &str, template: &str, body: &str) -> String {
    let template = template.trim();
    let body = body.trim();
    if template.is_empty() {
        return body.to_string();
    }
    let block = format!("{BLOCK_OPEN}{name}\">\n{template}\n{BLOCK_CLOSE}");
    if body.is_empty() {
        block
    } else {
        format!("{block}\n\n{COMMAND_DIVIDER}\n\n{body}")
    }
}

/// The annotation a composed request came from, for a view that shows what the
/// reader wrote.
///
/// A child run's transcript is flattened from the child's own agent, where the
/// user row is the model-facing request - the block included. This turns that
/// row back into `/name words`, which is what the reader typed and what the
/// parent's own row already says. `None` means the text is not a composed
/// command request, so the caller keeps it as it is.
pub(crate) fn visible_text(text: &str) -> Option<String> {
    let base = text.len() - text.trim_start().len();
    let rest = text[base..].strip_prefix(BLOCK_OPEN)?;
    let name_end = rest.find("\">")?;
    let name = &rest[..name_end];
    let after = rest[name_end + 2..].strip_prefix('\n')?;
    let close = after.find(BLOCK_CLOSE)?;
    let tail = after[close + BLOCK_CLOSE.len()..].trim_start_matches('\n');
    let words = tail.strip_prefix(COMMAND_DIVIDER).unwrap_or(tail).trim();
    let mut shown = format!("/{name}");
    if !words.is_empty() {
        shown.push(' ');
        shown.push_str(words);
    }
    Some(shown)
}

/// The model-facing content of one user row being REPLAYED into a later
/// request.
///
/// The session stores the reader's annotation, because the transcript is what
/// they wrote; a later turn must still carry the block the live send attached,
/// or the model reads a `/name` with no instruction behind it. A row that is
/// not an invocation comes back unchanged, and a parts row keeps its images
/// while only its text part is rewritten. The operation is idempotent: a row
/// that is already a composed block is left as it is.
pub(crate) fn replay(commands: &[CommandSurface], content: Content) -> Content {
    let text = match &content {
        Content::Text(text) => text.as_str(),
        Content::Parts(parts) => parts
            .iter()
            .find_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Binary { .. } => None,
            })
            .unwrap_or(""),
    };
    let Some(composed) = compose(commands, text) else {
        return content;
    };
    match content {
        Content::Text(_) => Content::Text(composed),
        Content::Parts(mut parts) => {
            match parts.iter_mut().find_map(|part| match part {
                ContentPart::Text { text } => Some(text),
                ContentPart::Binary { .. } => None,
            }) {
                Some(text) => *text = composed,
                // A parts row with no text at all gets the block in front, the
                // shape the skill gesture uses for the same case.
                None => parts.insert(0, ContentPart::Text { text: composed }),
            }
            Content::Parts(parts)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compose, draft, replay, unavailable, visible_text};
    use crate::extension::CommandSurface;
    use react::llm::{Content, ContentPart};

    /// The template the fixtures contribute.
    const TEMPLATE: &str = "Say hello from com.example.hello and name the open workspace.";

    fn commands() -> Vec<CommandSurface> {
        vec![CommandSurface {
            name: "com_example_hello__greet".to_string(),
            description: "Greet the reader".to_string(),
            template: TEMPLATE.to_string(),
        }]
    }

    /// The model-facing block the template rides in.
    fn block(name: &str, template: &str) -> String {
        format!("<extension_command name=\"{name}\">\n{template}\n</extension_command>")
    }

    /// Choosing the row writes the annotation, never the template: the reader
    /// gets a short token and a caret ready for their own words.
    #[test]
    fn a_chosen_command_writes_its_annotation_not_its_template() {
        assert_eq!(
            draft("com_example_hello__greet"),
            "/com_example_hello__greet "
        );
    }

    /// The annotation alone is the command: the request carries its template.
    #[test]
    fn a_bare_annotation_sends_the_template_alone() {
        let text = draft("com_example_hello__greet");
        assert_eq!(
            compose(&commands(), &text),
            Some(block("com_example_hello__greet", TEMPLATE))
        );
    }

    /// The reader's words follow the template behind the divider, and the
    /// annotation itself leaves the request.
    #[test]
    fn the_annotation_and_the_readers_words_compose_one_message() {
        let text = format!("{}in one sentence", draft("com_example_hello__greet"));
        let composed = compose(&commands(), &text).expect("the token composes");
        assert_eq!(
            composed,
            format!(
                "{}\n\n---\n\nin one sentence",
                block("com_example_hello__greet", TEMPLATE)
            ),
            "the template heads the request and the words follow it"
        );
        assert!(
            !composed.contains("/com_example_hello__greet"),
            "the annotation left the request"
        );
    }

    /// A name no loaded command contributes is prose: the draft is sent as
    /// written rather than being rewritten by a guess.
    #[test]
    fn an_unknown_name_is_left_as_prose() {
        for text in [
            "/plan do it",
            "/nosuch__command do it",
            "/usr/bin/env",
            "5/8 of it",
            "not a gesture",
            "/com_example_hello__greetx more",
            "/com_example_hello__greet, more",
        ] {
            assert_eq!(compose(&commands(), text), None, "{text:?}");
        }
    }

    /// Only the LEADING token is a command. A mid-sentence token reads as part
    /// of a sentence, which is the rule the skill gesture already follows.
    #[test]
    fn only_the_leading_token_is_the_gesture() {
        let text = "please /com_example_hello__greet now";
        assert_eq!(compose(&commands(), text), None);
    }

    /// A command with no body and a draft with no words composes nothing, so
    /// the caller sends the reader's line rather than an empty request.
    #[test]
    fn an_empty_body_with_no_words_is_no_composition() {
        let empty = vec![CommandSurface {
            name: "empty__noop".to_string(),
            description: String::new(),
            template: String::new(),
        }];
        assert_eq!(compose(&empty, "/empty__noop "), None);
        assert_eq!(
            compose(&empty, "/empty__noop now").as_deref(),
            Some("now"),
            "the reader's words still compose"
        );
    }

    /// A qualified name that resolves to no loaded command is reported, while
    /// prose and a loaded command are not.
    #[test]
    fn a_missing_qualified_command_is_reported_by_name() {
        assert_eq!(
            unavailable(&commands(), "/nosuch__command do it"),
            Some("nosuch__command")
        );
        assert_eq!(
            unavailable(&commands(), "/com_example_hello__greet do it"),
            None
        );
        assert_eq!(unavailable(&commands(), "/plan do it"), None);
        assert_eq!(unavailable(&commands(), "/usr/bin/env"), None);
        assert_eq!(unavailable(&commands(), "please /nosuch__command"), None);
    }

    /// The leading whitespace a draft may carry does not hide the token.
    #[test]
    fn leading_whitespace_is_not_part_of_the_draft() {
        let text = "  /com_example_hello__greet now";
        assert_eq!(
            compose(&commands(), text),
            Some(format!(
                "{}\n\n---\n\nnow",
                block("com_example_hello__greet", TEMPLATE)
            ))
        );
    }

    /// The model-facing request reads back as the annotation the reader typed,
    /// which is the row a child view shows.
    #[test]
    fn the_composed_request_reads_back_as_the_annotation() {
        let text = format!("{}in one sentence", draft("com_example_hello__greet"));
        let composed = compose(&commands(), &text).expect("the token composes");
        assert_eq!(
            visible_text(&composed).as_deref(),
            Some("/com_example_hello__greet in one sentence")
        );
        let bare = compose(&commands(), &draft("com_example_hello__greet"))
            .expect("a bare annotation composes");
        assert_eq!(
            visible_text(&bare).as_deref(),
            Some("/com_example_hello__greet"),
            "a command with no words shows only its annotation"
        );
    }

    /// A child row may also carry the skill block the send appended in front of
    /// the command, so the projection is read AFTER the skill projection - the
    /// order `caps::child_transcript` uses.
    #[test]
    fn the_projection_reads_through_a_skill_block() {
        let composed = compose(
            &commands(),
            &format!("{}in one sentence", draft("com_example_hello__greet")),
        )
        .expect("the token composes");
        let skill =
            format!("<skill_content name=\"alpha\">\n</skill_content>\n\n---\n\n{composed}");
        let skill_visible = crate::skill_invocation::visible_text(&skill);
        assert_eq!(
            visible_text(skill_visible).as_deref(),
            Some("/com_example_hello__greet in one sentence")
        );
    }

    /// Ordinary prose is not a composed request, so a caller keeps it as it is.
    #[test]
    fn prose_has_no_projection() {
        assert_eq!(visible_text("plain words"), None);
        assert_eq!(visible_text("/alpha do it"), None);
    }

    /// A stored annotation replays as the block its live send carried, and the
    /// operation is idempotent so a turn that re-seeds twice cannot nest
    /// blocks.
    #[test]
    fn a_stored_annotation_replays_as_its_block() {
        let stored = Content::Text("/com_example_hello__greet in one sentence".to_string());
        let composed = compose(&commands(), "/com_example_hello__greet in one sentence")
            .expect("the token composes");
        let replayed = replay(&commands(), stored);
        assert_eq!(replayed, Content::Text(composed.clone()));
        assert_eq!(
            replay(&commands(), replayed.clone()),
            replayed,
            "a composed row is not composed again"
        );
    }

    /// A parts row keeps its image and only its text part moves.
    #[test]
    fn a_replayed_parts_row_keeps_its_images() {
        let Content::Parts(mut image) =
            Content::binary("image/png".to_string(), "AAAA".to_string())
        else {
            panic!("binary content is parts");
        };
        let mut parts = vec![ContentPart::Text {
            text: "/com_example_hello__greet in one sentence".to_string(),
        }];
        parts.append(&mut image);
        let replayed = replay(&commands(), Content::Parts(parts));
        let Content::Parts(parts) = replayed else {
            panic!("a parts row stays parts");
        };
        assert!(
            parts
                .iter()
                .any(|part| matches!(part, ContentPart::Binary { .. })),
            "the image is still there"
        );
        let text = parts
            .iter()
            .find_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Binary { .. } => None,
            })
            .expect("the text part");
        assert_eq!(
            text,
            compose(&commands(), "/com_example_hello__greet in one sentence")
                .expect("the token composes")
        );
    }
}
