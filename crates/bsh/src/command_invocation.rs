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
//! - The template is model-facing text: it heads the request, the reader's own
//!   words follow the divider, and the leading annotation leaves the request
//!   once its template is attached - the model already has the instruction, and
//!   a literal `/name` only invites it to narrate the invocation instead of
//!   doing the work.
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

use crate::extension::CommandSurface;

/// The divider between a command's template and the reader's own words.
///
/// The same `---` the skill gesture uses ([crate::skill_invocation]): one
/// shape for "the harness put this in front of your question".
pub(crate) const COMMAND_DIVIDER: &str = "---";

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
    let base = text.len() - text.trim_start().len();
    let rest = text[base..].strip_prefix('/')?;
    let end = rest.bytes().take_while(|byte| is_name_byte(*byte)).count();
    if end == 0 {
        return None;
    }
    // The grammar ends the name at whitespace or end of input: `/greet.json`
    // and `/greet,next` name no command, exactly as `skill_name_end` refuses
    // a path or a fraction.
    let after = &rest[end..];
    if after
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return None;
    }
    let command = commands
        .iter()
        .find(|command| command.name == rest[..end])?;
    Some(merge(&command.template, after.trim_start()))
}

/// Whether a byte can appear in a command name.
///
/// Qualified names are sanitized (ASCII alphanumeric, `-` and `_`, lowercased),
/// so this is the whole alphabet: anything else ends the token.
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// The template first, then the reader's words after the divider - without a
/// divider when one half is empty, so a bare annotation is just its template.
fn merge(template: &str, body: &str) -> String {
    let template = template.trim();
    let body = body.trim();
    if body.is_empty() {
        return template.to_string();
    }
    if template.is_empty() {
        return body.to_string();
    }
    format!("{template}\n\n{COMMAND_DIVIDER}\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::{compose, draft};
    use crate::extension::CommandSurface;

    /// The template the fixtures contribute.
    const TEMPLATE: &str = "Say hello from com.example.hello and name the open workspace.";

    fn commands() -> Vec<CommandSurface> {
        vec![CommandSurface {
            name: "com_example_hello__greet".to_string(),
            description: "Greet the reader".to_string(),
            template: TEMPLATE.to_string(),
        }]
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
        assert_eq!(compose(&commands(), &text), Some(TEMPLATE.to_string()));
    }

    /// The reader's words follow the template behind the divider, and the
    /// annotation itself leaves the request.
    #[test]
    fn the_annotation_and_the_readers_words_compose_one_message() {
        let text = format!("{}in one sentence", draft("com_example_hello__greet"));
        let composed = compose(&commands(), &text).expect("the token composes");
        assert_eq!(
            composed,
            format!("{TEMPLATE}\n\n---\n\nin one sentence"),
            "the template heads the request and the words follow it"
        );
        assert!(!composed.contains('/'), "the annotation left the request");
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

    /// The leading whitespace a draft may carry does not hide the token.
    #[test]
    fn leading_whitespace_is_not_part_of_the_draft() {
        let text = "  /com_example_hello__greet now";
        assert_eq!(
            compose(&commands(), text),
            Some(format!("{TEMPLATE}\n\n---\n\nnow"))
        );
    }
}
