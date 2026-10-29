//! The reader's `/name` skill gesture, mirrored from the reference pre-step
//! listener.
//!
//! `dsh-tool-skill`'s `agent/pre-step` hook scans the step's user messages for
//! a whitespace-bounded `/name` token, resolves each name in the skill
//! registry, and appends one message per match holding
//! `renderSkillContent(skill)` with source
//! `{ kind: "skill-invocation", name, form: "instructions" }`. The rendered
//! block is measured from `dsh-skill`'s own `renderSkillContent`
//! (`docs/ui-mirror/reference/pkgs/dsh-skill__lib__index.js`):
//!
//! ```text
//! <skill_content name="NAME">
//! <skill_resources>
//! Base directory for this skill: PATH
//! Resolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.
//! </skill_resources>
//!
//! <skill_instructions>
//! BODY
//! </skill_instructions>
//! </skill_content>
//! ```
//!
//! The gesture is the same `/(^|\s)\/([a-z0-9]+(?:-[a-z0-9]+)*)(?=\s|$)/`
//! grammar: a second slash or any other non-boundary character breaks the
//! match, which keeps file paths (`/usr/bin`) and fractions (`5/8`) out.
//!
//! BSH's slash palette offers the gesture without pre-loading it: choosing a
//! skill row writes `/name ` into the draft and the reader keeps writing -
//! exactly the reference's own `SlashAction::Insert`. The loading therefore
//! belongs to the SEND: [`expand`] runs in the runner's turn task, against
//! the agent that turn was built with, and appends the block to the request's
//! own text. A skill the reader switched off was never registered, so nothing
//! is appended for it, and the `load_skill` tool answers from the same set.
//!
//! The leading invocation is replaced, not just annotated: once the block is
//! attached, the `/name` token leaves the request, so the model reads the
//! skill's instructions and the reader's own words rather than a literal
//! command it might narrate instead of executing. The transcript keeps what
//! the reader wrote. BSH carries the block in the SAME user turn rather than
//! as a second message: the engine's request input is one message, and the
//! content order the model reads is the reference's.
//!
//! A mid-sentence gesture is the exception: it stays in the text (it reads as
//! part of a sentence) and its skill still loads, because the reference's
//! scan is over the whole message.
//!
//! A message STEERED into a running turn is expanded too: the shell keeps the
//! turn's own agent beside its mailbox, so the gesture resolves against the
//! build that will take the message at its next step rather than against a
//! later turn's build.
//!
//! Only skills the reader may invoke are reachable here. A manifest that says
//! `disable-model-invocation: true` is still registered, which is exactly why
//! this reader finds it; the model's own catalogue and its `load_skill` tool
//! use `agent::skills::lookup_for_model`, so the two paths cannot disagree.

use agent::skills::SkillContent;
use agent::Agent;
use react::llm::{Content, ContentPart};

use crate::model::SkillInvocation;

/// The second line of the measured resource hint, shared by every arm of
/// `renderResourceHint` in `dsh-skill`.
pub(crate) const RESOURCE_HINT: &str = "Resolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.";

/// The skill names a text invokes, first-seen order and deduplicated.
///
/// The scan is over the reader's own words, not the whole composed request:
/// the caller passes the text the turn carries. Every match is a CANDIDATE -
/// it is not yet checked against the registry, exactly as the reference's
/// `invokedSkillNames` says - so an unknown name costs a failed lookup and
/// nothing else.
pub(crate) fn invoked_skill_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut names: Vec<String> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let boundary = index == 0 || bytes[index - 1].is_ascii_whitespace();
        if boundary && bytes[index] == b'/' {
            if let Some(end) = skill_name_end(bytes, index + 1) {
                let name = &text[index + 1..end];
                if !names.iter().any(|seen| seen == name) {
                    names.push(name.to_string());
                }
                index = end;
                continue;
            }
        }
        index += 1;
    }
    names
}

/// The skill name a leading /name token names, when the token fits the gesture
/// grammar.
///
/// Only the FIRST token counts here. The palette inserts the invocation at the
/// start of the draft, and a mid-sentence /word is far more likely to be prose
/// (/etc, /tmp) than a skill, so the send path's unresolved-name notice stays
/// quiet about those.
pub(crate) fn leading_skill_name(text: &str) -> Option<String> {
    let (start, end) = leading_gesture(text)?;
    Some(text[start..end].to_string())
}

/// The byte index one past the skill name starting at `start`, when both the
/// name grammar and the trailing boundary hold.
///
/// A name is one or more `[a-z0-9]+` groups joined by single hyphens, and it
/// ends only at whitespace or the end of the text. The trailing check is what
/// makes `/usr/bin` and `5/8` ordinary prose.
fn skill_name_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = name_group_end(bytes, start)?;
    while index < bytes.len() && bytes[index] == b'-' {
        index = name_group_end(bytes, index + 1)?;
    }
    if index < bytes.len() && !bytes[index].is_ascii_whitespace() {
        return None;
    }
    Some(index)
}

/// The end of one `[a-z0-9]+` group, or `None` when the group is empty.
fn name_group_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    while index < bytes.len()
        && (bytes[index].is_ascii_lowercase() || bytes[index].is_ascii_digit())
    {
        index += 1;
    }
    (index > start).then_some(index)
}

/// One skill rendered exactly the way the reference's `renderSkillContent`
/// renders it.
///
/// The name is attribute-escaped and the base directory is prose-escaped, so
/// a provider-supplied string cannot close the framing tags; the instructions
/// themselves are the manifest body, inserted verbatim (the reference does the
/// same), because a skill is allowed to carry markup of its own.
pub(crate) fn render_skill_content(skill: &SkillContent) -> String {
    let base = skill.skill_dir.to_string_lossy();
    let mut out = String::new();
    out.push_str("<skill_content name=\"");
    out.push_str(&escape_attr(&skill.metadata.name));
    out.push_str("\">\n<skill_resources>\n");
    out.push_str("Base directory for this skill: ");
    out.push_str(&escape_text(&base));
    out.push('\n');
    out.push_str(RESOURCE_HINT);
    out.push_str("\n</skill_resources>\n\n<skill_instructions>\n");
    out.push_str(&skill.instructions);
    out.push_str("\n</skill_instructions>\n</skill_content>");
    out
}

/// The request rewritten as ONE combined message: the invoked skills'
/// instructions, a divider, then the reader's question.
///
/// The block is not a second message and not a second request: the reference's
/// pre-step injection is folded into the prompt the turn already sends, so the
/// model reads `instructions + --- + question` as one ask. Three deliberate
/// shape choices:
///
/// - The instructions come FIRST. The skill says how to answer; the question
///   is what to answer, and a model that reads the procedure before the ask
///   starts in the right frame.
/// - The LEADING slash-name token leaves the request once its block is
///   attached. The reader wrote an invocation, not prose; the model already
///   has the skill, and leaving the token in makes it narrate "the user
///   invoked the skill" instead of doing the work.
/// - The durable transcript keeps what the reader wrote, so the bubble and
///   the next turn's replay still name the skill.
///
/// A mid-sentence gesture stays in the text (it reads as part of a sentence)
/// and its block still rides at the head of the same message.
///
/// It runs on the UI thread, against the agent the turn was built with, so the
/// bodies are already parsed: a send costs a lookup, never a disk read, and a
/// skill the settings switched off was never registered.
pub(crate) fn expand(agent: &Agent, content: Content) -> (Content, Vec<SkillInvocation>) {
    let text = text_of(&content);
    if text.is_empty() {
        return (content, Vec::new());
    }
    let leading = leading_gesture(&text);
    let leading_name = leading.map(|(start, end)| text[start..end].to_string());
    let mut blocks: Vec<String> = Vec::new();
    let mut invoked: Vec<SkillInvocation> = Vec::new();
    let mut drop_token = false;
    for name in invoked_skill_names(&text) {
        if let Some(skill) = agent.skill(&name) {
            if leading_name.as_deref() == Some(name.as_str()) {
                drop_token = true;
            }
            let block = render_skill_content(skill);
            blocks.push(block.clone());
            invoked.push(SkillInvocation {
                name: name.clone(),
                content: block,
            });
        }
    }
    if blocks.is_empty() {
        return (content, Vec::new());
    }
    let body = match (drop_token, leading) {
        (true, Some((_, end))) => text[end..].trim_start().to_string(),
        _ => text.clone(),
    };
    let merged = merge(&body, &blocks.join("\n\n"));
    let content = match content {
        Content::Text(_) => Content::Text(merged),
        Content::Parts(mut parts) => {
            // The request's own text block carries it, so the attachments keep
            // their measured position behind the words they belong to. A parts
            // turn with no text at all gets the block in front.
            match parts.iter_mut().find_map(|part| match part {
                ContentPart::Text { text } => Some(text),
                _ => None,
            }) {
                Some(text) => *text = merged,
                None => parts.insert(0, ContentPart::Text { text: merged }),
            }
            Content::Parts(parts)
        }
    };
    (content, invoked)
}

/// The reader's words followed by the skill blocks, without a leading blank
/// line when the reader wrote nothing but the invocation.
fn merge(body: &str, blocks: &str) -> String {
    if body.trim().is_empty() {
        blocks.to_string()
    } else {
        // The instructions come FIRST, then the divider, then the reader's
        // question: one message whose head is the skill and whose ask is the
        // request, which is the combined shape the reference's single user
        // message reads as. The divider keeps the two from running together.
        format!("{blocks}\n\n{SKILL_DIVIDER}\n\n{}", body.trim_start())
    }
}

/// The divider between a skill's instructions and the reader's question.
pub(crate) const SKILL_DIVIDER: &str = "---";

/// The byte span of a LEADING slash-name token in the text, if there is one.
fn leading_gesture(text: &str) -> Option<(usize, usize)> {
    let base = text.len() - text.trim_start().len();
    if !text[base..].starts_with('/') {
        return None;
    }
    let end = skill_name_end(text.as_bytes(), base + 1)?;
    Some((base + 1, end))
}

/// The part of a user row its author wrote: the request without a skill block
/// the harness appended for a slash-name gesture.
///
/// The child transcript reads the child's own session back, where that block
/// is part of the prompt row; the read-only view shows what the caller asked
/// for, not the skill body the harness fetched for it.
pub(crate) fn visible_text(text: &str) -> &str {
    // The combined form puts the block(s) first, so skip past the LAST
    // closing tag and the divider before showing anything.
    let rest = match text.strip_prefix(SKILL_BLOCK_MARKER) {
        Some(_) => match text.rfind(SKILL_CLOSE) {
            Some(index) => &text[index + SKILL_CLOSE.len()..],
            None => "",
        },
        None => text,
    };
    let rest = rest.trim_start_matches('\n');
    let rest = rest
        .strip_prefix(SKILL_DIVIDER)
        .unwrap_or(rest)
        .trim_start_matches('\n');
    match rest.find(SKILL_BLOCK_MARKER) {
        Some(index) => rest[..index].trim_end_matches('\n'),
        None => rest,
    }
}

/// The closing tag every rendered skill block ends with.
pub(crate) const SKILL_CLOSE: &str = "</skill_content>";

/// The opening tag every rendered skill block starts with.
pub(crate) const SKILL_BLOCK_MARKER: &str = "<skill_content name=";

/// The text parts of one request, in order.
///
/// An attachment turn is `Parts`; only its text blocks can carry a gesture,
/// and the binary blocks are irrelevant to this scan.
pub(crate) fn text_of(content: &Content) -> String {
    match content {
        Content::Text(text) => text.clone(),
        Content::Parts(parts) => {
            let mut text = String::new();
            for part in parts {
                if let ContentPart::Text { text: block } = part {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(block);
                }
            }
            text
        }
    }
}

/// Escape a value for an XML attribute, the reference's `escapeAttr`.
fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// Escape model-facing prose inside skill markup, the reference's
/// `escapeText`.
fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::{invoked_skill_names, render_skill_content};
    use agent::skills::{SkillContent, SkillMetadata};
    use std::path::PathBuf;

    /// One skill with the given body, rooted at `dir`.
    fn skill(name: &str, dir: &str, body: &str) -> SkillContent {
        SkillContent {
            metadata: SkillMetadata::new(
                name.to_string(),
                "a test skill".to_string(),
                PathBuf::from(dir).join("SKILL.md"),
            ),
            instructions: body.to_string(),
            skill_dir: PathBuf::from(dir),
        }
    }

    /// The grammar is the reference's: a whitespace-bounded name, anywhere in
    /// the sentence, and nothing that reads as a path or a fraction.
    #[test]
    fn a_whitespace_bounded_slash_name_is_the_gesture() {
        assert_eq!(invoked_skill_names("/alpha do it"), vec!["alpha"]);
        assert_eq!(
            invoked_skill_names("please /alpha then /beta-2 and /alpha again"),
            vec!["alpha", "beta-2"],
            "all matches, first-seen order, deduplicated"
        );
        assert_eq!(invoked_skill_names("end with /alpha"), vec!["alpha"]);
        for text in [
            "/usr/bin/env",
            "5/8 of the work",
            "x/alpha",
            "/Alpha",
            "/alpha, next",
            "/alpha-",
            "/alpha/beta",
            "not a gesture",
        ] {
            assert!(
                invoked_skill_names(text).is_empty(),
                "{text:?} must not read as an invocation"
            );
        }
    }

    /// The rendered block is the reference's own shape, byte for byte.
    #[test]
    fn the_block_is_the_reference_shape() {
        let rendered = render_skill_content(&skill("alpha", "/skills/alpha", "Body of alpha."));
        let expected = concat!(
            "<skill_content name=\"alpha\">\n",
            "<skill_resources>\n",
            "Base directory for this skill: /skills/alpha\n",
            "Resolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.\n",
            "</skill_resources>\n\n",
            "<skill_instructions>\n",
            "Body of alpha.\n",
            "</skill_instructions>\n",
            "</skill_content>"
        );
        assert_eq!(rendered, expected);
    }

    /// Provider-supplied text cannot forge the framing: the name is
    /// attribute-escaped and the base directory is prose-escaped.
    #[test]
    fn the_framing_cannot_be_forged_by_the_text_it_wraps() {
        let rendered = render_skill_content(&skill("a\"b<c", "/skills/a&b", "x"));
        assert!(
            rendered.starts_with("<skill_content name=\"a&quot;b&lt;c\">"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Base directory for this skill: /skills/a&amp;b\n"),
            "{rendered}"
        );
    }

    /// A user row that carries a skill block reads back as the caller's own
    /// words, which is what the child transcript draws - in either shape, the
    /// combined one (block first) or the older one (block after).
    #[test]
    fn the_visible_text_drops_the_skill_block() {
        let combined = concat!(
            "<skill_content name=\"grill-me\">\n",
            "<skill_instructions>\npush back\n</skill_instructions>\n",
            "</skill_content>\n\n---\n\n/grill-me aa"
        );
        let appended = concat!(
            "/grill-me aa\n\n",
            "<skill_content name=\"grill-me\">\n",
            "<skill_instructions>\npush back\n</skill_instructions>\n",
            "</skill_content>"
        );
        assert_eq!(super::visible_text(combined), "/grill-me aa");
        assert_eq!(super::visible_text(appended), "/grill-me aa");
        assert_eq!(
            super::visible_text("plain words"),
            "plain words",
            "a request with no block is unchanged"
        );
    }

    /// The send path loads a leading invocation, replaces the token with the
    /// reader's words, and hands back the block the transcript draws.
    #[test]
    fn an_invoked_skill_is_loaded_and_recorded() {
        let root = std::env::temp_dir().join(format!("bsh-skill-expand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("alpha");
        std::fs::create_dir_all(&dir).expect("the skill directory");
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: alpha\ndescription: a test skill\n---\n\nFollow the alpha procedure.\n",
        )
        .expect("the manifest");
        let mut agent = agent::Agent::new(
            agent::AgentConfig::default(),
            std::sync::Arc::new(agent::agent::agentic::LlmProvider::new()),
        );
        agent
            .register_skills_from_dir(root.clone())
            .expect("the skill registers");

        let (content, invoked) = super::expand(
            &agent,
            super::Content::Text("/alpha do the thing".to_string()),
        );
        let text = super::text_of(&content);
        assert!(
            text.starts_with("<skill_content name=\"alpha\">"),
            "the instructions open the combined message: {text}"
        );
        assert!(!text.starts_with("/alpha"), "no literal command: {text}");
        assert_eq!(
            super::visible_text(&text),
            "do the thing",
            "the divider separates the instructions from the question: {text}"
        );
        assert!(
            text.contains(&format!("{}\n\ndo the thing", super::SKILL_DIVIDER)),
            "one message: instructions, divider, question: {text}"
        );
        assert!(
            text.contains("<skill_content name=\"alpha\">"),
            "the skill is loaded: {text}"
        );
        assert!(
            text.contains(&format!("Base directory for this skill: {}", dir.display())),
            "with the directory its relative paths resolve against: {text}"
        );
        assert_eq!(
            invoked.len(),
            1,
            "the block is recorded for the pane: {invoked:?}"
        );
        assert_eq!(invoked[0].name, "alpha");
        assert!(invoked[0].content.contains("Follow the alpha procedure."));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A leading name no registered skill answers is left alone: it is the
    /// reader's text, and nothing is invented for it.
    #[test]
    fn an_unknown_leading_name_is_left_alone() {
        let agent = agent::Agent::new(
            agent::AgentConfig::default(),
            std::sync::Arc::new(agent::agent::agentic::LlmProvider::new()),
        );
        let (content, invoked) = super::expand(
            &agent,
            super::Content::Text("/nope do the thing".to_string()),
        );
        assert_eq!(super::text_of(&content), "/nope do the thing");
        assert!(invoked.is_empty(), "nothing is recorded: {invoked:?}");
    }

    /// The send path's notice reads only a LEADING gesture: a mid-sentence
    /// /word is prose, not an invocation.
    #[test]
    fn only_a_leading_gesture_is_the_sends_business() {
        assert_eq!(
            super::leading_skill_name("/grill-me aa"),
            Some("grill-me".to_string())
        );
        assert_eq!(
            super::leading_skill_name("  /alpha"),
            Some("alpha".to_string()),
            "leading whitespace does not hide the token"
        );
        for text in [
            "look at /etc/hosts",
            "please /alpha now",
            "/usr/bin/env",
            "/Alpha",
            "no gesture",
        ] {
            assert!(
                super::leading_skill_name(text).is_none(),
                "{text:?} is not a leading invocation"
            );
        }
    }
}
