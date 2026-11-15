//! The Memory pane: the reader's surface over the Durable Record Kernel.
//!
//! One card stack in the federation pane's design language: a header with the
//! record count and the add/refresh actions, a search field with kind filters,
//! the record list (or the recall hits while searching), an add/edit form, and
//! one activity log every action reports through. Reads and writes run on the
//! runner runtime - never on the UI thread - and every mutation reports
//! pending, succeeded or failed with the kernel's typed error label.
//!
//! The pane opens its own handle through the kernel's canonical-identity
//! registry, so it shares the very store the agent and the extensions use.
//! Its viewer is the WORKSPACE scope: the reader manages every record in the
//! workspace, including session-scoped ones, and writes land at workspace
//! scope (the reader is the authority here, unlike an extension activation).

use std::sync::Arc;

use agent::memory::{
    EvidenceSource, FileMemory, Lifecycle, MemoryKind, MemoryRecord, MemoryRepository, MemoryScope,
    MemoryStore, Provenance, RecallRequest,
};
use gpui_kit::*;

use super::federation_pane::{field, solid_button};

/// A solid button whose action receives the window (the shared helper's
/// action does not, and the memory forms clear inputs, which needs it).
fn window_button<F>(
    id: impl Into<SharedString>,
    label: String,
    primary: bool,
    cx: &mut Context<Shell>,
    action: F,
) -> AnyElement
where
    F: Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
{
    let id = id.into();
    let selector = id.to_string();
    let (fg, bg) = if primary {
        (0xFFFFFF, theme::accent())
    } else {
        (theme::text(), theme::new_session_fill())
    };
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .h(px(32.0))
        .px(px(12.0))
        .rounded(px(9.0))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .bg(rgb(bg))
        .when(!primary, |button| {
            button.border_1().border_color(rgb(theme::border()))
        })
        .cursor_pointer()
        .child(txt(label, 12.0, fg).font_weight(FontWeight::MEDIUM))
        .on_click(cx.listener(move |this, _event, window, cx| {
            action(this, window, cx);
        }))
        .into_any_element()
}
use super::shell::Shell;
use super::{icon, txt, DivExt, IconName};
use crate::theme;

/// One record as the pane lists it.
#[derive(Debug, Clone)]
pub(crate) struct MemoryRow {
    /// The record id.
    pub(crate) id: String,
    /// The remembered text.
    pub(crate) content: String,
    /// The kind label (wire spelling).
    pub(crate) kind: String,
    /// A short scope label.
    pub(crate) scope: String,
    /// The lifecycle label (wire spelling).
    pub(crate) status: String,
    /// The revision counter.
    pub(crate) revision: u32,
    /// The provenance source label (wire spelling).
    pub(crate) source: String,
    /// Last update time (Unix ms).
    pub(crate) updated_at_ms: u64,
}

impl MemoryRow {
    fn from_record(record: &MemoryRecord) -> Self {
        Self {
            id: record.id.clone(),
            content: record.content.clone(),
            kind: record.kind.label().to_string(),
            scope: scope_label(&record.scope),
            status: status_label(record.status).to_string(),
            revision: record.revision,
            source: source_label(record.provenance.source).to_string(),
            updated_at_ms: record.updated_at_ms,
        }
    }
}

/// One recall hit as the pane lists it.
#[derive(Debug, Clone)]
pub(crate) struct MemoryHit {
    /// The record row.
    pub(crate) row: MemoryRow,
    /// The ordering score.
    pub(crate) score: f64,
    /// The score's named contributions, rendered for the reader.
    pub(crate) reasons: String,
}

/// The edit in flight: which record, and whether it supersedes.
#[derive(Debug, Clone)]
pub(crate) struct MemoryEdit {
    /// The record being edited or superseded.
    pub(crate) id: String,
    /// True when saving creates a successor instead of updating in place.
    pub(crate) supersede: bool,
}

/// The state every memory action reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryActivityState {
    /// The action is in flight.
    Pending,
    /// The action committed.
    Succeeded,
    /// The action was refused or failed.
    Failed,
}

/// One entry in the pane's operation log.
#[derive(Debug, Clone)]
pub(crate) struct MemoryActivity {
    /// Stable id, so an async completion settles the row it started.
    pub(crate) id: u64,
    /// The action's label key.
    pub(crate) action: &'static str,
    /// What the action touched.
    pub(crate) target: String,
    /// Where the action stands.
    pub(crate) state: MemoryActivityState,
    /// The result or the refusal, when there is one worth keeping.
    pub(crate) detail: Option<String>,
}

/// The memory pane's own state.
#[derive(Default)]
pub(crate) struct MemoryPane {
    /// The loaded records, newest first.
    pub(crate) rows: Vec<MemoryRow>,
    /// Whether the first load has settled.
    pub(crate) loaded: bool,
    /// Whether a load is in flight.
    pub(crate) loading: bool,
    /// Why the store could not be opened, when it could not.
    pub(crate) load_error: Option<String>,
    /// The shared store handle, joined through the canonical-identity registry.
    pub(crate) store: Option<Arc<FileMemory>>,
    /// Recall hits while a search is active; None lists the records.
    pub(crate) hits: Option<Vec<MemoryHit>>,
    /// The kind filter (wire spelling), or None for every kind.
    pub(crate) kind_filter: Option<String>,
    /// Whether superseded/expired/tombstoned records are listed too.
    pub(crate) show_history: bool,
    /// Whether the add form is open.
    pub(crate) add_open: bool,
    /// The kind the add form will commit (wire spelling).
    pub(crate) add_kind: String,
    /// The edit in flight, when one is.
    pub(crate) edit: Option<MemoryEdit>,
    /// The operation log, newest last, bounded.
    pub(crate) activity: Vec<MemoryActivity>,
    /// The activity id sequence.
    pub(crate) activity_seq: u64,
}

/// The wire kinds the pane offers, in display order.
const KINDS: [&str; 8] = [
    "fact",
    "preference",
    "decision",
    "lesson",
    "procedure",
    "task_state",
    "note",
    "exchange",
];

fn scope_label(scope: &MemoryScope) -> String {
    match scope {
        MemoryScope::User { user } => format!("user:{user}"),
        MemoryScope::Workspace { workspace } => format!("ws:{workspace}"),
        MemoryScope::Session { workspace, session } => format!("ws:{workspace}/{session}"),
        MemoryScope::Agent {
            workspace,
            session,
            agent,
        } => format!("ws:{workspace}/{session}/{agent}"),
    }
}

fn status_label(status: Lifecycle) -> &'static str {
    match status {
        Lifecycle::Candidate => "candidate",
        Lifecycle::Active => "active",
        Lifecycle::Superseded => "superseded",
        Lifecycle::Expired => "expired",
        Lifecycle::Tombstoned => "tombstoned",
    }
}

fn source_label(source: EvidenceSource) -> &'static str {
    match source {
        EvidenceSource::UserStatement => "user_statement",
        EvidenceSource::ModelInference => "model_inference",
        EvidenceSource::ToolEvidence => "tool_evidence",
        EvidenceSource::Legacy => "legacy",
        EvidenceSource::System => "system",
    }
}

fn kind_from_label(label: &str) -> MemoryKind {
    match label {
        "exchange" => MemoryKind::Exchange,
        "preference" => MemoryKind::Preference,
        "decision" => MemoryKind::Decision,
        "lesson" => MemoryKind::Lesson,
        "procedure" => MemoryKind::Procedure,
        "task_state" => MemoryKind::TaskState,
        "note" => MemoryKind::Note,
        _ => MemoryKind::Fact,
    }
}

/// The static locale key for a kind label.
fn kind_key(kind: &str) -> &'static str {
    match kind {
        "exchange" => "memory.kind.exchange",
        "preference" => "memory.kind.preference",
        "decision" => "memory.kind.decision",
        "lesson" => "memory.kind.lesson",
        "procedure" => "memory.kind.procedure",
        "task_state" => "memory.kind.task_state",
        "note" => "memory.kind.note",
        _ => "memory.kind.fact",
    }
}

/// The static locale key for a status label.
fn status_key(status: &str) -> &'static str {
    match status {
        "candidate" => "memory.status.candidate",
        "superseded" => "memory.status.superseded",
        "expired" => "memory.status.expired",
        "tombstoned" => "memory.status.tombstoned",
        _ => "memory.status.active",
    }
}

/// A compact age label: 5m, 3h, 2d.
fn age_label(updated_at_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mins = now.saturating_sub(updated_at_ms) / 60_000;
    if mins < 60 {
        format!("{mins}m")
    } else if mins < 24 * 60 {
        format!("{}h", mins / 60)
    } else {
        format!("{}d", mins / (24 * 60))
    }
}

impl Shell {
    /// Start one operation and return the id its completion settles.
    pub(crate) fn memory_op_begin(
        &mut self,
        action: &'static str,
        target: impl Into<String>,
    ) -> u64 {
        let pane = &mut self.right.memory;
        pane.activity_seq = pane.activity_seq.wrapping_add(1).max(1);
        let id = pane.activity_seq;
        pane.activity.push(MemoryActivity {
            id,
            action,
            target: target.into(),
            state: MemoryActivityState::Pending,
            detail: None,
        });
        while pane.activity.len() > 8 {
            pane.activity.remove(0);
        }
        id
    }

    /// Move one operation to a state, keeping any detail already recorded.
    pub(crate) fn memory_op_set(
        &mut self,
        id: u64,
        state: MemoryActivityState,
        detail: Option<String>,
    ) {
        if let Some(operation) = self
            .right
            .memory
            .activity
            .iter_mut()
            .find(|operation| operation.id == id)
        {
            operation.state = state;
            if detail.is_some() {
                operation.detail = detail;
            }
        }
    }

    /// The workspace binding the pane reads and writes through.
    fn memory_binding(&self) -> crate::memory_scope::MemoryBinding {
        let workspace = self.state.workspace_root();
        crate::memory_scope::binding_for(&self.state.settings, workspace.as_deref())
    }

    /// Run kernel work on the GPUI background executor, driving a private
    /// current-thread tokio runtime INSIDE that future. The test scheduler can
    /// drive the background executor deterministically; a shared tokio runtime
    /// runs on threads the harness refuses to see activity from (the same rule
    /// the comparison pane follows).
    fn memory_kernel_task<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl std::future::Future<Output = Result<T, agent::memory::MemoryError>> + Send + 'static,
    ) -> gpui::Task<Result<T, agent::memory::MemoryError>> {
        cx.background_executor().spawn(async move {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| agent::memory::MemoryError::Unavailable(e.to_string()))?;
            rt.block_on(work)
        })
    }

    /// Load the records once per open; refresh re-runs the same path.
    pub(crate) fn memory_load(&mut self, cx: &mut Context<Self>) {
        if self.right.memory.loading {
            return;
        }
        if self.right.memory.loaded && self.right.memory.store.is_some() {
            return;
        }
        self.right.memory.loading = true;
        let binding = self.memory_binding();
        let task = self.memory_kernel_task(cx, async move {
            let store = FileMemory::open_scoped(
                &binding.workspace_store,
                binding.storage_scope.clone(),
                Some(binding.storage_scope.clone()),
            )
            .await?;
            let records = store.visible_records();
            Ok::<_, agent::memory::MemoryError>((Arc::new(store), records))
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                let pane = &mut this.right.memory;
                pane.loading = false;
                pane.loaded = true;
                match outcome {
                    Ok((store, records)) => {
                        pane.rows = records.iter().map(MemoryRow::from_record).collect();
                        pane.rows.reverse();
                        pane.store = Some(store);
                        pane.load_error = None;
                    }
                    Err(error) => {
                        pane.load_error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-read the records, keeping the search and filters.
    pub(crate) fn memory_refresh(&mut self, cx: &mut Context<Self>) {
        self.right.memory.loaded = false;
        self.right.memory.store = None;
        self.memory_load(cx);
    }

    /// Run the hybrid recall for the search field's current text.
    pub(crate) fn memory_search(&mut self, cx: &mut Context<Self>) {
        let query = self.memory_query.read(cx).value().trim().to_string();
        if query.is_empty() {
            self.right.memory.hits = None;
            cx.notify();
            return;
        }
        let Some(store) = self.right.memory.store.clone() else {
            return;
        };
        let op = self.memory_op_begin("memory.act.search", query.clone());
        let task = self.memory_kernel_task(cx, async move {
            Ok::<_, agent::memory::MemoryError>(
                store.recall(&query, &RecallRequest::default()).await,
            )
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(recall) => {
                        let hits: Vec<MemoryHit> = recall
                            .hits
                            .iter()
                            .map(|hit| MemoryHit {
                                row: MemoryRow::from_record(&hit.record),
                                score: hit.score,
                                reasons: hit
                                    .features
                                    .iter()
                                    .map(|feature| format!("{} {:.2}", feature.name, feature.value))
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                            })
                            .collect();
                        let count = hits.len();
                        this.right.memory.hits = Some(hits);
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Succeeded,
                            Some(format!("{count}")),
                        );
                    }
                    Err(error) => {
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Failed,
                            Some(error.to_string()),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open or close the add form.
    pub(crate) fn memory_toggle_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = !self.right.memory.add_open;
        self.right.memory.add_open = open;
        self.right.memory.edit = None;
        if open {
            if self.right.memory.add_kind.is_empty() {
                self.right.memory.add_kind = "fact".to_string();
            }
            self.memory_add_input
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Choose the kind the add form will commit.
    pub(crate) fn memory_set_add_kind(&mut self, kind: &str) {
        self.right.memory.add_kind = kind.to_string();
    }

    /// Choose the kind filter (None lists every kind).
    pub(crate) fn memory_set_kind_filter(&mut self, kind: Option<String>) {
        self.right.memory.kind_filter = kind;
    }

    /// Toggle the history rows (superseded, expired, tombstoned).
    pub(crate) fn memory_toggle_history(&mut self) {
        self.right.memory.show_history = !self.right.memory.show_history;
    }

    /// Commit the add form's content as a new record.
    pub(crate) fn memory_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.memory_add_input.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        let Some(store) = self.right.memory.store.clone() else {
            return;
        };
        let kind = kind_from_label(&self.right.memory.add_kind);
        let op = self.memory_op_begin(
            "memory.act.add",
            content.chars().take(40).collect::<String>(),
        );
        self.memory_add_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.right.memory.add_open = false;
        let task = self.memory_kernel_task(cx, async move {
            let mut record = MemoryRecord::new(
                kind,
                store.write_scope().clone(),
                content,
                Provenance {
                    source: EvidenceSource::UserStatement,
                    session: None,
                    turn: None,
                    evidence: vec!["ui:memory-pane".to_string()],
                },
            );
            record.confidence = agent::memory::Confidence {
                level: agent::memory::ConfidenceLevel::Medium,
                verified: false,
            };
            store.repository().add_record(record).await
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(committed) => {
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Succeeded,
                            Some(committed.id.clone()),
                        );
                        this.memory_reload_records(cx);
                    }
                    Err(error) => {
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Failed,
                            Some(error.label().to_string()),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Begin editing a record in place.
    pub(crate) fn memory_begin_edit(
        &mut self,
        id: &str,
        supersede: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = self
            .right
            .memory
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.content.clone())
            .unwrap_or_default();
        self.right.memory.edit = Some(MemoryEdit {
            id: id.to_string(),
            supersede,
        });
        self.right.memory.add_open = false;
        self.memory_edit_input
            .update(cx, |input, cx| input.set_value(content, window, cx));
        cx.notify();
    }

    /// Cancel the edit in flight.
    pub(crate) fn memory_cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.right.memory.edit = None;
        cx.notify();
    }

    /// Save the edit: update in place, or commit a successor.
    pub(crate) fn memory_save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.right.memory.edit.clone() else {
            return;
        };
        let content = self.memory_edit_input.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        let Some(store) = self.right.memory.store.clone() else {
            return;
        };
        let action = if edit.supersede {
            "memory.act.supersede"
        } else {
            "memory.act.edit"
        };
        let op = self.memory_op_begin(action, edit.id.clone());
        self.right.memory.edit = None;
        self.memory_edit_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        let task = self.memory_kernel_task(cx, async move {
            if edit.supersede {
                let successor = MemoryRecord::new(
                    MemoryKind::Fact,
                    store.write_scope().clone(),
                    content,
                    Provenance {
                        source: EvidenceSource::UserStatement,
                        session: None,
                        turn: None,
                        evidence: vec!["ui:memory-pane".to_string()],
                    },
                );
                store.repository().supersede(&edit.id, successor).await
            } else {
                store
                    .repository()
                    .update_record(&edit.id, content, None)
                    .await
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(committed) => {
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Succeeded,
                            Some(format!("rev {}", committed.revision)),
                        );
                        this.memory_reload_records(cx);
                    }
                    Err(error) => {
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Failed,
                            Some(error.label().to_string()),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Tombstone one record.
    pub(crate) fn memory_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(store) = self.right.memory.store.clone() else {
            return;
        };
        let op = self.memory_op_begin("memory.act.delete", id.to_string());
        let id = id.to_string();
        let task = self.memory_kernel_task(cx, async move { store.repository().remove(&id).await });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(removed) => {
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Succeeded,
                            Some(if removed { "removed" } else { "not found" }.to_string()),
                        );
                        this.memory_reload_records(cx);
                    }
                    Err(error) => {
                        this.memory_op_set(
                            op,
                            MemoryActivityState::Failed,
                            Some(error.label().to_string()),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-read the records through the cached handle after a mutation.
    fn memory_reload_records(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.right.memory.store.clone() else {
            return;
        };
        let task = self.memory_kernel_task(cx, async move {
            store.refresh().await?;
            Ok::<_, agent::memory::MemoryError>(store.visible_records())
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(records) = outcome {
                    let pane = &mut this.right.memory;
                    pane.rows = records.iter().map(MemoryRow::from_record).collect();
                    pane.rows.reverse();
                    // A search stays live across mutations: re-run it quietly.
                    if pane.hits.is_some() {
                        this.memory_search(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The pane body.
    pub(crate) fn memory_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let pane = &self.right.memory;
        let mut column = div()
            .id("memory-pane-scroll")
            .debug_selector(|| "memory-pane".to_string())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(theme::RIGHT_PAD * 2.0))
            .py(px(12.0))
            .overflow_y_scroll();

        // Header: title, count, refresh and add.
        let count = if pane.show_history {
            pane.rows.len()
        } else {
            pane.rows
                .iter()
                .filter(|row| row.status == "active")
                .count()
        };
        column = column.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(icon(IconName::BookOpen, 16.0, theme::accent()))
                .child(
                    txt(lang.tr("memory.title"), 14.0, theme::text())
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(txt(format!("{count}"), 12.0, theme::faint()))
                .flex_1()
                .child(
                    div()
                        .id("memory-refresh")
                        .debug_selector(|| "memory-refresh".to_string())
                        .cursor_pointer()
                        .p(px(4.0))
                        .child(icon(IconName::RefreshCw, 14.0, theme::faint()))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.memory_refresh(cx);
                        })),
                )
                .child(
                    div()
                        .id("memory-add-toggle")
                        .debug_selector(|| "memory-add-toggle".to_string())
                        .cursor_pointer()
                        .p(px(4.0))
                        .child(icon(IconName::Plus, 14.0, theme::accent()))
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.memory_toggle_add(window, cx);
                        })),
                ),
        );

        // Search field + search action.
        column = column.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(field(&self.memory_query, "memory-search-field"))
                .child(solid_button(
                    "memory-search",
                    lang.tr("memory.search"),
                    false,
                    true,
                    cx,
                    |this, cx| this.memory_search(cx),
                )),
        );

        // Kind filter chips.
        let mut kinds = div().flex().flex_wrap().gap(px(6.0));
        kinds = kinds.child(self.memory_kind_chip(None, lang.tr("memory.kind.all"), cx));
        for kind in KINDS {
            kinds = kinds.child(self.memory_kind_chip(
                Some(kind.to_string()),
                lang.tr(kind_key(kind)),
                cx,
            ));
        }
        column = column.child(kinds);

        // History toggle + capture summary.
        let mut meta = div().flex().items_center().gap(px(8.0));
        meta = meta.child(
            div()
                .id("memory-history-toggle")
                .debug_selector(|| "memory-history-toggle".to_string())
                .cursor_pointer()
                .child(txt(
                    format!(
                        "{} {}",
                        if pane.show_history { "✓" } else { "○" },
                        lang.tr("memory.history")
                    ),
                    12.0,
                    if pane.show_history {
                        theme::accent()
                    } else {
                        theme::faint()
                    },
                ))
                .on_click(cx.listener(|this, _event, _window, _cx| {
                    this.memory_toggle_history();
                })),
        );
        if let Some(capture) = crate::memory_capture::last_capture_summary() {
            let line = match &capture.failure {
                Some(label) => format!(
                    "{}: {} ({})",
                    lang.tr("memory.capture"),
                    lang.tr("memory.failed"),
                    label
                ),
                None => format!(
                    "{}: {} {} · {} {} · {} {}",
                    lang.tr("memory.capture"),
                    capture.committed,
                    lang.tr("memory.committed"),
                    capture.duplicates,
                    lang.tr("memory.duplicates"),
                    capture.contradictions,
                    lang.tr("memory.contradictions"),
                ),
            };
            meta = meta.child(txt(line, 11.0, theme::faint()).flex_1().min_w_0());
        }
        column = column.child(meta);

        // The add form.
        if pane.add_open {
            column = column.child(self.memory_add_form(cx));
        }

        // The list: hits while searching, records otherwise.
        if let Some(error) = &pane.load_error {
            column = column.child(
                div()
                    .debug_selector(|| "memory-load-error".to_string())
                    .child(txt(lang.tr("memory.loadFailed"), 13.0, theme::error()))
                    .child(txt(error.clone(), 11.0, theme::faint())),
            );
        } else if pane.loading && !pane.loaded {
            column = column.child(txt(lang.tr("memory.loading"), 12.0, theme::faint()));
        } else if let Some(hits) = &pane.hits {
            if hits.is_empty() {
                column = column.child(txt(lang.tr("memory.noHits"), 12.0, theme::faint()));
            } else {
                for hit in hits {
                    column = column.child(self.memory_hit_row(hit, cx));
                }
            }
        } else {
            let rows: Vec<&MemoryRow> = pane
                .rows
                .iter()
                .filter(|row| pane.show_history || row.status == "active")
                .filter(|row| {
                    pane.kind_filter
                        .as_ref()
                        .is_none_or(|kind| &row.kind == kind)
                })
                .collect();
            if rows.is_empty() {
                column = column.child(
                    div()
                        .debug_selector(|| "memory-empty".to_string())
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.0))
                        .py(px(24.0))
                        .child(icon(IconName::BookOpen, 22.0, theme::faint()))
                        .child(txt(lang.tr("memory.empty"), 13.0, theme::faint()))
                        .child(
                            txt(lang.tr("memory.emptyHint"), 11.0, theme::faint()).text_center(),
                        ),
                );
            } else {
                for row in rows {
                    column = column.child(self.memory_record_row(row, cx));
                }
            }
        }

        // The activity log.
        if !pane.activity.is_empty() {
            let mut log = div()
                .debug_selector(|| "memory-activity".to_string())
                .flex()
                .flex_col()
                .gap(px(4.0))
                .pt(px(8.0));
            for operation in pane.activity.iter().rev() {
                let (mark, color) = match operation.state {
                    MemoryActivityState::Pending => ("…", theme::faint()),
                    MemoryActivityState::Succeeded => ("✓", theme::success()),
                    MemoryActivityState::Failed => ("✗", theme::error()),
                };
                let mut line = format!(
                    "{} {} {}",
                    mark,
                    lang.tr(operation.action),
                    operation.target
                );
                if let Some(detail) = &operation.detail {
                    line.push_str(&format!(" — {detail}"));
                }
                log = log.child(txt(line, 11.0, color));
            }
            column = column.child(log);
        }

        column.into_any_element()
    }

    /// One kind filter chip.
    fn memory_kind_chip(
        &self,
        kind: Option<String>,
        label: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.right.memory.kind_filter == kind;
        let id = format!(
            "memory-kind-{}",
            kind.clone().unwrap_or_else(|| "all".into())
        );
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .cursor_pointer()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(999.0))
            .bg(rgb(if selected {
                theme::accent()
            } else {
                theme::new_session_fill()
            }))
            .child(txt(
                label,
                11.0,
                if selected { 0xFFFFFF } else { theme::faint() },
            ))
            .on_click(cx.listener(move |this, _event, _window, _cx| {
                this.memory_set_kind_filter(kind.clone());
            }))
            .into_any_element()
    }

    /// The add form: content field, kind chips, commit/cancel.
    fn memory_add_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let mut card = div()
            .debug_selector(|| "memory-add-form".to_string())
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(px(12.0))
            .bg(rgb(theme::new_session_fill()));
        card = card.child(field(&self.memory_add_input, "memory-add-field"));
        let mut kinds = div().flex().flex_wrap().gap(px(6.0));
        for kind in KINDS {
            let selected = self.right.memory.add_kind == kind;
            let id = format!("memory-add-kind-{kind}");
            let selector = id.clone();
            let kind_owned = kind.to_string();
            kinds = kinds.child(
                div()
                    .id(SharedString::from(id))
                    .debug_selector(move || selector.clone())
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(999.0))
                    .bg(rgb(if selected {
                        theme::accent()
                    } else {
                        theme::page()
                    }))
                    .child(txt(
                        lang.tr(kind_key(kind)),
                        11.0,
                        if selected { 0xFFFFFF } else { theme::faint() },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, _cx| {
                        this.memory_set_add_kind(&kind_owned);
                    })),
            );
        }
        card = card.child(kinds);
        card = card.child(
            div()
                .flex()
                .gap(px(8.0))
                .child(window_button(
                    "memory-add-commit",
                    lang.tr("memory.add"),
                    true,
                    cx,
                    |this, window, cx| this.memory_add(window, cx),
                ))
                .child(window_button(
                    "memory-add-cancel",
                    lang.tr("memory.cancel"),
                    false,
                    cx,
                    |this, window, cx| this.memory_toggle_add(window, cx),
                )),
        );
        card.into_any_element()
    }

    /// One record row, with its edit/supersede/delete actions or its editor.
    fn memory_record_row(&self, row: &MemoryRow, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let editing = self
            .right
            .memory
            .edit
            .as_ref()
            .is_some_and(|edit| edit.id == row.id);
        let mut card = div()
            .debug_selector(|| format!("memory-row-{}", row.id))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .rounded(px(12.0))
            .bg(rgb(theme::new_session_fill()));
        if editing {
            let supersede = self
                .right
                .memory
                .edit
                .as_ref()
                .is_some_and(|edit| edit.supersede);
            card = card
                .child(field(&self.memory_edit_input, "memory-edit-field"))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(window_button(
                            "memory-edit-save",
                            lang.tr(if supersede {
                                "memory.supersede"
                            } else {
                                "memory.save"
                            }),
                            true,
                            cx,
                            |this, window, cx| this.memory_save_edit(window, cx),
                        ))
                        .child(solid_button(
                            "memory-edit-cancel",
                            lang.tr("memory.cancel"),
                            false,
                            true,
                            cx,
                            |this, cx| this.memory_cancel_edit(cx),
                        )),
                );
            return card.into_any_element();
        }
        let content: String = row.content.chars().take(240).collect();
        card = card.child(txt(content, 13.0, theme::text()).line_height(px(19.0)));
        let mut chips = div().flex().flex_wrap().items_center().gap(px(6.0));
        chips = chips
            .child(self.memory_chip(lang.tr(kind_key(&row.kind)), theme::accent()))
            .child(self.memory_chip(row.scope.clone(), theme::faint()));
        if row.status != "active" {
            chips = chips.child(self.memory_chip(lang.tr(status_key(&row.status)), theme::warn()));
        }
        chips = chips
            .child(txt(
                format!(
                    "{} {} · {} · {}",
                    lang.tr("memory.rev"),
                    row.revision,
                    row.source,
                    age_label(row.updated_at_ms)
                ),
                11.0,
                theme::faint(),
            ))
            .flex_1()
            .min_w_0();
        let id = row.id.clone();
        let id_edit_sel = id.clone();
        let id_edit = id.clone();
        let id_supersede_sel = id.clone();
        let id_supersede = id.clone();
        let id_delete_sel = id.clone();
        let id_delete = id.clone();
        chips = chips
            .child(
                div()
                    .id(SharedString::from(format!("memory-edit-{id}")))
                    .debug_selector(move || format!("memory-edit-{id_edit_sel}"))
                    .cursor_pointer()
                    .p(px(3.0))
                    .child(icon(IconName::Pencil, 13.0, theme::faint()))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.memory_begin_edit(&id_edit, false, window, cx);
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("memory-supersede-{id}")))
                    .debug_selector(move || format!("memory-supersede-{id_supersede_sel}"))
                    .cursor_pointer()
                    .p(px(3.0))
                    .child(icon(IconName::ArrowUp, 13.0, theme::faint()))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.memory_begin_edit(&id_supersede, true, window, cx);
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("memory-delete-{id}")))
                    .debug_selector(move || format!("memory-delete-{id_delete_sel}"))
                    .cursor_pointer()
                    .p(px(3.0))
                    .child(icon(IconName::Trash, 13.0, theme::error()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.memory_delete(&id_delete, cx);
                    })),
            );
        card = card.child(chips);
        card.into_any_element()
    }

    /// One recall hit row: the record plus its score explanation.
    fn memory_hit_row(&self, hit: &MemoryHit, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let mut card = div()
            .debug_selector(|| format!("memory-hit-{}", hit.row.id))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(12.0))
            .rounded(px(12.0))
            .bg(rgb(theme::new_session_fill()));
        let content: String = hit.row.content.chars().take(240).collect();
        card = card.child(txt(content, 13.0, theme::text()).line_height(px(19.0)));
        card = card.child(txt(
            format!("{:.2} · {}", hit.score, hit.reasons),
            11.0,
            theme::accent(),
        ));
        let id = hit.row.id.clone();
        let id_selector = id.clone();
        let id_delete = id.clone();
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(self.memory_chip(lang.tr(kind_key(&hit.row.kind)), theme::faint()))
                .child(
                    txt(hit.row.scope.clone(), 11.0, theme::faint())
                        .flex_1()
                        .min_w_0(),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("memory-hit-delete-{id}")))
                        .debug_selector(move || format!("memory-hit-delete-{id_selector}"))
                        .cursor_pointer()
                        .p(px(3.0))
                        .child(icon(IconName::Trash, 13.0, theme::error()))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.memory_delete(&id_delete, cx);
                        })),
                ),
        );
        card.into_any_element()
    }

    /// One small label chip.
    fn memory_chip(&self, label: String, color: u32) -> AnyElement {
        div()
            .px(px(7.0))
            .py(px(2.0))
            .rounded(px(999.0))
            .bg(rgb(theme::page()))
            .child(txt(label, 10.0, color))
            .into_any_element()
    }
}
