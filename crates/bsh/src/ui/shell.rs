//! The root view: state ownership, layout, and the streaming pump.

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::*;

use super::settings::SettingsInputs;
use super::DivExt;
use crate::runner::AgentEvent;
use crate::state::{AppState, Overlay};

/// How close to the end still counts as following the tail, in pixels.
const TAIL_EPSILON: f32 = 24.0;

/// The window's only view.
pub(crate) struct Shell {
    /// The model list the last successful fetch returned, if any.
    pub(crate) models_fetched: Option<Vec<String>>,
    /// Whether the last fetch failed, which the pane explains.
    pub(crate) models_fetch_failed: bool,
    /// Whether the last fetch was asked for with no base URL.
    pub(crate) models_fetch_needs_base_url: bool,
    /// The fetched model ids currently ticked in the picker.
    pub(crate) models_selected: std::collections::BTreeSet<String>,
    /// All shell state.
    pub(crate) state: AppState,
    /// The composer's text engine.
    pub(crate) composer: Entity<TextareaState>,
    /// Whether the composer currently carries the hero's placeholder, so the
    /// refresh only runs when the phase actually flips.
    placeholder_hero: bool,
    /// Whether the to-do panel's list is expanded. The reference starts it
    /// collapsed and keeps the state per mounted panel, not per session.
    pub(crate) todo_open: bool,
    /// The queue row's inline editor. The reference edits one queued row at a
    /// time, so a single engine serves the whole list.
    pub(crate) queue_edit: Entity<TextareaState>,
    /// The queued row being edited, named by its identity; None when none is.
    pub(crate) queue_edit_for: Option<u64>,
    /// Whether the queue dock's list is expanded. The reference starts it
    /// collapsed and shows a lone waiting message regardless.
    pub(crate) queue_open: bool,
    /// The session-name search field.
    pub(crate) search: Entity<InputState>,
    /// The plugin-name filter on the plugin list pane.
    pub(crate) plugin_search: Entity<InputState>,
    /// The model-id filter on the fetched-model picker.
    pub(crate) picker_search: Entity<InputState>,
    /// The task-name filter on the Automation tasks pane.
    pub(crate) task_search: Entity<InputState>,
    /// The new-task name field.
    pub(crate) task_name: Entity<InputState>,
    /// The new-task instruction field.
    pub(crate) task_prompt: Entity<InputState>,
    /// The new-task repeat interval in minutes.
    pub(crate) task_every: Entity<InputState>,
    /// The status filter: 0 all, 1 enabled, 2 inactive.
    pub(crate) task_filter: usize,
    /// The task whose delete control is awaiting confirmation.
    pub(crate) task_delete: Option<String>,
    /// The transcript scroll position, so the turn rail can jump within it.
    pub(crate) transcript_scroll: ScrollHandle,
    /// The transcript content height as of the last frame. Following the tail
    /// compares against it, so content that grew this frame still counts as
    /// "the reader was at the bottom".
    tail_max: Option<Pixels>,
    /// The session the tail state belongs to, so switching sessions starts over.
    tail_session: Option<String>,
    /// Whether the newest message should stay in view. A rail jump clears it,
    /// and content growing while the reader is at the end sets it again.
    pub(crate) following_tail: bool,
    /// The reasoning disclosures the reader has opened, by message index. A
    /// session switch closes them all, the way the reference resets a turn's
    /// disclosure state.
    pub(crate) think_open: std::collections::HashSet<usize>,
    /// The rail's scroll position, so the active mark can be kept in view.
    pub(crate) rail_scroll: ScrollHandle,
    /// The turn whose mark the pointer is over, which opens its preview card.
    pub(crate) rail_hover: Option<usize>,
    /// The conversation column's width as of the last frame, less the sidebar.
    /// The rail's container rule and its card breadth are measured against it.
    pub(crate) conversation_w: Pixels,
    /// The chat content width a reader dragged out, or none while the column
    /// follows the reference's clamp. Loaded from settings.content_width.
    pub(crate) chat_user_w: Option<f32>,
    /// The width drag in flight, if any.
    pub(crate) chat_drag: Option<ChatDrag>,
    /// The handle the pointer is over, which is the reference's :hover half
    /// of the bar's visibility rule.
    pub(crate) chat_hover: Option<HandleSide>,
    /// The sidebar width a reader dragged out, before it is committed.
    /// Loaded from settings.sidebar_width.
    pub(crate) sidebar_user_w: Option<f32>,
    /// The sidebar resize in flight, if any.
    pub(crate) sidebar_drag: Option<SideDrag>,
    /// The viewport width as of the last frame, which is what the reference's
    /// sidebar auto-collapse measures.
    pub(crate) viewport_w: f32,
    /// The background job whose stop button is armed (or showing a failure),
    /// with the instant that phase began.
    pub(crate) jobs_arm: Option<super::header::JobArm>,
    /// The composer command menu (the reference's add listbox).
    pub(crate) command_menu: super::command_menu::CommandMenu,
    /// The turn the rail was last scrolled to, so the follow fires once per
    /// change instead of every frame.
    rail_active: Option<usize>,
    /// The composer's current text, mirrored for the send button state.
    pub(crate) composer_value: String,
    /// The agent capabilities, read once when the settings dialog opens.
    pub(crate) caps: Vec<crate::caps::CapInfo>,
    pub(crate) plugins: Vec<crate::plugins::PluginInfo>,
    /// The settings dialog's text fields.
    pub(crate) settings_inputs: SettingsInputs,
    /// Live subscriptions (input events).
    _subs: Vec<Subscription>,
}

/// Which measured width handle a drag is working.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HandleSide {
    /// The handle left of the content column: dragging left widens it.
    Left,
    /// The handle right of the content column: dragging right widens it.
    Right,
}

/// A width drag in flight: the side being worked, the pointer x it started at
/// and the width the column had then - origin and base in the reference.
#[derive(Clone, Copy)]
pub(crate) struct ChatDrag {
    pub(crate) side: HandleSide,
    pub(crate) origin_x: f32,
    pub(crate) base: f32,
    /// Whether the pointer ever moved: the reference commits only a real drag.
    pub(crate) moved: bool,
}

/// A sidebar resize in flight: the reference records the pointer x and the
/// column width at pointerdown and then works from base + dx.
#[derive(Clone, Copy)]
pub(crate) struct SideDrag {
    pub(crate) origin_x: f32,
    pub(crate) base: f32,
    pub(crate) moved: bool,
}

impl Shell {
    /// Build the view, wiring the composer's input events.
    ///
    /// `open_settings` opens the settings dialog as the window appears on the
    /// given rail index, which is what the `--settings` flag asks for.
    pub(crate) fn new(
        window: &mut Window,
        open_settings: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::assemble(window, open_settings, AppState::new(), cx)
    }

    /// Build the view over a supplied state. Tests point the store at a scratch
    /// directory through this, so no session file outside it is touched.
    #[cfg(test)]
    pub(crate) fn build_for_test(
        window: &mut Window,
        open_settings: Option<usize>,
        state: AppState,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::assemble(window, open_settings, state, cx)
    }

    /// The one place the shell is wired up.
    fn assemble(
        window: &mut Window,
        open_settings: Option<usize>,
        mut state: AppState,
        cx: &mut Context<Self>,
    ) -> Self {
        state.restore_session();
        // Measured: the reference reads its stored content width
        // (dsh.conversation.contentWidth) on every width publish, so a window
        // reopens on the column the reader dragged.
        let chat_user_w = state.settings.content_width;
        let sidebar_user_w = state.settings.sidebar_width;
        let viewport_w = window.viewport_size().width.as_f32();
        // The meter's tools bucket follows the tool array a request carries, so
        // it is seeded from the same registry the next turn will send: without
        // this the bucket reads zero until the first turn runs.
        state.set_tools_schema(crate::caps::build_agent(&state.settings).tools_schema_chars);
        // The tree order the sidebar last chose, from the settings file.
        state.sort = crate::store::SessionSort::from_persisted(&state.settings.session_sort);
        let lang = state.lang;
        // Measured: the hero editor draws its own placeholder, so the phase
        // the restore landed in picks the string.
        let hero = state.hero_phase();
        let placeholder = if hero {
            "composer.heroPlaceholder"
        } else {
            "composer.placeholder"
        };
        // Measured: the composer is a soft-wrapping editor that grows with its
        // content behind a 336px ceiling, and Enter submits while Shift+Enter
        // opens a line - which is the kit's own rule once the field is multi
        // line and `submit_on_enter` holds.
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(crate::i18n::tr(lang, placeholder))
                .auto_grow(1, crate::theme::COMPOSER_INPUT_MAX_ROWS)
                .submit_on_enter(true)
        });
        // The queue row's editor shares that shape: the reference's own
        // `.editor` is a textarea capped at 127px.
        let queue_edit = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, crate::theme::QUEUE_EDITOR_MAX_ROWS)
                .submit_on_enter(true)
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "side.search"))
        });
        let plugin_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.searchPlugins"))
        });
        let picker_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "settings.fetchSearch"))
        });
        let task_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::tr(lang, "task.searchPlaceholder"))
        });
        let task_name = cx.new(|cx| InputState::new(window, cx));
        let task_prompt = cx.new(|cx| InputState::new(window, cx));
        let task_every = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this: &mut Shell,
                 _state: &Entity<TextareaState>,
                 event: &InputEvent,
                 window: &mut Window,
                 cx: &mut Context<Shell>| match event {
                    InputEvent::Change => {
                        this.composer_value = this.composer.read(cx).value().to_string();
                        // The card follows the draft: a `/` token shows the command
                        // source, an `@` token the reference source, and neither
                        // closes it.
                        this.command_menu.sync(this.composer_value.as_str());
                        cx.notify();
                    }
                    InputEvent::PressEnter { shift: false, .. } => {
                        // The reference gives Enter to the open menu first: the
                        // highlighted row runs, and the composer keeps its draft.
                        if this.command_menu.open {
                            this.run_row(this.command_menu.active, window, cx);
                        } else {
                            this.send(window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &queue_edit,
                window,
                |this: &mut Shell,
                 _state: &Entity<TextareaState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { shift: false, .. } => this.save_queue_edit(cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.state.search_query = this.search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &plugin_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.state.plugin_query = this.plugin_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &picker_search,
                window,
                |this: &mut Shell,
                 _state: &Entity<InputState>,
                 event: &InputEvent,
                 _window: &mut Window,
                 cx: &mut Context<Shell>| {
                    if matches!(event, InputEvent::Change) {
                        this.state.picker_query = this.picker_search.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
        ];
        let settings_inputs = SettingsInputs::new(window, cx);
        let mut shell = Self {
            state,
            composer,
            placeholder_hero: hero,
            todo_open: false,
            queue_edit,
            queue_edit_for: None,
            queue_open: false,
            search,
            plugin_search,
            picker_search,
            task_search,
            task_name,
            task_prompt,
            task_every,
            task_filter: 0,
            task_delete: None,
            composer_value: String::new(),
            caps: Vec::new(),
            plugins: Vec::new(),
            settings_inputs,
            transcript_scroll: ScrollHandle::new(),
            tail_max: None,
            tail_session: None,
            following_tail: true,
            think_open: std::collections::HashSet::new(),
            rail_scroll: ScrollHandle::new(),
            rail_hover: Some(58),
            rail_active: None,
            conversation_w: px(0.0),
            chat_user_w,
            sidebar_user_w,
            viewport_w,
            jobs_arm: None,
            command_menu: super::command_menu::CommandMenu::default(),
            chat_drag: None,
            chat_hover: None,
            sidebar_drag: None,
            models_fetched: None,
            models_fetch_failed: false,
            models_fetch_needs_base_url: false,
            models_selected: std::collections::BTreeSet::new(),
            _subs: subs,
        };
        crate::theme::apply(&shell.state.settings.theme_mode, window, cx);
        // Follow a live host appearance switch, which is what makes the System
        // option mean anything: the copied palette tokens would otherwise go
        // stale the moment macOS changed.
        shell
            ._subs
            .push(cx.observe_window_appearance(window, |this, window, cx| {
                this.appearance_changed(window, cx);
            }));
        // The jobs clock: the reference re-renders its elapsed figures once a
        // second while the panel is open over a live job, and bsh needs the same
        // cadence to expire an armed stop inside its measured window.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            if this
                .update(cx, |this, cx| {
                    this.tick_jobs(std::time::Instant::now(), cx);
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        // The scheduler: one check every 30 seconds, so a task comes due while the
        // window is open without needing a turn of its own to hang off.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            if this
                .update(cx, |this, cx| {
                    if !this.state.settings.schedules.is_empty() {
                        this.check_schedules(cx);
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        if let Some(section) = open_settings {
            shell.open_settings_at(section, window, cx);
        }
        shell
    }

    /// React to the host appearance changing.
    ///
    /// Only the System mode follows it; a pinned Light or Dark keeps the palette
    /// the user chose, so this is the gate the option promises.
    /// Fetch the endpoint model list through the react vendor seam.
    ///
    /// The reference gates this on a base URL, so an empty one is a hint and
    /// not a request. The HTTP call runs on the shell tokio runtime because
    /// the gpui executor has no reactor, and only the answer comes back to the
    /// view; a refusal is reported rather than painted as an empty list.
    pub(crate) fn fetch_models(&mut self, cx: &mut Context<Self>) {
        let endpoint = self.state.draft().base_url.trim().to_string();
        if endpoint.is_empty() {
            self.models_fetch_needs_base_url = true;
            cx.notify();
            return;
        }
        let draft = self.state.draft();
        let model = draft.model.trim().to_string();
        let api_key = draft.api_key.clone();
        let (tx, rx) = async_channel::bounded(1);
        self.state.runner.rt().spawn(async move {
            let vendor = react::llm::vendor::OpenAiVendor::new(endpoint, model, api_key);
            let _ = tx.send(vendor.fetch_model_ids().await).await;
        });
        self.models_fetch_failed = false;
        self.models_fetch_needs_base_url = false;
        cx.spawn(async move |this, cx| {
            let outcome = rx.recv().await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(Ok(models)) => {
                        this.models_fetched = Some(models);
                        this.models_selected.clear();
                    }
                    _ => {
                        this.models_fetched = None;
                        this.models_fetch_failed = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Tick or untick one fetched model in the picker.
    pub(crate) fn toggle_fetched_model(&mut self, model: &str) {
        if !self.models_selected.remove(model) {
            self.models_selected.insert(model.to_string());
        }
    }

    /// Add every ticked fetched model to the active provider catalog.
    ///
    /// The catalog itself refuses blanks and repeats, so adding twice is
    /// harmless; the picker closes because the reference dialog does.
    pub(crate) fn add_selected_models(&mut self) {
        let provider = self.state.draft().active_provider.clone();
        let ids: Vec<String> = self.models_selected.iter().cloned().collect();
        let draft = self.state.draft_mut();
        for id in ids {
            let _ = draft.add_catalog_model(&provider, &id);
        }
        self.models_selected.clear();
        self.models_fetched = None;
    }

    pub(crate) fn appearance_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.settings.theme_mode == "system" {
            crate::theme::apply("system", window, cx);
            cx.notify();
        }
    }

    /// Drop one pending attachment.
    pub(crate) fn remove_attachment(&mut self, index: usize) {
        if index < self.state.attachments.len() {
            self.state.attachments.remove(index);
        }
    }

    /// Ask for files and mention them in the composer.
    pub(crate) fn attach_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let accepts = self.state.settings.active_accepts_image();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach to the message".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let mut picked: Vec<crate::state::Attachment> = Vec::new();
            let mut mention = String::new();
            let mut refused = false;
            for path in &paths {
                match crate::state::Attachment::from_path(path) {
                    Some(_) if !accepts => refused = true,
                    Some(attachment) => picked.push(attachment),
                    None => mention.push_str(&format!(" @{}", path.display())),
                }
            }
            this.update_in(cx, |this, window, cx| {
                if refused {
                    this.state.notice(
                        crate::i18n::tr(this.state.lang, "composer.imageRefused").to_string(),
                    );
                }
                this.state.attachments.extend(picked);
                if !mention.is_empty() {
                    let current = this.composer.read(cx).value().to_string();
                    let next = format!("{current}{mention}");
                    this.composer
                        .update(cx, |state, cx| state.set_value(next.clone(), window, cx));
                    this.composer_value = next;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Ask for a folder and add it as a workspace.
    ///
    /// A folder that is already a workspace is selected instead of added
    /// twice, so the button is safe to press on a root the user already has.
    pub(crate) fn add_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt: SharedString = self.state.lang.tr("side.addWorkspace").into();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.first() else {
                return;
            };
            let root = path.to_string_lossy().to_string();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| root.clone());
            this.update_in(cx, |this, _window, cx| {
                let existing = this
                    .state
                    .settings
                    .workspaces
                    .iter()
                    .find(|w| w.root == root)
                    .map(|w| w.id.clone());
                let id = match existing {
                    Some(id) => id,
                    None => {
                        let id = uuid::Uuid::new_v4().to_string();
                        this.state
                            .settings
                            .workspaces
                            .push(crate::settings::Workspace {
                                id: id.clone(),
                                name,
                                root,
                                created_at: crate::state::now(),
                                updated_at: crate::state::now(),
                                ..Default::default()
                            });
                        id
                    }
                };
                this.state.settings.active_workspace = id;
                let _ = this.state.settings.save();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the settings dialog on a given rail section.
    pub(crate) fn open_settings_at(
        &mut self,
        section: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(window, cx);
        self.state.settings_section = section;
    }

    /// Open the settings dialog, loading the fields from the saved settings.
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.open_settings();
        let draft = self.state.draft().clone();
        self.settings_inputs.sync(&draft, window, cx);
        cx.notify();
    }

    /// Close the settings dialog, discarding everything it edited.
    pub(crate) fn cancel_settings(&mut self, cx: &mut Context<Self>) {
        self.state.cancel_settings();
        cx.notify();
    }

    /// Keep the dialog's settings, persist them, and rebuild the capability list.
    pub(crate) fn save_settings(&mut self, cx: &mut Context<Self>) {
        let mut draft = self.state.draft.clone().unwrap_or_default();
        self.settings_inputs.read(&mut draft, cx);
        self.state.draft = Some(draft);
        match self.state.commit_settings() {
            Ok(()) => {
                self.caps.clear();
                self.plugins.clear();
                self.state
                    .notice(crate::i18n::tr(self.state.lang, "settings.saved").to_string());
            }
            Err(error) => self.state.notice(format!(
                "{} {error}",
                crate::i18n::tr(self.state.lang, "settings.saveFailed")
            )),
        }
        cx.notify();
    }

    /// Send the composer's text, or queue it while the session is running.
    pub(crate) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let attachments = std::mem::take(&mut self.state.attachments);
        // Measured: the reference queues a submission while its session is
        // already running instead of opening a second turn over the same
        // transcript, which is what its queue dock lists.
        if let Some(id) = self.state.active_id.clone() {
            if self.state.is_streaming(&id) {
                self.state.enqueue(&id, text, attachments);
                self.clear_composer(window, cx);
                return;
            }
        }
        let input = crate::state::turn_input(&text, &attachments);
        let display = crate::state::display_text(&text, &attachments);
        let Some((id, transcript, budget)) = self.state.begin_turn(&display) else {
            self.state.attachments = attachments;
            return;
        };
        self.spawn_turn(id, input, transcript, budget, cx);
        self.clear_composer(window, cx);
    }

    /// Empty the composer after a submission left it.
    fn clear_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.composer_value.clear();
        cx.notify();
    }

    /// Spawn a turn whose transcript and budget the state has already opened.
    ///
    /// The composer, the scheduler and the queue all reach a turn this way, so
    /// the agent they run is built and accounted for identically.
    fn spawn_turn(
        &mut self,
        id: String,
        input: react::llm::Content,
        transcript: Vec<crate::model::ChatMessage>,
        budget: usize,
        cx: &mut Context<Self>,
    ) {
        let built = crate::caps::build_agent(&self.state.settings);
        self.state.set_tools_schema(built.tools_schema_chars);
        let agent = built.agent;
        // The agent shares this store with its `update_plan` tool, so the
        // to-do panel reads the running plan through it.
        self.state.attach_plan(&id, agent.plan());
        let cap = self.state.settings.effective().tool_output_cap;
        let rx = self
            .state
            .runner
            .spawn(&id, agent, input, transcript, budget, cap);
        self.pump(id, rx, cx);
    }

    /// Save the queue row being edited, which is Enter in its own editor.
    fn save_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = self.queue_edit_for else {
            return;
        };
        let text = self.queue_edit.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(id) = self.state.active_id.clone() {
            self.state.edit_queued(&id, seq, text);
        }
        self.queue_edit_for = None;
        cx.notify();
    }

    /// Start the next queued message once a session's turn has ended.
    ///
    /// The reference drains its inbox as each turn finishes, so a reader who
    /// submits while the agent runs gets a second turn rather than a second
    /// stream writing over the same transcript.
    pub(crate) fn drain_inbox(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.state.is_streaming(id) {
            return;
        }
        let Some(queued) = self.state.pop_inbox(id) else {
            return;
        };
        let input = crate::state::turn_input(&queued.text, &queued.attachments);
        let display = crate::state::display_text(&queued.text, &queued.attachments);
        let Some((transcript, budget)) = self.state.begin_turn_in(id, &display) else {
            return;
        };
        self.spawn_turn(id.to_string(), input, transcript, budget, cx);
    }

    /// Start every automation task that has come due.
    ///
    /// The reference runs a task in the session it was created in, so its events
    /// fold into that session and the reader sees the turn when they open it. The
    /// tasks come from the SAVED settings, which is what the pane own Save
    /// writes; a session that is gone switches its task off rather than firing
    /// into nothing, and a task whose session is already streaming is skipped for
    /// this tick.
    pub(crate) fn check_schedules(&mut self, cx: &mut Context<Self>) {
        let now = crate::settings::unix_now();
        let due = self.state.settings.due_schedules(now);
        for id in due {
            let Some(task) = self
                .state
                .settings
                .schedules
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
            else {
                continue;
            };
            if self.state.streams.contains_key(&task.session_id) {
                continue;
            }
            let Some((transcript, budget)) =
                self.state.begin_turn_in(&task.session_id, &task.prompt)
            else {
                self.state.settings.toggle_schedule(&id);
                continue;
            };
            let input = crate::state::turn_input(&task.prompt, &[]);
            self.spawn_turn(task.session_id.clone(), input, transcript, budget, cx);
            self.state.settings.mark_schedule_ran(&id, now);
        }
        cx.notify();
    }

    /// Forward runner events into the state until the turn ends.
    fn pump(&self, id: String, rx: async_channel::Receiver<AgentEvent>, cx: &mut Context<Self>) {
        // Keep the elapsed-time line moving while the turn streams.
        cx.spawn(async move |this, cx| loop {
            let live = this
                .update(cx, |this, cx| {
                    if this.state.any_streaming() {
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !live {
                break;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
        })
        .detach();

        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let terminal = matches!(&event, AgentEvent::Done | AgentEvent::Error(_));
                let session_id = id.clone();
                let _ = this.update(cx, |shell, cx| {
                    shell.state.apply_event(&session_id, event);
                    cx.notify();
                });
                if terminal {
                    // The turn is over, so its plan is the session's own now and
                    // whatever waited behind it can start.
                    let _ = this.update(cx, |shell, cx| {
                        shell.state.finish_plan(&session_id);
                        shell.drain_inbox(&session_id, cx);
                    });
                    break;
                }
            }
            // A stream that ends without a terminal event (a stop) still has to
            // release its store, or the saved plan would never catch up, and the
            // queue behind it still belongs to the reader.
            let _ = this.update(cx, |shell, cx| {
                shell.state.finish_plan(&id);
                shell.drain_inbox(&id, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The reference's chat content width for the column measured on this
    /// frame, `clamp(680px, conversation-column * .64, 920px)` over a
    /// conversation column of the viewport less the sidebar. See
    /// `crate::theme::chat_width_at`.
    pub(crate) fn chat_w(&self) -> f32 {
        crate::theme::resolve_content_width(self.conversation_w.as_f32(), self.chat_user_w)
    }

    /// The measured width-handle strip for the live column.
    pub(crate) fn handle_w(&self) -> f32 {
        crate::theme::handle_width_at(self.conversation_w.as_f32(), self.chat_w())
    }

    /// A pointer landed on a width handle: remember the side, the x it started
    /// at and the width the column had then - origin and base in the reference.
    pub(crate) fn begin_chat_drag(
        &mut self,
        side: HandleSide,
        pointer_x: f32,
        cx: &mut Context<Self>,
    ) {
        self.chat_drag = Some(ChatDrag {
            side,
            origin_x: pointer_x,
            base: self.chat_w(),
            moved: false,
        });
        cx.notify();
    }

    /// The pointer moved with a drag in flight. The reference's outwardWidth is
    /// base + outward * 2, where outward is the dx signed toward the dragged
    /// side, because the content column stays centred while one edge moves.
    pub(crate) fn drag_chat_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.chat_drag else { return };
        let dx = pointer_x - drag.origin_x;
        let outward = match drag.side {
            HandleSide::Right => dx,
            HandleSide::Left => -dx,
        };
        if dx != 0.0 {
            self.chat_drag = Some(ChatDrag {
                moved: true,
                ..drag
            });
        }
        let width = drag.base + outward * 2.0;
        self.chat_user_w = Some(crate::theme::resolve_content_width(
            self.conversation_w.as_f32(),
            Some(width),
        ));
        cx.notify();
    }

    /// The pointer lifted. The reference commits only when the pointer actually
    /// moved, and stores the RESOLVED width, under the measured client-store key
    /// dsh.conversation.contentWidth; bsh's store is the settings file.
    pub(crate) fn commit_chat_width(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.chat_drag.take() else {
            return;
        };
        if drag.moved {
            self.state.settings.content_width = self.chat_user_w;
            let _ = self.state.settings.save();
        }
        cx.notify();
    }

    /// The pointer entered a width handle: the reference reveals the bar on
    /// :hover. The strip stops the move's propagation, so the body's own move
    /// (which clears this) is what makes leaving the strip hide it again.
    pub(crate) fn hover_chat_handle(&mut self, side: HandleSide, cx: &mut Context<Self>) {
        if self.chat_hover != Some(side) {
            self.chat_hover = Some(side);
            cx.notify();
        }
    }

    /// The pointer moved somewhere the handles are not, ending the reference's
    /// :hover. A drag keeps the bar up on its own, through chat_drag.
    pub(crate) fn clear_chat_hover(&mut self, cx: &mut Context<Self>) {
        if self.chat_hover.take().is_some() {
            cx.notify();
        }
    }

    /// The reference's resolved sidebar column: the dragged width, or its own
    /// 280 store default, inside the measured 264..420 contract.
    pub(crate) fn sidebar_w(&self) -> f32 {
        crate::theme::resolve_sidebar_width(self.sidebar_user_w)
    }

    /// The reference's `pressKill`: the first press arms the button, the
    /// confirming press stops the job. bsh's stop is local (the runner's stop
    /// plus the stream removal), so the only failure it can observe is a job that
    /// is already gone - and the reference's failed phase is the same shape, a
    /// hint held for its measured window.
    pub(crate) fn press_job_kill_at(
        &mut self,
        id: &str,
        at: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        let armed = self.jobs_arm.as_ref().is_some_and(|arm| {
            arm.id == id && arm.phase == super::header::JobPhase::Armed && !arm.expired(at)
        });
        if armed {
            self.jobs_arm = None;
            let live = self.state.is_streaming(id);
            self.state.stop(id);
            if !live {
                self.jobs_arm = Some(super::header::JobArm {
                    id: id.to_string(),
                    phase: super::header::JobPhase::Failed,
                    at,
                });
            }
        } else {
            self.jobs_arm = Some(super::header::JobArm {
                id: id.to_string(),
                phase: super::header::JobPhase::Armed,
                at,
            });
        }
        cx.notify();
    }

    /// Drop an arming or failure hint that has outlived its measured window,
    /// reporting whether one went.
    pub(crate) fn expire_job_arm(&mut self, at: std::time::Instant) -> bool {
        if self.jobs_arm.as_ref().is_some_and(|arm| arm.expired(at)) {
            self.jobs_arm = None;
            return true;
        }
        false
    }

    /// One tick of the jobs panel's own clock: it re-renders the elapsed figures
    /// and drops an expired kill hint. The reference runs this once a second
    /// while the popover is open over a live row; the kit popover carries no
    /// open-change seam here, so bsh ticks while a background job exists at all -
    /// which is the only state in which the trigger, and so the popover, can be
    /// drawn.
    pub(crate) fn tick_jobs(&mut self, at: std::time::Instant, cx: &mut Context<Self>) {
        let live = !self.state.background_jobs().is_empty();
        let expired = self.expire_job_arm(at);
        if live || expired {
            cx.notify();
        }
    }

    /// The reference's own fold resolution: below the 1024 breakpoint the
    /// column folds unless the reader expanded it over the squeezed centre.
    pub(crate) fn sidebar_folded(&self) -> bool {
        crate::theme::sidebar_folded(
            self.viewport_w,
            self.state.sidebar_collapsed,
            self.state.narrow_expanded,
        )
    }

    /// The reference's own toggleSidebar: a narrow viewport flips the runtime
    /// narrowExpanded flag so the column can expand over the centre, and only a
    /// wide one flips the stored collapse.
    pub(crate) fn toggle_sidebar_at(&mut self, viewport: f32, cx: &mut Context<Self>) {
        if viewport < crate::theme::SIDEBAR_AUTO_COLLAPSE {
            self.state.narrow_expanded = !self.state.narrow_expanded;
        } else {
            self.state.sidebar_collapsed = !self.state.sidebar_collapsed;
        }
        cx.notify();
    }

    /// A mousedown on the sidebar's own handle: the reference records the
    /// column width and the pointer x and then works from that pair.
    pub(crate) fn begin_sidebar_drag(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        self.sidebar_drag = Some(SideDrag {
            origin_x: pointer_x,
            base: self.sidebar_w(),
            moved: false,
        });
        cx.notify();
    }

    /// The reference's own onSidebarDrag: setSidebar(base + dx), clamped by
    /// clampWidth, applied live and stored once the pointer is released.
    pub(crate) fn drag_sidebar_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.sidebar_drag.as_mut() else {
            return;
        };
        drag.moved = true;
        let width =
            crate::theme::resolve_sidebar_width(Some(drag.base + (pointer_x - drag.origin_x)));
        if self.sidebar_user_w != Some(width) {
            self.sidebar_user_w = Some(width);
            cx.notify();
        }
    }

    /// A pointerup ends the resize with the reference's final dx applied, and
    /// only a real move is worth a store write.
    pub(crate) fn commit_sidebar_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.sidebar_drag.take() else {
            return;
        };
        let width =
            crate::theme::resolve_sidebar_width(Some(drag.base + (pointer_x - drag.origin_x)));
        if drag.moved {
            self.sidebar_user_w = Some(width);
            self.state.settings.sidebar_width = self.sidebar_user_w;
            let _ = self.state.settings.save();
        }
        cx.notify();
    }

    /// Stop the active session's stream.
    pub(crate) fn stop_active(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.state.active_id.clone() {
            self.state.stop(&id);
            cx.notify();
        }
    }

    /// Select a session and focus the composer.
    pub(crate) fn open_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.state.select(id);
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Create a chat and focus the composer.
    pub(crate) fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.new_session();
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// The message index of every user turn, in order.
    pub(crate) fn turn_indices(&self) -> Vec<usize> {
        self.state
            .active_session()
            .map(|session| {
                session
                    .messages
                    .iter()
                    .enumerate()
                    .filter(|(_, message)| message.role == crate::model::Role::User)
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keep the active mark in the rail's view unless the pointer is on it.
    ///
    /// `TurnNavigatorRail` scrolls to the active index only when it changes and
    /// skips the follow while the pointer is inside the frame, so a reader
    /// choosing a mark is never fighting the scroll.
    pub(crate) fn follow_rail(&mut self) {
        let turns = self.turn_indices();
        let active = self.visible_turn(&turns);
        if active == self.rail_active {
            return;
        }
        self.rail_active = active;
        if self.rail_hover.is_none() {
            // The handle counts the scroller's children, i.e. marks.
            if let Some(index) = active.and_then(|turn| turns.iter().position(|item| *item == turn))
            {
                self.rail_scroll.scroll_to_item(index);
            }
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The hero editor carries its own placeholder, so re-point the composer
        // whenever the phase flips - a session switch, a new chat, the first
        // turn. Updating it only on the flip keeps the notify out of the frame
        // loop it would otherwise join.
        let hero = self.state.hero_phase();
        if hero != self.placeholder_hero {
            self.placeholder_hero = hero;
            let placeholder = if hero {
                "composer.heroPlaceholder"
            } else {
                "composer.placeholder"
            };
            let text = self.state.lang.tr(placeholder);
            self.composer
                .update(cx, |input, cx| input.set_placeholder(text, window, cx));
        }
        // The rail's container is the conversation column, which is the window
        // less the sidebar (`@container (width <= 900px)` hides the frame).
        // The frame's width is what the reference's auto-collapse measures, so
        // record it before the fold is resolved.
        self.viewport_w = window.viewport_size().width.as_f32();
        let sidebar = if self.sidebar_folded() {
            px(0.0)
        } else {
            px(self.sidebar_w())
        };
        self.conversation_w = window.viewport_size().width - sidebar;
        // The rail follows the active mark, unless the pointer is working it.
        self.follow_rail();
        // Follow the tail, the state the reference marks as data-chat-following-tail:
        // the newest message stays in view until the reader leaves the end. A rail
        // jump clears the flag; content growing while the reader is still at the end
        // (or a fresh session) sets it again, so a reader who scrolled up is left alone.
        let active = self.state.active_id.clone();
        if active != self.tail_session {
            self.tail_session = active;
            self.tail_max = None;
            self.following_tail = true;
            self.think_open.clear();
            self.rail_hover = None;
            self.rail_active = None;
            self.state.stat_open = false;
            self.state.meter_open = false;
        }
        let scroll = &self.transcript_scroll;
        let offset = scroll.offset().y;
        let max = scroll.max_offset().y;
        let was_at_end = self
            .tail_max
            .is_none_or(|previous| previous + offset <= px(TAIL_EPSILON));
        if was_at_end && self.tail_max.is_some_and(|previous| max > previous) {
            self.following_tail = true;
        }
        if self.following_tail {
            scroll.scroll_to_bottom();
            if max <= px(0.0) {
                // Nothing has been laid out yet, so the content height is still
                // unknown: take one more frame and scroll once it is measured.
                cx.notify();
            }
        }
        self.tail_max = Some(max);
        let overlay = if self.state.overlay == Overlay::Settings {
            // The panel is sized against the viewport, like the reference's
            // own min(800px, 100vh - 48px) height.
            let viewport = window.viewport_size();
            Some(self.render_settings(viewport.width.as_f32(), viewport.height.as_f32(), cx))
        } else {
            None
        };
        // The delete confirmation sits over the settings dialog: the reference
        // draws a dialog there rather than deleting on the row control.
        let confirm_delete =
            if self.state.overlay == Overlay::Settings && self.task_delete.is_some() {
                Some(self.render_task_delete(cx))
            } else {
                None
            };
        div()
            .id("shell-frame")
            .flex()
            .flex_row()
            .size_full()
            .overflow_hidden()
            .relative()
            // The reference's DragHandle captures its pointer, so its own
            // onPointerUp ends the drag; gpui hands the release back to the
            // pressed strip but the MOVES to whatever is under the pointer,
            // so the frame carries those. It is a no-op without a drag.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.drag_sidebar_width(event.position.x.as_f32(), cx);
            }))
            .bg(rgb(crate::theme::page()))
            .text_color(rgb(crate::theme::text()))
            .when(!self.sidebar_folded(), |root| {
                root.child(self.render_sidebar(cx))
            })
            .child(self.render_main_column(cx))
            // The reference draws its DragHandle over the seam at z-index 11,
            // and only while the column is expanded.
            .when(!self.sidebar_folded(), |root| {
                root.child(self.render_sidebar_handle(cx))
            })
            .when_some(overlay, |root, overlay| root.child(overlay))
            .when_some(confirm_delete, |root, confirm| root.child(confirm))
    }
}
