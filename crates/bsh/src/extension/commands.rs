//! The command seam: the shell tool a session's turn already built.
//!
//! A scheduled job must run under the SAME policy, sandbox and preset as a turn
//! in that session. Rebuilding a tool from settings would duplicate the
//! permission decision; instead the agent build publishes the tool it built, and
//! the scheduler looks it up by session. A session that never built an agent has
//! no tool, and the job is refused with a reason rather than run unconfined.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use agent::tools::Tool;

fn installed() -> &'static Mutex<HashMap<String, Arc<dyn Tool>>> {
    static INSTALLED: OnceLock<Mutex<HashMap<String, Arc<dyn Tool>>>> = OnceLock::new();
    INSTALLED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Publish the shell tool a session's turn built.
pub(crate) fn install_command_tool(session: &str, tool: Arc<dyn Tool>) {
    if let Ok(mut tools) = installed().lock() {
        tools.insert(session.to_string(), tool);
    }
}

/// The shell tool for a session, if its agent build published one.
pub(crate) fn command_tool(session: &str) -> Option<Arc<dyn Tool>> {
    installed()
        .lock()
        .ok()
        .and_then(|tools| tools.get(session).cloned())
}

/// Drop every published tool. Tests call this so one case cannot see another's.
pub(crate) fn clear_command_tools() {
    if let Ok(mut tools) = installed().lock() {
        tools.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Echo {
        calls: Arc<AtomicUsize>,
    }

    impl Tool for Echo {
        fn name(&self) -> &str {
            "bash"
        }

        fn description(&self) -> String {
            "echo".to_string()
        }

        fn run(&self, _input: &Value) -> Result<Value, agent::tools::ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({ "job_id": "job-1" }))
        }
    }

    #[test]
    fn a_published_tool_is_found_by_session() {
        clear_command_tools();
        let calls = Arc::new(AtomicUsize::new(0));
        install_command_tool(
            "ses-1",
            Arc::new(Echo {
                calls: calls.clone(),
            }),
        );
        let tool = command_tool("ses-1").expect("published");
        assert!(tool.run(&serde_json::json!({})).is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(command_tool("ses-2").is_none());
        clear_command_tools();
        assert!(command_tool("ses-1").is_none());
    }
}
