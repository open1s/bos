//! External command hooks: an event matcher plus an argv command.
//!
//! An [`ExternalHook`](crate::agent::hooks::external::ExternalHook) pairs an [`EventMatcher`](crate::agent::hooks::external::EventMatcher) with a command line. A
//! [`HookDispatcher`](crate::agent::hooks::external::HookDispatcher) runs every hook whose matcher accepts the fired event and
//! reports one [`HookOutcome`](crate::agent::hooks::external::HookOutcome) per command, including the process exit status.
//! A command that cannot be spawned (for example a non-existent program) yields
//! an outcome with `exit_code: None` and a populated `error`; it never panics
//! and never prevents the remaining hooks from running.
//!
//! Context data is exported to the child process environment: the agent id is
//! available as `BOS_AGENT_ID`, and each context key becomes `BOS_HOOK_<KEY>`
//! with the key upper-cased and non-alphanumeric characters replaced by `_`.

use std::process::Command;
use std::time::{Duration, Instant};

use super::{HookContext, HookEvent};

/// The default bound on one hook command. A hook that hangs must not block
/// event dispatch indefinitely (issue #319).
pub const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// Selects the events an [`ExternalHook`] reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventMatcher {
    /// Matches every event.
    Any,
    /// Matches exactly one event.
    Exact(HookEvent),
    /// Matches any of the listed events.
    OneOf(Vec<HookEvent>),
}

impl EventMatcher {
    /// Whether this matcher accepts `event`.
    pub fn matches(&self, event: &HookEvent) -> bool {
        match self {
            EventMatcher::Any => true,
            EventMatcher::Exact(only) => only == event,
            EventMatcher::OneOf(events) => events.iter().any(|candidate| candidate == event),
        }
    }
}

/// An event matcher paired with a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalHook {
    /// Which events trigger the command.
    pub matcher: EventMatcher,
    /// Program and arguments; `argv[0]` is the program to spawn.
    pub argv: Vec<String>,
}

impl ExternalHook {
    /// Create a hook from a matcher and an argv command.
    pub fn new(matcher: EventMatcher, argv: Vec<String>) -> Self {
        Self { matcher, argv }
    }

    /// Create a hook that runs `argv` for one specific event.
    pub fn on(event: HookEvent, argv: Vec<String>) -> Self {
        Self::new(EventMatcher::Exact(event), argv)
    }

    /// Create a hook that runs `argv` for every event.
    pub fn any(argv: Vec<String>) -> Self {
        Self::new(EventMatcher::Any, argv)
    }

    /// Whether this hook fires for `event`.
    pub fn matches(&self, event: &HookEvent) -> bool {
        self.matcher.matches(event)
    }
}

/// The result of running one external hook command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookOutcome {
    /// The command line that was run.
    pub argv: Vec<String>,
    /// Process exit code, or `None` when the process could not be spawned or was
    /// killed by a signal.
    pub exit_code: Option<i32>,
    /// Whether the command exited with status 0.
    pub succeeded: bool,
    /// Spawn or wait error, when one occurred.
    pub error: Option<String>,
}

/// Runs external hook commands for matching events.
///
/// Every command runs through the shared cross-platform process runtime with
/// a bounded execution time: a hung hook is terminated together with its
/// whole process tree, and the remaining hooks still run (issue #319).
#[derive(Debug, Clone)]
pub struct HookDispatcher {
    hooks: Vec<ExternalHook>,
    timeout: Duration,
}

impl Default for HookDispatcher {
    fn default() -> Self {
        Self {
            hooks: Vec::new(),
            timeout: DEFAULT_HOOK_TIMEOUT,
        }
    }
}

impl HookDispatcher {
    /// Create an empty dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the per-command execution bound.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add a hook and return the dispatcher for chaining.
    pub fn register(&mut self, hook: ExternalHook) -> &mut Self {
        self.hooks.push(hook);
        self
    }

    /// The registered hooks, in registration order.
    pub fn hooks(&self) -> &[ExternalHook] {
        &self.hooks
    }

    /// Hooks whose matcher accepts `event`.
    pub fn matching(&self, event: &HookEvent) -> Vec<&ExternalHook> {
        self.hooks
            .iter()
            .filter(|hook| hook.matches(event))
            .collect()
    }

    /// Run every matching hook for `event` with `context`, in registration
    /// order, and return one outcome per command.
    pub fn dispatch(&self, event: &HookEvent, context: &HookContext) -> Vec<HookOutcome> {
        self.matching(event)
            .into_iter()
            .map(|hook| Self::run_bounded(&hook.argv, context, self.timeout))
            .collect()
    }

    /// Run every matching hook for `event` with an empty context.
    pub fn dispatch_event(&self, event: &HookEvent) -> Vec<HookOutcome> {
        self.dispatch(event, &HookContext::new(""))
    }

    /// Spawn `argv` and wait for it, returning its exit status.
    pub fn run(argv: &[String]) -> HookOutcome {
        Self::run_with_context(argv, &HookContext::new(""))
    }

    /// Spawn argv with context exported to the environment and wait for it,
    /// bounded by [DEFAULT_HOOK_TIMEOUT].
    pub fn run_with_context(argv: &[String], context: &HookContext) -> HookOutcome {
        Self::run_bounded(argv, context, DEFAULT_HOOK_TIMEOUT)
    }

    /// Spawn argv through the shared process runtime and wait, bounded.
    ///
    /// argv semantics are preserved: the program is spawned directly, never
    /// wrapped in a shell. The child owns its process tree, so a timeout
    /// terminates every descendant, not just the direct child (issue #319).
    fn run_bounded(argv: &[String], context: &HookContext, timeout: Duration) -> HookOutcome {
        let failure = |error: String| HookOutcome {
            argv: argv.to_vec(),
            exit_code: None,
            succeeded: false,
            error: Some(error),
        };
        let Some((program, args)) = argv.split_first() else {
            return failure("empty argv: no program to run".to_string());
        };

        let mut command = Command::new(program);
        command.args(args);
        command.env("BOS_AGENT_ID", &context.agent_id);
        for (key, value) in &context.data {
            command.env(format!("BOS_HOOK_{}", env_key(key)), value);
        }

        let mut child = match hostkit::proc::OwnedChild::spawn(&mut command) {
            Ok(child) => child,
            Err(err) => return failure(err.to_string()),
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return HookOutcome {
                        argv: argv.to_vec(),
                        exit_code: status.code(),
                        succeeded: status.success(),
                        error: None,
                    };
                }
                Ok(None) => {}
                Err(err) => return failure(err.to_string()),
            }
            if Instant::now() >= deadline {
                let report = child.terminate(Duration::from_millis(500));
                return failure(format!(
                    "hook timed out after {}s; process tree termination {}",
                    timeout.as_secs(),
                    if report.confirmed() {
                        "confirmed"
                    } else {
                        "could not be confirmed"
                    }
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn env_key(key: &str) -> String {
    key.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn echo_path() -> Option<String> {
        let path = "/bin/echo";
        Path::new(path).exists().then(|| path.to_string())
    }

    #[test]
    fn echo_command_succeeds() {
        let Some(echo) = echo_path() else {
            return;
        };
        let outcome = HookDispatcher::run(&[echo, "hello".to_string()]);
        assert!(outcome.succeeded, "{outcome:?}");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.error.is_none());
    }

    #[test]
    fn non_existent_command_fails_cleanly() {
        let outcome = HookDispatcher::run(&[
            "/definitely/not/a/real/binary-xyz".to_string(),
            "arg".to_string(),
        ]);
        assert!(!outcome.succeeded, "{outcome:?}");
        assert_eq!(outcome.exit_code, None);
        assert!(outcome.error.is_some());
    }

    #[test]
    fn empty_argv_is_an_error_not_a_panic() {
        let outcome = HookDispatcher::run(&[]);
        assert!(!outcome.succeeded);
        assert!(outcome.error.unwrap().contains("empty argv"));
    }

    #[test]
    fn matcher_filters_events() {
        let mut dispatcher = HookDispatcher::new();
        dispatcher.register(ExternalHook::on(
            HookEvent::BeforeToolCall,
            vec!["true".to_string()],
        ));

        assert_eq!(dispatcher.matching(&HookEvent::BeforeToolCall).len(), 1);
        assert_eq!(dispatcher.matching(&HookEvent::AfterToolCall).len(), 0);
        assert!(dispatcher
            .dispatch_event(&HookEvent::AfterToolCall)
            .is_empty());
    }

    #[test]
    fn any_matcher_matches_every_event() {
        let matcher = EventMatcher::Any;
        for event in HookEvent::ALL {
            assert!(matcher.matches(&event), "{event}");
        }

        let one_of = EventMatcher::OneOf(vec![HookEvent::OnError, HookEvent::OnComplete]);
        assert!(one_of.matches(&HookEvent::OnError));
        assert!(one_of.matches(&HookEvent::OnComplete));
        assert!(!one_of.matches(&HookEvent::OnMessage));
    }

    #[test]
    fn dispatch_runs_every_matching_hook() {
        let Some(echo) = echo_path() else {
            return;
        };
        let mut dispatcher = HookDispatcher::new();
        dispatcher.register(ExternalHook::any(vec![echo.clone(), "a".to_string()]));
        dispatcher.register(ExternalHook::on(
            HookEvent::OnComplete,
            vec![echo, "b".to_string()],
        ));

        // BeforeToolCall only matches the `Any` hook.
        let before = dispatcher.dispatch_event(&HookEvent::BeforeToolCall);
        assert_eq!(before.len(), 1);
        assert!(before[0].succeeded);

        // OnComplete matches both.
        let complete = dispatcher.dispatch_event(&HookEvent::OnComplete);
        assert_eq!(complete.len(), 2);
        assert!(complete.iter().all(|outcome| outcome.succeeded));
    }

    #[test]
    fn a_bad_command_does_not_block_later_hooks() {
        let Some(echo) = echo_path() else {
            return;
        };
        let mut dispatcher = HookDispatcher::new();
        dispatcher.register(ExternalHook::any(vec![
            "/definitely/not/a/real/binary-xyz".to_string()
        ]));
        dispatcher.register(ExternalHook::any(vec![echo, "ok".to_string()]));

        let outcomes = dispatcher.dispatch_event(&HookEvent::OnMessage);
        assert_eq!(outcomes.len(), 2);
        assert!(!outcomes[0].succeeded);
        assert!(outcomes[1].succeeded);
    }

    #[test]
    fn dispatch_returns_exit_status() {
        let Some(echo) = echo_path() else {
            return;
        };
        let mut dispatcher = HookDispatcher::new();
        dispatcher.register(ExternalHook::any(vec![echo]));
        let outcomes = dispatcher.dispatch_event(&HookEvent::OnError);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].exit_code, Some(0));
    }

    #[test]
    fn env_key_is_sanitized() {
        assert_eq!(env_key("tool-name"), "TOOL_NAME");
        assert_eq!(env_key("a.b"), "A_B");
    }

    #[test]
    fn context_is_passed_to_the_command() {
        if !Path::new("/bin/sh").exists() {
            return;
        }
        // `sh -c` asserts the exported environment, then exits 7 so we can tell
        // it really ran with the expected values.
        let mut context = HookContext::new("agent-7");
        context.set("tool-name", "bash");
        let outcome = HookDispatcher::run_with_context(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "test \"$BOS_HOOK_TOOL_NAME\" = bash && test \"$BOS_AGENT_ID\" = agent-7 && exit 7"
                    .to_string(),
            ],
            &context,
        );
        assert_eq!(outcome.exit_code, Some(7), "{outcome:?}");
        assert!(!outcome.succeeded);
        assert!(outcome.error.is_none());
    }

    /// A cross-platform echo argv: the tests must not assume /bin/echo.
    fn echo_argv() -> Vec<String> {
        if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo".to_string(),
                "hello".to_string(),
            ]
        } else {
            vec!["/bin/echo".to_string(), "hello".to_string()]
        }
    }

    #[test]
    fn echo_runs_on_every_platform() {
        let outcome = HookDispatcher::run(&echo_argv());
        assert!(outcome.succeeded, "{outcome:?}");
        assert_eq!(outcome.exit_code, Some(0));
    }

    #[test]
    fn a_hung_hook_times_out_instead_of_blocking() {
        let argv: Vec<String> = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "ping -n 30 127.0.0.1 >NUL".to_string(),
            ]
        } else {
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 30".to_string(),
            ]
        };
        let dispatcher = HookDispatcher::new().with_timeout(Duration::from_millis(300));
        let started = Instant::now();
        let outcomes = dispatcher.dispatch_event(&HookEvent::OnMessage);
        // The dispatcher has no hooks registered; run the bounded path direct.
        let outcome =
            HookDispatcher::run_bounded(&argv, &HookContext::new(""), Duration::from_millis(300));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a hung hook must not block"
        );
        assert!(!outcome.succeeded);
        assert!(outcome.error.unwrap().contains("timed out"));
        assert!(outcomes.is_empty());
    }

    /// A timed-out hook's descendants die with it: the boundary, not the
    /// direct child, is what the runtime terminates.
    #[cfg(unix)]
    #[test]
    fn timeout_terminates_the_hook_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let script = format!("sleep 30 & echo $! > {}; sleep 30", pidfile.display());
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script];
        let outcome =
            HookDispatcher::run_bounded(&argv, &HookContext::new(""), Duration::from_millis(400));
        assert!(outcome.error.unwrap().contains("timed out"));
        // Give the kill a moment to land, then prove the descendant is gone.
        std::thread::sleep(Duration::from_millis(300));
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .expect("the hook wrote its child pid")
            .trim()
            .parse()
            .expect("a pid");
        assert!(
            !crate::jobs::pid_exists(pid),
            "the descendant must be terminated"
        );
    }

    /// A program path containing spaces runs without shell reinterpretation.
    #[cfg(unix)]
    #[test]
    fn a_program_path_with_spaces_runs() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("a dir with spaces");
        std::fs::create_dir_all(&spaced).unwrap();
        let script = spaced.join("hook.sh");
        std::fs::write(&script, "#!/bin/sh\nexit 5\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let outcome = HookDispatcher::run(&[script.to_string_lossy().into_owned()]);
        assert_eq!(outcome.exit_code, Some(5), "{outcome:?}");
    }
}
