//! The typed slash-command grammar every interface shares.
//!
//! A command line is parsed the way the reference's command registry parses
//! it: the name is the first whitespace-delimited token and everything after it
//! is the argument. The grammar lives here, not in the desktop's state, because
//! the terminal accepts the same lines: a `/goal pause` a window understands
//! must mean the same thing in a terminal, including the rule that a control
//! word only acts when it fills the whole argument.

/// One typed slash command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashLine {
    /// `/plan ...`
    Plan(PlanRequest),
    /// `/goal ...`
    Goal(GoalRequest),
    /// `/compact` with its raw argument (the command takes none).
    Compact(String),
}

/// What a `/plan` line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanRequest {
    /// Bare `/plan`: enter plan mode.
    On,
    /// `/plan off`: leave it without model input.
    Off,
    /// `/plan <task>`: enter plan mode and submit the task as the turn.
    Task(String),
}

/// What a `/goal` line asked for.
///
/// The control words are recognized only when they fill the whole argument, so
/// `/goal pause after verification` creates that literal objective.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalRequest {
    /// Bare `/goal`: report the goal.
    Show,
    /// `/goal <objective>`: create and arm.
    Create(String),
    /// `/goal edit <objective>`: rewrite the objective in place.
    Edit(String),
    /// `/goal pause`
    Pause,
    /// `/goal resume`
    Resume,
    /// `/goal clear`
    Clear,
    /// `/goal edit` with no replacement.
    InvalidEdit,
}

/// Parse a typed slash command line.
///
/// The commands the reference's command plane owns here take a line: `/plan`
/// (hint [off|message]), `/goal`
/// ([objective|clear|edit objective|pause|resume]) and `/compact`.
/// `/compact` takes NO arguments - the shipped command returns its usage line
/// when rawInput.trim() is non-empty - so the raw argument travels to the
/// handler, which owns that refusal rather than the parser. Anything else, or a
/// leading word that merely starts with one of them, is not a command line and
/// is sent as a normal message.
pub(crate) fn parse_slash_line(text: &str) -> Option<SlashLine> {
    let text = text.trim();
    let (name, rest) = match text.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (text, ""),
    };
    match name {
        "/plan" => Some(SlashLine::Plan(match rest {
            "" => PlanRequest::On,
            "off" => PlanRequest::Off,
            task => PlanRequest::Task(task.to_string()),
        })),
        "/goal" => Some(SlashLine::Goal(parse_goal_request(rest))),
        "/compact" => Some(SlashLine::Compact(rest.to_string())),
        _ => None,
    }
}

/// The `/goal` grammar: exact control words, `edit <objective>`, else an
/// objective.
pub(crate) fn parse_goal_request(rest: &str) -> GoalRequest {
    if rest.is_empty() {
        return GoalRequest::Show;
    }
    match rest.to_lowercase().as_str() {
        "clear" => return GoalRequest::Clear,
        "pause" => return GoalRequest::Pause,
        "resume" => return GoalRequest::Resume,
        "edit" => return GoalRequest::InvalidEdit,
        _ => {}
    }
    if let Some(end) = rest.find(char::is_whitespace)
        && rest[..end].eq_ignore_ascii_case("edit")
    {
        let objective = rest[end..].trim();
        return if objective.is_empty() {
            GoalRequest::InvalidEdit
        } else {
            GoalRequest::Edit(objective.to_string())
        };
    }
    GoalRequest::Create(rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::{GoalRequest, PlanRequest, SlashLine, parse_slash_line};

    /// The /plan and /goal line grammar is the reference's command registry
    /// grammar: exact control words only when they fill the argument, every
    /// other suffix an objective, and /plan off the one non-objective word.
    #[test]
    fn the_slash_command_line_grammar_is_the_references() {
        assert_eq!(
            parse_slash_line("/plan"),
            Some(SlashLine::Plan(PlanRequest::On))
        );
        assert_eq!(
            parse_slash_line("  /plan   "),
            Some(SlashLine::Plan(PlanRequest::On))
        );
        assert_eq!(
            parse_slash_line("/plan off"),
            Some(SlashLine::Plan(PlanRequest::Off))
        );
        assert_eq!(
            parse_slash_line("/plan build the thing"),
            Some(SlashLine::Plan(PlanRequest::Task(
                "build the thing".to_string()
            )))
        );
        assert_eq!(parse_slash_line("/planner hi"), None);
        assert_eq!(parse_slash_line("just a message"), None);
        assert_eq!(parse_slash_line("mail a@b.com"), None);

        assert_eq!(
            parse_slash_line("/goal"),
            Some(SlashLine::Goal(GoalRequest::Show))
        );
        assert_eq!(
            parse_slash_line("/goal pause"),
            Some(SlashLine::Goal(GoalRequest::Pause))
        );
        assert_eq!(
            parse_slash_line("/goal PAUSE"),
            Some(SlashLine::Goal(GoalRequest::Pause)),
            "control words are case-insensitive"
        );
        assert_eq!(
            parse_slash_line("/goal pause after verification"),
            Some(SlashLine::Goal(GoalRequest::Create(
                "pause after verification".to_string()
            ))),
            "a control word only acts when it fills the whole argument"
        );
        assert_eq!(
            parse_slash_line("/goal edit a better objective"),
            Some(SlashLine::Goal(GoalRequest::Edit(
                "a better objective".to_string()
            )))
        );
        assert_eq!(
            parse_slash_line("/goal edit"),
            Some(SlashLine::Goal(GoalRequest::InvalidEdit))
        );
        assert_eq!(
            parse_slash_line("/goal clear"),
            Some(SlashLine::Goal(GoalRequest::Clear))
        );
        assert_eq!(
            parse_slash_line("/goal resume"),
            Some(SlashLine::Goal(GoalRequest::Resume))
        );
        assert_eq!(
            parse_slash_line("/goal CLEAR"),
            Some(SlashLine::Goal(GoalRequest::Clear))
        );
        assert_eq!(
            parse_slash_line("/goal ReSuMe"),
            Some(SlashLine::Goal(GoalRequest::Resume))
        );
        assert_eq!(
            parse_slash_line("/goal EdIt a better objective"),
            Some(SlashLine::Goal(GoalRequest::Edit(
                "a better objective".to_string()
            )))
        );
        assert_eq!(
            parse_slash_line("/goal edit\ta better objective"),
            Some(SlashLine::Goal(GoalRequest::Edit(
                "a better objective".to_string()
            ))),
            "edit is recognized before any Unicode whitespace"
        );
        assert_eq!(
            parse_slash_line("/goal edit \u{00a0}"),
            Some(SlashLine::Goal(GoalRequest::InvalidEdit)),
            "an edit whose replacement trims to nothing is invalid"
        );
        assert_eq!(
            parse_slash_line("/goal editx"),
            Some(SlashLine::Goal(GoalRequest::Create("editx".to_string()))),
            "edit must be a whole word"
        );
        assert_eq!(
            parse_slash_line("/goal    "),
            Some(SlashLine::Goal(GoalRequest::Show)),
            "bare /goal with trailing whitespace shows"
        );
        assert_eq!(
            parse_slash_line("/goal edit   "),
            Some(SlashLine::Goal(GoalRequest::InvalidEdit))
        );
        assert_eq!(
            parse_slash_line("/goal clear now"),
            Some(SlashLine::Goal(GoalRequest::Create(
                "clear now".to_string()
            ))),
            "a control word plus an argument is an objective"
        );
        assert_eq!(parse_slash_line("/goal/clear"), None);
        assert_eq!(parse_slash_line("/goals clear"), None);
    }

    /// The compact command takes no arguments; any suffix is the usage
    /// refusal, and it is a command even when the slash card is closed.
    #[test]
    fn the_compact_grammar_is_argument_free() {
        assert_eq!(
            parse_slash_line("/compact"),
            Some(SlashLine::Compact(String::new()))
        );
        assert_eq!(
            parse_slash_line("  /compact   "),
            Some(SlashLine::Compact(String::new())),
            "trailing whitespace trims to the bare command"
        );
        assert_eq!(
            parse_slash_line("/compact last 5"),
            Some(SlashLine::Compact("last 5".to_string())),
            "the argument reaches the handler, which owns the refusal"
        );
        assert_eq!(parse_slash_line("/compactnow"), None);
    }
}
