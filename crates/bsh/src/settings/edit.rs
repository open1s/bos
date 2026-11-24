//! The settings a client may write through the Host.
//!
//! A window's Settings dialog edits these fields directly; a terminal has no
//! form, so the Host offers the same fields as named keys and applies, in one
//! place, exactly the rules the dialog applies when it reads a field back:
//! a capacity may carry `K` or `M`, a temperature outside 0 to 2 is refused
//! rather than clamped, an emptied sampling field means "the model's default"
//! rather than zero. Keeping the rules here - beside the documents they write -
//! is what stops the two surfaces from drifting: there is one parser for both.
//!
//! Secrets are deliberately absent. A vault entry is written through the vault,
//! not through a settings key, because a value that travels as a settings row
//! would also travel back out as one, and a settings report never carries a key.

use super::Settings;

/// The keys this Host accepts, in the order a client lists them.
///
/// The report's editable rows carry these ids, so a client never needs a second
/// vocabulary: the row it draws is the key it sends. [`apply`] refuses a key
/// outside this list and names the ones it has, so an old client asking for a
/// removed field is told what replaced it.
pub(crate) const EDITABLE_KEYS: [&str; 13] = [
    "temperature",
    "max-tokens",
    "context-budget",
    "verify-command",
    "llm-timeout",
    "llm-retries",
    "shell-timeout",
    "shell",
    "files",
    "hosted-web-search",
    "parallel-tool-calls",
    "web-search-base-url",
    "web-search-results",
];

/// Write one key from its text form, the way the Settings dialog writes a field.
///
/// The value arrives as text because that is what a form holds: the reader types
/// `0.4`, `8K` or a command. Parsing happens here, once, so a client that sends
/// the text the pane showed writes the value the pane promised. An empty value
/// is not a special case invented for the terminal: it is the same "clear this
/// field" the dialog offers, and each key decides whether clearing it means
/// something.
pub(crate) fn apply(settings: &mut Settings, key: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    let agent = &mut settings.workspace.agent;
    match key {
        // A sampling temperature of NaN parses as a float but cannot be
        // serialized onto the wire, and a number outside 0 to 2 is not a
        // sampling value the engine honours, so neither is stored and neither
        // is silently clamped: the write says what the field needs.
        "temperature" => {
            if value.is_empty() {
                agent.temperature = None;
                return Ok(());
            }
            match value.parse::<f32>() {
                Ok(parsed) if parsed.is_finite() && (0.0..=2.0).contains(&parsed) => {
                    agent.temperature = Some(parsed);
                    Ok(())
                }
                _ => Err(format!(
                    "temperature must be a number from 0 to 2 (or empty for the model default), not '{value}'"
                )),
            }
        }
        // Zero is not an output cap. Storing "none" is the same fact the
        // request will carry; storing 0 would reach an endpoint as
        // `max_tokens: 0`, which every provider rejects. A count too large for
        // the wire is as unreadable as a word: clamping it would send a value
        // the reader never typed.
        "max-tokens" => {
            if value.is_empty() {
                agent.max_tokens = None;
                return Ok(());
            }
            match super::parse_capacity(value) {
                Some(0) => {
                    agent.max_tokens = None;
                    Ok(())
                }
                Some(parsed) if parsed <= u32::MAX as usize => {
                    agent.max_tokens = Some(parsed as u32);
                    Ok(())
                }
                _ => Err(format!(
                    "max-tokens must be a count such as 8192, 8K or 1M, not '{value}'"
                )),
            }
        }
        // The budget is a capacity, and zero is a real setting here: it turns
        // the budget off instead of standing for "no opinion".
        "context-budget" => match super::parse_capacity(value) {
            Some(parsed) => {
                agent.context_budget = parsed;
                Ok(())
            }
            None => Err(format!(
                "context-budget must be a count such as 131072 or 128K (0 turns the budget off), not '{value}'"
            )),
        },
        // An emptied verify field is "no command": the desktop stores `None`
        // for exactly this, and a stored empty string would be run as a blank
        // shell line.
        "verify-command" => {
            agent.verify_command = (!value.is_empty()).then(|| value.to_string());
            Ok(())
        }
        // A timeout of zero would expire every request before it was sent, so
        // the floor is one second - the same floor the dialog writes.
        "llm-timeout" => match value.parse::<u64>() {
            Ok(parsed) => {
                agent.llm_timeout_secs = parsed.max(1);
                Ok(())
            }
            Err(_) => Err(format!(
                "llm-timeout must be a whole number of seconds, not '{value}'"
            )),
        },
        // A ten-retry budget is already generous; more is a hang dressed up as
        // resilience, which is why the dialog caps it instead of refusing.
        "llm-retries" => match value.parse::<u32>() {
            Ok(parsed) => {
                agent.llm_max_retries = parsed.min(10);
                Ok(())
            }
            Err(_) => Err(format!(
                "llm-retries must be a whole number from 0 to 10, not '{value}'"
            )),
        },
        "shell-timeout" => match value.parse::<u64>() {
            Ok(parsed) => {
                agent.shell_timeout_secs = parsed.max(1);
                Ok(())
            }
            Err(_) => Err(format!(
                "shell-timeout must be a whole number of seconds, not '{value}'"
            )),
        },
        // The four tool switches the desktop's Tools pane writes. A switch has
        // no empty form: "off" is a decision, not an absence, so a blank value is
        // refused instead of guessed at.
        "shell" => {
            agent.bash_enabled = switch(value)?;
            Ok(())
        }
        "files" => {
            agent.file_tools_enabled = switch(value)?;
            Ok(())
        }
        "hosted-web-search" => {
            agent.web_tools_enabled = switch(value)?;
            Ok(())
        }
        "parallel-tool-calls" => {
            agent.parallel_tool_calls = switch(value)?;
            Ok(())
        }
        // A cleared base URL is the shipped default, not an empty endpoint: an
        // empty one makes every search fail with a setup error, and a reader
        // who empties the field means "put back what it was".
        "web-search-base-url" => {
            settings.global.web_search.base_url = if value.is_empty() {
                crate::web::DEFAULT_BASE_URL.to_string()
            } else {
                value.to_string()
            };
            Ok(())
        }
        // The tool asks for a count in 1..=MAX_RESULTS and clamps it there
        // anyway; storing the clamped value keeps the row honest about what the
        // next search will ask for. Unlike a dialog field, a key that was typed
        // on purpose cannot silently keep the old value: an unreadable count is
        // refused so the reader learns the row did not change.
        "web-search-results" => match value.parse::<usize>() {
            Ok(parsed) => {
                settings.global.web_search.results_per_search =
                    parsed.clamp(1, crate::web::MAX_RESULTS);
                Ok(())
            }
            Err(_) => Err(format!(
                "web-search-results must be a whole number from 1 to {}, not '{value}'",
                crate::web::MAX_RESULTS
            )),
        },
        other => Err(format!(
            "no setting is named '{other}'; this Host accepts {}",
            EDITABLE_KEYS.join(", ")
        )),
    }
}

/// Read a switch the way the Tools pane's toggle writes one.
///
/// Both spellings the dialog can produce are accepted - the toggle stores a
/// bool, and a reader typing a value may use any of these words - but a blank or
/// invented word is refused: "shell" is either registered or not.
fn switch(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "enabled" => Ok(true),
        "off" | "false" | "0" | "no" | "disabled" => Ok(false),
        _ => Err(format!(
            "a switch takes on/off (or true/false), not '{value}'"
        )),
    }
}

/// Whether a key accepts an empty value at all, in the words a client shows.
///
/// A terminal types `/set <key>` to clear a field, so a client has to know
/// which fields clearing means something for before it sends a write that
/// would only come back refused.
pub(crate) fn accepts_empty(key: &str) -> bool {
    matches!(
        key,
        "temperature" | "max-tokens" | "verify-command" | "web-search-base-url"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Settings {
        Settings::default()
    }

    #[test]
    fn every_key_writes_its_field() {
        let mut settings = fixture();
        apply(&mut settings, "temperature", "0.4").expect("temperature");
        assert_eq!(settings.workspace.agent.temperature, Some(0.4));
        apply(&mut settings, "max-tokens", "8K").expect("max tokens");
        assert_eq!(settings.workspace.agent.max_tokens, Some(8 * 1024));
        apply(&mut settings, "context-budget", "64K").expect("budget");
        assert_eq!(settings.workspace.agent.context_budget, 64 * 1024);
        apply(&mut settings, "verify-command", "cargo test").expect("verify");
        assert_eq!(
            settings.workspace.agent.verify_command.as_deref(),
            Some("cargo test")
        );
        apply(&mut settings, "llm-timeout", "90").expect("timeout");
        assert_eq!(settings.workspace.agent.llm_timeout_secs, 90);
        apply(&mut settings, "llm-retries", "4").expect("retries");
        assert_eq!(settings.workspace.agent.llm_max_retries, 4);
        apply(&mut settings, "shell-timeout", "45").expect("shell");
        assert_eq!(settings.workspace.agent.shell_timeout_secs, 45);
        apply(
            &mut settings,
            "web-search-base-url",
            "https://example.test/search",
        )
        .expect("base url");
        assert_eq!(
            settings.global.web_search.base_url,
            "https://example.test/search"
        );
        apply(&mut settings, "web-search-results", "12").expect("results");
        assert_eq!(settings.global.web_search.results_per_search, 12);
        apply(&mut settings, "shell", "off").expect("shell switch");
        assert!(!settings.workspace.agent.bash_enabled);
        apply(&mut settings, "files", "false").expect("files switch");
        assert!(!settings.workspace.agent.file_tools_enabled);
        apply(&mut settings, "hosted-web-search", "on").expect("web switch");
        assert!(settings.workspace.agent.web_tools_enabled);
        apply(&mut settings, "parallel-tool-calls", "yes").expect("parallel switch");
        assert!(settings.workspace.agent.parallel_tool_calls);
    }

    #[test]
    fn a_switch_takes_the_words_a_toggle_can_write_and_refuses_a_blank() {
        let mut settings = fixture();
        apply(&mut settings, "shell", "ON").expect("upper case is the same switch");
        assert!(settings.workspace.agent.bash_enabled);
        for value in ["", "maybe", "2"] {
            assert!(
                apply(&mut settings, "shell", value).is_err(),
                "{value} is not a switch"
            );
        }
    }

    #[test]
    fn an_empty_value_clears_the_optional_fields() {
        let mut settings = fixture();
        settings.workspace.agent.temperature = Some(0.6);
        settings.workspace.agent.max_tokens = Some(4096);
        settings.workspace.agent.verify_command = Some("cargo test".to_string());
        settings.global.web_search.base_url = "https://example.test/search".to_string();
        for key in [
            "temperature",
            "max-tokens",
            "verify-command",
            "web-search-base-url",
        ] {
            apply(&mut settings, key, "  ").expect("clearing is a write");
        }
        assert_eq!(settings.workspace.agent.temperature, None);
        assert_eq!(settings.workspace.agent.max_tokens, None);
        assert_eq!(settings.workspace.agent.verify_command, None);
        assert_eq!(
            settings.global.web_search.base_url,
            crate::web::DEFAULT_BASE_URL
        );
    }

    #[test]
    fn an_unknown_key_is_refused_with_the_keys_the_host_has() {
        let error = apply(&mut fixture(), "temperature-invalid", "1").expect_err("refused");
        assert!(error.contains("no setting is named"), "{error}");
        for key in EDITABLE_KEYS {
            assert!(error.contains(key), "{key} is named: {error}");
        }
    }

    #[test]
    fn a_temperature_outside_the_sampling_range_is_refused_not_clamped() {
        for value in ["-0.1", "2.1", "NaN", "warm"] {
            let error = apply(&mut fixture(), "temperature", value)
                .expect_err("a non-sampling temperature is refused");
            assert!(error.contains("0 to 2"), "{value}: {error}");
        }
        // The bounds themselves are values the engine accepts.
        let mut settings = fixture();
        apply(&mut settings, "temperature", "0").expect("zero");
        apply(&mut settings, "temperature", "2").expect("two");
        assert_eq!(settings.workspace.agent.temperature, Some(2.0));
    }

    #[test]
    fn a_zero_output_cap_is_stored_as_no_cap() {
        let mut settings = fixture();
        apply(&mut settings, "max-tokens", "0").expect("zero clears");
        assert_eq!(settings.workspace.agent.max_tokens, None);
    }

    #[test]
    fn counts_accept_suffixes_grouping_and_refuse_words() {
        let mut settings = fixture();
        apply(&mut settings, "max-tokens", "12,800").expect("grouped");
        assert_eq!(settings.workspace.agent.max_tokens, Some(12_800));
        apply(&mut settings, "max-tokens", "1M").expect("megabytes");
        assert_eq!(settings.workspace.agent.max_tokens, Some(1_048_576));
        for value in ["lots", "8T", "-1"] {
            assert!(
                apply(&mut settings, "max-tokens", value).is_err(),
                "{value} is not a count"
            );
        }
        // A count wider than the wire cannot carry is refused, not clamped.
        assert!(apply(&mut settings, "max-tokens", "9000000000000").is_err());
    }

    #[test]
    fn timeouts_have_a_floor_of_one_second() {
        let mut settings = fixture();
        apply(&mut settings, "llm-timeout", "0").expect("zero is floored");
        assert_eq!(settings.workspace.agent.llm_timeout_secs, 1);
        apply(&mut settings, "shell-timeout", "0").expect("zero is floored");
        assert_eq!(settings.workspace.agent.shell_timeout_secs, 1);
        assert!(apply(&mut settings, "llm-timeout", "soon").is_err());
        assert!(apply(&mut settings, "shell-timeout", "later").is_err());
    }

    #[test]
    fn retries_are_capped_instead_of_refused() {
        let mut settings = fixture();
        apply(&mut settings, "llm-retries", "11").expect("capped");
        assert_eq!(settings.workspace.agent.llm_max_retries, 10);
        assert!(apply(&mut settings, "llm-retries", "-1").is_err());
    }

    #[test]
    fn a_search_count_is_clamped_into_the_tools_range() {
        let mut settings = fixture();
        apply(&mut settings, "web-search-results", "0").expect("clamped up");
        assert_eq!(settings.global.web_search.results_per_search, 1);
        apply(&mut settings, "web-search-results", "500").expect("clamped down");
        assert_eq!(
            settings.global.web_search.results_per_search,
            crate::web::MAX_RESULTS
        );
        assert!(apply(&mut settings, "web-search-results", "many").is_err());
    }

    #[test]
    fn a_blank_context_budget_is_refused_because_zero_is_the_off_switch() {
        let mut settings = fixture();
        let error = apply(&mut settings, "context-budget", "")
            .expect_err("a blank budget is not a capacity");
        assert!(error.contains("0 turns the budget off"), "{error}");
        apply(&mut settings, "context-budget", "0").expect("off");
        assert_eq!(settings.workspace.agent.context_budget, 0);
    }

    #[test]
    fn the_clearing_list_matches_the_keys_that_clear() {
        let mut settings = fixture();
        settings.workspace.agent.temperature = Some(0.6);
        settings.workspace.agent.max_tokens = Some(1024);
        settings.workspace.agent.verify_command = Some("cargo test".to_string());
        for key in EDITABLE_KEYS {
            let cleared = apply(&mut settings, key, "").is_ok();
            assert_eq!(
                cleared,
                accepts_empty(key),
                "{key}: the table and the writer disagree about clearing"
            );
        }
    }
}
