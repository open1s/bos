//! The managed terminal: a local PTY whose every command is gated by a
//! Rego policy before it runs.
//!
//! The pane owns a [`portable_pty`] child, a line buffer that accumulates
//! output, and a [`regorus`] engine that decides whether a command is allowed.
//! The gate is the point: a reader can type freely, but nothing reaches the
//! shell until the policy says it may.

use std::path::PathBuf;

use regorus::{Engine as RegoEngine, Value as RegoValue};

/// The terminal's screen: a scrollback of output lines.
#[derive(Clone, Debug, Default)]
pub(crate) struct Screen {
    /// Output lines, oldest first.
    pub(crate) lines: Vec<String>,
    /// The maximum number of lines kept in scrollback.
    pub(crate) max_lines: usize,
}

impl Screen {
    /// An empty screen with the given scrollback limit.
    pub(crate) fn new(max_lines: usize) -> Self {
        Self {
            lines: Vec::new(),
            max_lines,
        }
    }

    /// Append a line to the scrollback, dropping the oldest when full.
    pub(crate) fn push_line(&mut self, line: String) {
        if self.lines.len() >= self.max_lines {
            self.lines.remove(0);
        }
        self.lines.push(line);
    }
}

/// The regorus gate that decides whether a command may run.
///
/// The engine holds one Rego policy that receives the command as input and
/// returns a boolean. The default policy allows everything; a deployment
/// replaces it with its own rules.
pub(crate) struct Gate {
    engine: RegoEngine,
}

impl Gate {
    /// A gate whose policy allows every command.
    pub(crate) fn allow_all() -> Self {
        let mut engine = RegoEngine::new();
        let policy = r#"
            package terminal

            default allow := true

            allow := false if {
                input.deny
            }
        "#;
        engine
            .add_policy("terminal.rego".to_string(), policy.to_string())
            .expect("the default policy is valid Rego");
        Self { engine }
    }

    /// Whether the command is permitted to run.
    pub(crate) fn allows(&mut self, command: &str) -> bool {
        let input = RegoValue::from_json_str(
            &serde_json::json!({
                "command": command,
                "deny": false,
            })
            .to_string(),
        );
        match input {
            Ok(v) => {
                self.engine.set_input(v);
                self.engine
                    .eval_bool_query("data.terminal.allow".to_string(), false)
                    .unwrap_or(false)
            }
            Err(_) => false,
        }
    }

    /// Load an additional policy module.
    ///
    /// Deployments narrow the gate by adding their own modules: a later
    /// policy's decisions layer over the default's allow-all baseline.
    pub(crate) fn load_policy(&mut self, id: &str, source: &str) -> anyhow::Result<()> {
        self.engine.add_policy(id.to_string(), source.to_string())?;
        Ok(())
    }
}

/// A running terminal: the PTY child, its writer, the screen, and the gate.
///
/// The PTY itself is wired in a later increment; this holds the parts the
/// pane reads and the gate the commands pass through.
pub(crate) struct Terminal {
    /// The screen the reader sees.
    pub(crate) screen: Screen,
    /// The gate every command passes through before it runs.
    pub(crate) gate: Gate,
    /// The shell's working directory.
    pub(crate) cwd: PathBuf,
}

impl Terminal {
    /// A terminal that is not yet attached to a shell.
    ///
    /// The pane shows this state while the PTY spawns, so the reader sees
    /// the screen and the gate rather than an empty panel. A policy the
    /// workspace carries at `.bsh/terminal-policy.rego` is loaded into the
    /// gate, so a workspace can narrow what its terminal may run.
    pub(crate) fn new(cwd: PathBuf) -> Self {
        let mut gate = Gate::allow_all();
        let policy_path = cwd.join(".bsh").join("terminal-policy.rego");
        if let Ok(policy) = std::fs::read_to_string(&policy_path) {
            // A workspace policy that fails to parse leaves the default gate
            // in force: an unusable rule must never widen the gate.
            let _ = gate.load_policy("workspace.rego", &policy);
        }
        Self {
            screen: Screen::new(1000),
            gate,
            cwd,
        }
    }

    /// Whether the command passes the gate.
    pub(crate) fn allows(&mut self, command: &str) -> bool {
        self.gate.allows(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_screen_is_blank() {
        let screen = Screen::new(1000);
        assert!(screen.lines.is_empty());
    }

    #[test]
    fn push_line_adds_a_new_row() {
        let mut screen = Screen::new(1000);
        screen.push_line("first".to_string());
        screen.push_line("second".to_string());
        assert_eq!(screen.lines, vec!["first", "second"]);
    }

    #[test]
    fn scrollback_drops_the_oldest_line_when_full() {
        let mut screen = Screen::new(2);
        screen.push_line("one".to_string());
        screen.push_line("two".to_string());
        screen.push_line("three".to_string());
        assert_eq!(screen.lines, vec!["two", "three"]);
    }

    #[test]
    fn the_default_gate_allows_everything() {
        let mut gate = Gate::allow_all();
        assert!(gate.allows("ls -la"));
        assert!(gate.allows("rm -rf /"));
        assert!(gate.allows(""));
    }
}
