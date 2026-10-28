//! Subagent runs: the child tasks a turn delegates and the header lists.
//!
//! The registry is shared between the tool that runs a child agent and the
//! state the conversation reads, so a run is visible while it is still
//! streaming. It holds only the small public facts the header needs - title,
//! prompt, status, timing and token usage - never the child's context.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// The lifecycle state of one subagent run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SubagentStatus {
    /// The child agent is running.
    Running,
    /// The child returned a final answer.
    Done,
    /// The child's turn ended in an error.
    Failed,
}

impl SubagentStatus {
    /// Whether this run still occupies a concurrency slot.
    pub(crate) fn is_running(self) -> bool {
        matches!(self, SubagentStatus::Running)
    }
}

/// One child transcript row, flattened from the child's session.
#[derive(Clone, Debug)]
pub(crate) struct SubagentMessage {
    /// user, assistant or tool.
    pub(crate) role: String,
    /// The message text or tool output.
    pub(crate) content: String,
    /// The tool name, when this row is a tool call or result.
    pub(crate) tool: Option<String>,
}

/// One subagent run as the header lists it.
#[derive(Clone, Debug)]
pub(crate) struct SubagentRun {
    /// Stable run id, also used by the menu row.
    pub(crate) id: String,
    /// The root conversation the run belongs to.
    pub(crate) session: String,
    /// The run id of the direct parent, or the session id at the root.
    pub(crate) parent: String,
    /// How deep below the session root this run sits.
    pub(crate) depth: usize,
    /// The short description the model supplied.
    pub(crate) title: String,
    /// The task text handed to the child.
    pub(crate) prompt: String,
    /// Lifecycle state.
    pub(crate) status: SubagentStatus,
    /// Epoch milliseconds when the child started.
    pub(crate) started_at: u64,
    /// Epoch milliseconds when the child finished, when it has.
    pub(crate) finished_at: Option<u64>,
    /// A rough token estimate shown while the child has not reported usage.
    pub(crate) estimated_tokens: u64,
    /// Prompt tokens the child reported.
    pub(crate) prompt_tokens: u64,
    /// Completion tokens the child reported.
    pub(crate) completion_tokens: u64,
    /// The child's final answer, when it finished.
    pub(crate) result: Option<String>,
    /// The failure text, when it failed.
    pub(crate) error: Option<String>,
    /// Monotonic counter for live transcript/token updates.
    pub(crate) revision: u64,
    /// The child's flattened transcript.
    pub(crate) transcript: Vec<SubagentMessage>,
}

impl SubagentStatus {
    /// The persisted spelling.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    /// Read the persisted spelling; an unknown value is a finished run.
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "running" => Self::Running,
            "failed" => Self::Failed,
            _ => Self::Done,
        }
    }
}

impl SubagentRun {
    /// The child transcript as the main chat rows expect it.
    ///
    /// Tool calls become ToolEvent entries on the preceding assistant row and a
    /// following result fills that event's output, so the child can be rendered
    /// by the same transcript component as a normal session.
    pub(crate) fn chat_messages(&self) -> Vec<crate::model::ChatMessage> {
        use crate::model::{ChatMessage, Role, ToolEvent};
        let mut out: Vec<ChatMessage> = Vec::new();
        for row in &self.transcript {
            match row.role.as_str() {
                "user" => out.push(ChatMessage::user(row.content.clone())),
                "assistant" => {
                    let mut message = ChatMessage::assistant();
                    message.text = row.content.clone();
                    out.push(message);
                }
                _ => {
                    if let Some(name) = &row.tool {
                        let args = row
                            .content
                            .strip_prefix(name)
                            .map(str::trim)
                            .unwrap_or(row.content.as_str())
                            .to_string();
                        let event = ToolEvent {
                            name: name.clone(),
                            args,
                            output: None,
                            ms: None,
                            at: None,
                        };
                        if let Some(last) = out.last_mut() {
                            if last.role == Role::Assistant {
                                last.tools.push(event);
                                continue;
                            }
                        }
                        let mut message = ChatMessage::assistant();
                        message.tools.push(event);
                        out.push(message);
                    } else if let Some(last) = out.iter_mut().rev().find(|message| {
                        message.role == Role::Assistant
                            && message
                                .tools
                                .last()
                                .is_some_and(|tool| tool.output.is_none())
                    }) {
                        if let Some(tool) = last.tools.last_mut() {
                            tool.output = Some(row.content.clone());
                        }
                    } else {
                        let mut message = ChatMessage::assistant();
                        message.text = row.content.clone();
                        out.push(message);
                    }
                }
            }
        }
        // The engine must record one assistant row per answer, but keep the
        // view model honest if an older transcript already carries a duplicate.
        out.dedup_by(|a, b| {
            a.role == Role::Assistant
                && b.role == Role::Assistant
                && a.text == b.text
                && a.tools.is_empty()
                && b.tools.is_empty()
        });
        out
    }

    /// The metadata document saved beside the child's transcript.
    pub(crate) fn meta(&self) -> crate::model::SubagentMeta {
        crate::model::SubagentMeta {
            root_id: self.session.clone(),
            parent_id: self.parent.clone(),
            depth: self.depth,
            status: self.status.as_str().to_string(),
            prompt: self.prompt.clone(),
            estimated_tokens: self.estimated_tokens,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            started_at: self.started_at,
            finished_at: self.finished_at,
            result: self.result.clone(),
            error: self.error.clone(),
        }
    }

    /// Total tokens the child reported.
    pub(crate) fn total_tokens(&self) -> u64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }

    /// Milliseconds the run has taken, saturating at zero.
    pub(crate) fn elapsed_ms(&self) -> u64 {
        let end = self.finished_at.unwrap_or_else(now_ms);
        end.saturating_sub(self.started_at)
    }
}

/// The shared run registry.
#[derive(Default)]
pub(crate) struct SubagentRegistry {
    runs: Mutex<Vec<SubagentRun>>,
    /// The live child agent behind each settled run, so the reader can send a
    /// follow-up into the same conversation.
    agents: Mutex<HashMap<String, Arc<tokio::sync::Mutex<agent::Agent>>>>,
    next: AtomicU64,
}

/// How many child runs may occupy the machine at once.
pub(crate) const SUBAGENT_SLOTS: usize = 3;

impl SubagentRegistry {
    /// Start one run and return its id.
    ///
    /// Test-only now that the tool reserves a slot atomically through
    /// [`Self::try_begin`]; the tests still want the unconditional form.
    #[cfg(test)]
    pub(crate) fn begin(
        &self,
        session: &str,
        parent: &str,
        depth: usize,
        estimated_tokens: u64,
        title: &str,
        prompt: &str,
    ) -> String {
        let _ = self.next.fetch_add(1, Ordering::Relaxed);
        let id = format!("sub-{}", Uuid::new_v4());
        let run = SubagentRun {
            id: id.clone(),
            session: session.to_string(),
            parent: parent.to_string(),
            depth,
            title: title.to_string(),
            prompt: prompt.to_string(),
            status: SubagentStatus::Running,
            started_at: now_ms(),
            finished_at: None,
            estimated_tokens,
            prompt_tokens: 0,
            completion_tokens: 0,
            result: None,
            error: None,
            revision: 0,
            transcript: Vec::new(),
        };
        if let Ok(mut runs) = self.runs.lock() {
            runs.push(run);
        }
        id
    }

    /// Start one run only when a concurrency slot is free.
    ///
    /// The check and the insert share one lock: a batch of parallel subagent
    /// calls cannot all observe the same free slot and overrun the cap.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_begin(
        &self,
        session: &str,
        parent: &str,
        depth: usize,
        estimated_tokens: u64,
        title: &str,
        prompt: &str,
        limit: usize,
    ) -> Option<String> {
        let _ = self.next.fetch_add(1, Ordering::Relaxed);
        let id = format!("sub-{}", Uuid::new_v4());
        let run = SubagentRun {
            id: id.clone(),
            session: session.to_string(),
            parent: parent.to_string(),
            depth,
            title: title.to_string(),
            prompt: prompt.to_string(),
            status: SubagentStatus::Running,
            started_at: now_ms(),
            finished_at: None,
            estimated_tokens,
            prompt_tokens: 0,
            completion_tokens: 0,
            result: None,
            error: None,
            revision: 0,
            transcript: Vec::new(),
        };
        let mut runs = self.runs.lock().ok()?;
        if runs.iter().filter(|run| run.status.is_running()).count() >= limit {
            return None;
        }
        runs.push(run);
        Some(id)
    }

    /// One run's current snapshot, when it exists.
    pub(crate) fn run(&self, id: &str) -> Option<SubagentRun> {
        self.runs
            .lock()
            .ok()?
            .iter()
            .find(|run| run.id == id)
            .cloned()
    }

    /// Mark one run done with its final answer and token usage.
    pub(crate) fn finish(
        &self,
        id: &str,
        result: String,
        usage: (u64, u64),
        transcript: Vec<SubagentMessage>,
    ) {
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(run) = runs.iter_mut().find(|run| run.id == id) {
                run.status = SubagentStatus::Done;
                run.finished_at = Some(now_ms());
                run.revision += 1;
                run.prompt_tokens = usage.0;
                run.completion_tokens = usage.1;
                run.result = Some(result);
                run.transcript = transcript;
            }
        }
    }

    /// Mark one run failed with the error text.
    pub(crate) fn fail(
        &self,
        id: &str,
        error: String,
        usage: Option<(u64, u64)>,
        transcript: Vec<SubagentMessage>,
    ) {
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(run) = runs.iter_mut().find(|run| run.id == id) {
                run.status = SubagentStatus::Failed;
                run.finished_at = Some(now_ms());
                run.revision += 1;
                if let Some((prompt, completion)) = usage {
                    run.prompt_tokens = prompt;
                    run.completion_tokens = completion;
                }
                run.error = Some(error);
                run.transcript = transcript;
            }
        }
    }

    /// Keep the live child agent behind a run for follow-up input.
    pub(crate) fn set_agent(&self, id: &str, agent: Arc<tokio::sync::Mutex<agent::Agent>>) {
        if let Ok(mut agents) = self.agents.lock() {
            agents.insert(id.to_string(), agent);
        }
    }

    /// The live child agent behind a run.
    pub(crate) fn agent(&self, id: &str) -> Option<Arc<tokio::sync::Mutex<agent::Agent>>> {
        self.agents
            .lock()
            .ok()
            .and_then(|agents| agents.get(id).cloned())
    }

    /// Append a user follow-up and put the run back into Running.
    pub(crate) fn begin_followup(&self, id: &str, prompt: &str) -> bool {
        let Ok(mut runs) = self.runs.lock() else {
            return false;
        };
        let Some(run) = runs.iter_mut().find(|run| run.id == id) else {
            return false;
        };
        if run.status.is_running() {
            return false;
        }
        run.status = SubagentStatus::Running;
        run.revision += 1;
        run.finished_at = None;
        run.result = None;
        run.error = None;
        run.transcript.push(SubagentMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
            tool: None,
        });
        true
    }

    /// Append streamed assistant text to the live transcript.
    pub(crate) fn append_text(&self, id: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(run) = runs.iter_mut().find(|run| run.id == id) {
                if let Some(last) = run.transcript.last_mut() {
                    if last.role == "assistant" && last.tool.is_none() {
                        last.content.push_str(text);
                        run.revision += 1;
                        return;
                    }
                }
                run.transcript.push(SubagentMessage {
                    role: "assistant".to_string(),
                    content: text.to_string(),
                    tool: None,
                });
                run.revision += 1;
            }
        }
    }

    /// Record a live tool call.
    pub(crate) fn append_tool_call(&self, id: &str, name: &str, args: &str) {
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(run) = runs.iter_mut().find(|run| run.id == id) {
                run.transcript.push(SubagentMessage {
                    role: "tool".to_string(),
                    content: format!("{name} {args}"),
                    tool: Some(name.to_string()),
                });
                run.revision += 1;
            }
        }
    }

    /// Record a live tool result.
    pub(crate) fn append_tool_result(&self, id: &str, output: &str) {
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(run) = runs.iter_mut().find(|run| run.id == id) {
                run.transcript.push(SubagentMessage {
                    role: "tool".to_string(),
                    content: output.to_string(),
                    tool: None,
                });
                run.revision += 1;
            }
        }
    }

    /// Update usage while the child is still running.
    pub(crate) fn set_usage(&self, id: &str, prompt: u64, completion: u64) {
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(run) = runs.iter_mut().find(|run| run.id == id) {
                run.prompt_tokens = prompt;
                run.completion_tokens = completion;
                run.revision += 1;
            }
        }
    }

    /// Put a persisted run back into the menu.
    pub(crate) fn restore(&self, run: SubagentRun) {
        if let Ok(mut runs) = self.runs.lock() {
            if runs.iter().any(|existing| existing.id == run.id) {
                return;
            }
            runs.push(run);
            runs.sort_by_key(|run| run.started_at);
        }
    }

    /// Turn a saved Session transcript back into the flattened rows the
    /// registry keeps for the menu and preview.
    pub(crate) fn messages_from_chat(
        messages: &[crate::model::ChatMessage],
    ) -> Vec<SubagentMessage> {
        use crate::model::Role;
        let mut rows = Vec::new();
        for message in messages {
            match message.role {
                Role::User => rows.push(SubagentMessage {
                    role: "user".to_string(),
                    content: message.text.clone(),
                    tool: None,
                }),
                Role::Assistant => {
                    if !message.text.is_empty() {
                        rows.push(SubagentMessage {
                            role: "assistant".to_string(),
                            content: message.text.clone(),
                            tool: None,
                        });
                    }
                    for tool in &message.tools {
                        rows.push(SubagentMessage {
                            role: "tool".to_string(),
                            content: format!("{} {}", tool.name, tool.args),
                            tool: Some(tool.name.clone()),
                        });
                        if let Some(output) = &tool.output {
                            rows.push(SubagentMessage {
                                role: "tool".to_string(),
                                content: output.clone(),
                                tool: None,
                            });
                        }
                    }
                }
            }
        }
        rows
    }

    /// A snapshot in start order.
    pub(crate) fn snapshot(&self) -> Vec<SubagentRun> {
        self.runs
            .lock()
            .map(|runs| runs.clone())
            .unwrap_or_default()
    }

    /// How many children are currently running.
    pub(crate) fn running_count(&self) -> usize {
        self.runs
            .lock()
            .map(|runs| runs.iter().filter(|run| run.status.is_running()).count())
            .unwrap_or(0)
    }
}

/// Epoch milliseconds.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A background start reserves its slot under one lock, so parallel
    /// subagent calls cannot all pass the same `running_count` check.
    #[test]
    fn try_begin_reserves_a_slot_and_run_reads_it() {
        let registry = SubagentRegistry::default();
        let first = registry
            .try_begin("s", "s", 1, 0, "A", "a", 1)
            .expect("the first slot");
        assert!(
            registry.try_begin("s", "s", 1, 0, "B", "b", 1).is_none(),
            "the only slot is taken"
        );
        assert_eq!(
            registry.run(&first).expect("the reserved run").status,
            SubagentStatus::Running
        );
        registry.finish(&first, "ok".to_string(), (1, 2), Vec::new());
        assert!(
            registry.try_begin("s", "s", 1, 0, "C", "c", 1).is_some(),
            "a finished run frees its slot"
        );
    }

    #[test]
    fn a_run_moves_from_running_to_done() {
        let registry = SubagentRegistry::default();
        let id = registry.begin("parent", "parent", 0, 0, "Inspect", "Look at the files");
        assert_eq!(registry.running_count(), 1);
        let run = &registry.snapshot()[0];
        assert_eq!(run.status, SubagentStatus::Running);
        assert_eq!(run.parent, "parent");
        registry.finish(
            &id,
            "done".to_string(),
            (10, 20),
            vec![SubagentMessage {
                role: "assistant".to_string(),
                content: "done".to_string(),
                tool: None,
            }],
        );
        let run = &registry.snapshot()[0];
        assert_eq!(run.status, SubagentStatus::Done);
        assert_eq!(run.total_tokens(), 30);
        assert_eq!(run.result.as_deref(), Some("done"));
        assert_eq!(run.transcript.len(), 1);
        assert_eq!(run.transcript[0].content, "done");
        assert_eq!(registry.running_count(), 0);
    }

    #[test]
    fn a_run_keeps_its_lineage() {
        let registry = SubagentRegistry::default();
        let top = registry.begin("session", "session", 1, 0, "Top", "task");
        let child = registry.begin("session", &top, 2, 0, "Child", "task");
        let runs = registry.snapshot();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].session, "session");
        assert_eq!(runs[0].depth, 1);
        assert_eq!(runs[1].parent, top);
        assert_eq!(runs[1].session, "session");
        assert_eq!(runs[1].depth, 2);
        assert_ne!(runs[0].id, child);
    }

    #[test]
    fn settled_runs_are_not_evicted() {
        let registry = SubagentRegistry::default();
        for turn in 0..40 {
            let id = registry.begin("session", "session", 1, 0, "Task", &format!("job {turn}"));
            registry.finish(&id, "done".to_string(), (1, 1), Vec::new());
        }
        assert_eq!(registry.snapshot().len(), 40);
        assert_eq!(registry.running_count(), 0);
    }

    #[test]
    fn a_followup_reopens_the_child_transcript() {
        let registry = SubagentRegistry::default();
        let id = registry.begin("session", "session", 1, 0, "Task", "first");
        registry.finish(
            &id,
            "done".to_string(),
            (1, 2),
            vec![SubagentMessage {
                role: "assistant".to_string(),
                content: "done".to_string(),
                tool: None,
            }],
        );
        assert!(registry.begin_followup(&id, "one more"));
        let run = &registry.snapshot()[0];
        assert_eq!(run.status, SubagentStatus::Running);
        assert_eq!(
            run.transcript.last().map(|row| row.content.as_str()),
            Some("one more")
        );
        assert!(
            !registry.begin_followup(&id, "while busy"),
            "a running child refuses a second follow-up"
        );
    }

    #[test]
    fn live_stream_rows_move_the_transcript_and_tokens() {
        let registry = SubagentRegistry::default();
        let id = registry.begin("session", "session", 1, 0, "Task", "go");
        registry.append_text(&id, "hel");
        registry.append_text(&id, "lo");
        registry.set_usage(&id, 3, 4);
        registry.append_tool_call(&id, "read_file", "{\"path\":\"a\"}");
        registry.append_tool_result(&id, "contents");
        let run = &registry.snapshot()[0];
        assert_eq!(run.transcript[0].content, "hello");
        assert_eq!(run.total_tokens(), 7);
        assert_eq!(run.transcript[1].tool.as_deref(), Some("read_file"));
        assert_eq!(run.transcript[2].content, "contents");
        assert!(run.revision >= 5);
    }

    #[test]
    fn a_duplicate_assistant_row_is_shown_once() {
        let registry = SubagentRegistry::default();
        let id = registry.begin("session", "session", 1, 0, "Task", "say hi");
        registry.finish(
            &id,
            "hi".to_string(),
            (1, 1),
            vec![
                SubagentMessage {
                    role: "assistant".to_string(),
                    content: "hi".to_string(),
                    tool: None,
                },
                SubagentMessage {
                    role: "assistant".to_string(),
                    content: "hi".to_string(),
                    tool: None,
                },
            ],
        );
        let messages = registry.snapshot()[0].chat_messages();
        assert_eq!(messages.len(), 1, "one answer, not two");
    }

    #[test]
    fn a_run_can_fail_and_carry_usage() {
        let registry = SubagentRegistry::default();
        let id = registry.begin("parent", "parent", 0, 0, "Fail", "task");
        registry.fail(&id, "boom".to_string(), Some((3, 4)), Vec::new());
        let run = &registry.snapshot()[0];
        assert_eq!(run.status, SubagentStatus::Failed);
        assert_eq!(run.error.as_deref(), Some("boom"));
        assert_eq!(run.total_tokens(), 7);
    }
}
