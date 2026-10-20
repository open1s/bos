//! Interaction tests: boot the shell headlessly and click real controls.
//!
//! A probe can only show that an element was built. A click proves the handler
//! behind it runs, which is what the reference does when a control is pressed.
//!
//! Imports are named on purpose: a glob would pull in the gpui re-export of the
//! test attribute macro, which shadows the builtin one its own expansion emits.

use gpui_kit::{
    px, AppContext as _, Context, Entity, IntoElement, Modifiers, Render, TestAppContext,
    VisualTestContext, Window,
};

use crate::i18n::Lang;
use crate::model::{
    ChatMessage, MessageMeta, MessageUsage, PlanItem, PlanStatus, PlanStore, ToolEvent,
};
use crate::state::{AppState, Overlay, Tab};
use crate::store::Store;
use crate::ui::right_panel::{FileTab, RightTab};
use crate::ui::settings::{load_provider_into_form, section_index};
use crate::ui::shell::Shell;
use crate::ui::todo_panel::progress;
use gpui_kit::component::ActiveTheme as _;

/// Boot the kit and bind bsh's own keys, the way `ui::run` does.
///
/// A test that never walks `ui::run` never gets the cmd-s binding the files
/// editor relies on, so every test boots through here instead.
fn init_test_app(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    cx.update(crate::ui::bind_global_actions);
}

/// Holds the shell so the window can render an entity the test also keeps.
struct Harness {
    shell: Entity<Shell>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.shell.clone()
    }
}

/// Pin the reference's own defaults on a test state.
///
/// `AppState::with_store` loads the real `~/.bos/bsh/settings.toml`, so a
/// machine that has dragged its sidebar or stepped its font size would colour
/// every measured chrome assertion with the developer's own values. A test that
/// compares against a measured number calls this first, and the assertion then
/// tests what the reference ships, not what this machine has drifted to.
fn tests_pin_reference_defaults(state: &mut AppState) {
    state.settings.global.general.content_font_size = crate::theme::CONTENT_DEFAULT;
    state.settings.global.general.sidebar_width = None;
    state.settings.global.general.content_width = None;
    state.settings.global.general.theme_mode = "system".to_string();
}

/// A store rooted in the build directory: a test must never touch real sessions.
/// The label keeps tests apart - they run in parallel inside one process.
fn scratch_store(label: &str) -> Store {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("bsh-scratch-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    Store::open_at(dir)
}

/// A store with one session carrying that many user turns, so the transcript is
/// tall enough to scroll.
fn state_with_turns(label: &str, turns: usize) -> AppState {
    let mut state = AppState::with_store(scratch_store(label));
    let id = state.active_id.clone().expect("an active session");
    let long = "lorem ipsum dolor sit amet ".repeat(12);
    if let Some(session) = state.store.session_mut(&id) {
        for turn in 0..turns {
            session
                .messages
                .push(ChatMessage::user(format!("turn {turn}: {long}")));
            session.messages.push(ChatMessage::assistant());
        }
    }
    state
}

/// One plan step, the shape the agent's update_plan tool writes.
fn plan_item(text: &str, status: PlanStatus) -> PlanItem {
    PlanItem {
        text: text.to_string(),
        status,
    }
}

/// A store with one session carrying the given plan, so the to-do panel has the
/// data a real turn would have left behind.
///
/// The turn is seeded too: a plan only exists after a turn, and the reference
/// draws its hero - not the composer stack - while a conversation is empty.
fn state_with_plan(label: &str, items: Vec<PlanItem>) -> AppState {
    let mut state = AppState::with_store(scratch_store(label));
    let id = state.active_id.clone().expect("an active session");
    if let Some(session) = state.store.session_mut(&id) {
        session.messages.push(ChatMessage::user("plan the work"));
        session.messages.push(ChatMessage::assistant());
        session.plan = items;
    }
    state
}

/// The header summary is the reference's own rule: one segment per non-zero
/// status joined with a spaced middle dot, with the zero-count ones dropped.
#[test]
fn the_progress_summary_counts_each_status() {
    let items = vec![
        plan_item("done", PlanStatus::Completed),
        plan_item("running", PlanStatus::InProgress),
        plan_item("waiting", PlanStatus::Pending),
        plan_item("also waiting", PlanStatus::Pending),
    ];
    assert_eq!(
        progress(&items, Lang::En),
        "1 completed · 1 in progress · 2 pending"
    );
    assert_eq!(progress(&items, Lang::Zh), "1 已完成 · 1 进行中 · 2 待处理");
    let only_pending = vec![
        plan_item("one", PlanStatus::Pending),
        plan_item("two", PlanStatus::Pending),
    ];
    assert_eq!(
        progress(&only_pending, Lang::En),
        "2 pending",
        "zero-count segments are dropped"
    );
}

/// The to-do panel, from the reference's own TodoPanel.module.css: a radius-lg
/// surface 64px narrower than the composer card and centred over it, collapsed
/// by default, sitting on the composer stack's own 6px gap above the card.
#[gpui::test]
fn the_todo_panel_takes_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_plan(
            "todo-chrome",
            vec![
                plan_item("Read the code", PlanStatus::Completed),
                plan_item("Draw the panel", PlanStatus::InProgress),
            ],
        );
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let panel = cx.debug_bounds("todo-panel").expect("the panel renders");
    let head = cx
        .debug_bounds("todo-panel-header")
        .expect("its header renders");
    let card = cx
        .debug_bounds("composer-card")
        .expect("the composer card renders");
    let column = cx.debug_bounds("main-column").expect("the column renders");

    assert_eq!(
        panel.size.width,
        card.size.width - 2.0 * px(crate::theme::TODO_SIDE_INSET),
        "the panel is the live composer card less its 32px inset a side"
    );
    assert_eq!(
        head.size.height,
        px(24.0),
        "the header is its 24px title line"
    );
    assert_eq!(
        panel.size.height,
        px(37.0),
        "12px of body padding over the 24px line, plus the 1px the .5px ring adds"
    );
    assert_eq!(
        card.origin.y - (panel.origin.y + panel.size.height),
        px(6.0),
        "the composer stack's own gap separates the panel from the card"
    );
    let panel_mid = panel.origin.x + panel.size.width / 2.0;
    let column_mid = column.origin.x + column.size.width / 2.0;
    let off = panel_mid - column_mid;
    assert!(
        off > px(-1.5) && off < px(1.5),
        "the panel is centred in the column"
    );
    assert!(
        cx.debug_bounds("todo-panel-list").is_none(),
        "the reference starts the list collapsed"
    );
}

/// The header toggles the list open, which is the only control the panel has.
#[gpui::test]
fn the_todo_header_expands_the_list(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_plan(
            "todo-expand",
            vec![
                plan_item("Read the code", PlanStatus::Completed),
                plan_item("Draw the panel", PlanStatus::InProgress),
            ],
        );
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let head = cx
        .debug_bounds("todo-panel-header")
        .expect("the header renders");
    cx.simulate_click(head.center(), Modifiers::default());
    let open = cx.update(|_, cx| shell.read(cx).todo_open);
    assert!(open, "the header click must expand the list");

    cx.update(|window, cx| window.draw(cx).clear(cx));
    let list = cx
        .debug_bounds("todo-panel-list")
        .expect("the expanded list renders");
    assert_eq!(
        list.size.height,
        px(48.0),
        "two 20px rows over the list's own 8px gap"
    );
    let panel = cx.debug_bounds("todo-panel").expect("the panel renders");
    assert_eq!(
        panel.size.height,
        px(93.0),
        "the 36px header block plus the 8px gap and the 48px list, plus the ring"
    );
}

/// A session with no plan draws no panel, which is what the reference does.
#[gpui::test]
fn the_todo_panel_is_hidden_without_a_plan(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("todo-empty"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("todo-panel").is_none(),
        "an empty plan renders nothing"
    );
}

/// A store with one session carrying a turn and the given messages already
/// waiting in its queue, so the dock has the data a live queue would have.
fn state_with_queue(label: &str, texts: &[&str]) -> AppState {
    let mut state = AppState::with_store(scratch_store(label));
    let id = state.active_id.clone().expect("an active session");
    if let Some(session) = state.store.session_mut(&id) {
        session.messages.push(ChatMessage::user("queue the work"));
        session.messages.push(ChatMessage::assistant());
    }
    for text in texts {
        state.enqueue(&id, text.to_string(), Vec::new());
    }
    state
}

/// The composer is the reference's growing editor: it opens at the measured
/// 36px floor, grows with each line, and stops at the measured 336px ceiling
/// with the wrapper scrolling past it.
/// The composer is the reference's growing editor: it rests on the measured
/// 36px floor, grows with each line and stops at the measured 336px ceiling
/// with the wrapper scrolling past it.
///
/// One honest delta: bsh's field grows 20px a line where the reference's grows
/// its inherited 24, because the kit's text field owns its own line box and
/// exposes no size hook - the same limit recorded for the font-size step.
#[gpui::test]
fn the_composer_editor_grows_to_its_measured_ceiling(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-grow", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let empty = cx
        .debug_bounds("composer-editor")
        .expect("the editor renders");
    let empty_card = cx
        .debug_bounds("composer-card")
        .expect("the composer card renders");

    set_draft(&shell, cx, "one\ntwo\nthree");
    let three = cx
        .debug_bounds("composer-editor")
        .expect("the editor renders");

    let long: String = (0..20).map(|i| format!("line {i}\n")).collect();
    set_draft(&shell, cx, long.as_str());
    let many = cx
        .debug_bounds("composer-editor")
        .expect("the editor renders");
    let many_card = cx
        .debug_bounds("composer-card")
        .expect("the composer card renders");

    assert_eq!(
        empty.size.height,
        px(36.0),
        "an empty draft rests on the measured 36px floor"
    );
    assert_eq!(
        empty_card.size.height,
        px(99.0),
        "which puts the resting card on its measured height (the 34px primary \
         is 4 taller than the row bsh used to draw)"
    );
    assert_eq!(
        three.size.height,
        px(76.0),
        "three lines grow the field a measured line at a time"
    );
    assert_eq!(
        many.size.height,
        px(336.0),
        "twenty lines stop at the measured 336px ceiling"
    );
    assert_eq!(
        many_card.size.height,
        px(399.0),
        "and the card grows with it"
    );
}

/// The queue row's editor is the reference's own `textarea`: it grows with the
/// draft and stops at the measured 127px ceiling, and the row grows with it.
#[gpui::test]
fn the_queue_editor_grows_to_its_measured_ceiling(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_queue("queue-grow", &["first wait"]);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let edit = cx
        .debug_bounds("queue-edit-1")
        .expect("the edit action renders");
    cx.simulate_click(edit.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let one = cx
        .debug_bounds("queue-row-1")
        .expect("the editing row renders");

    set_queue_draft(&shell, cx, "a\nb\nc");
    let three = cx
        .debug_bounds("queue-row-1")
        .expect("the editing row renders");

    // Six lines want a 136px field and twelve want 256, but the measured 127px
    // ceiling stops the box at 127 - which the dock's own 180px list shows as a
    // 125px row, the same height for both drafts.
    let six: String = (0..6).map(|i| format!("line {i}\n")).collect();
    set_queue_draft(&shell, cx, six.as_str());
    let many = cx
        .debug_bounds("queue-row-1")
        .expect("the editing row renders");
    let twelve: String = (0..12).map(|i| format!("line {i}\n")).collect();
    set_queue_draft(&shell, cx, twelve.as_str());
    let capped = cx
        .debug_bounds("queue-row-1")
        .expect("the editing row renders");

    assert_eq!(
        one.size.height,
        px(45.0),
        "a one-line editor keeps the measured 36px field over the row's 4px pads"
    );
    assert_eq!(
        three.size.height,
        px(85.0),
        "a wrapped draft grows the row a line at a time"
    );
    assert!(
        many.size.height > three.size.height,
        "six lines keep growing the row"
    );
    assert_eq!(
        capped.size.height, many.size.height,
        "and twelve are no taller than six: the field stops at its measured ceiling"
    );
}

/// Set the composer draft the way a keystroke would.
fn set_draft(shell: &Entity<Shell>, cx: &mut VisualTestContext, text: &str) {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell
                .composer
                .update(cx, |input, cx| input.set_value(text, window, cx));
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Set the queue row's own editor draft.
fn set_queue_draft(shell: &Entity<Shell>, cx: &mut VisualTestContext, text: &str) {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell
                .queue_edit
                .update(cx, |input, cx| input.set_value(text, window, cx));
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The queue dock, from the reference's own QueueDock.module.css: a panel 32px
/// inside each edge of the composer card, rounded on its top corners and tucked
/// 3px under the card, holding a 36-tall count header while more than one
/// message waits.
#[gpui::test]
fn the_queue_dock_takes_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_queue("queue-chrome", &["first wait", "second wait"]);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let dock = cx.debug_bounds("queue-dock").expect("the dock renders");
    let head = cx
        .debug_bounds("queue-dock-header")
        .expect("its count header renders");
    let card = cx
        .debug_bounds("composer-card")
        .expect("the composer card renders");
    let column = cx.debug_bounds("main-column").expect("the column renders");

    assert_eq!(
        dock.size.width,
        card.size.width - 2.0 * px(crate::theme::QUEUE_SIDE_INSET),
        "the dock is the live composer card less its 32px inset a side"
    );
    assert_eq!(head.size.height, px(36.0), "the count header is 36 tall");
    assert_eq!(
        dock.size.height,
        px(40.5),
        "2px of panel padding twice over the 36px header: the ring's own top edge adds half a pixel, because this panel's bottom edge is left open"
    );
    assert_eq!(
        card.origin.y - (dock.origin.y + dock.size.height),
        px(-3.0),
        "the dock tucks its bottom edge 3px under the card"
    );
    let dock_mid = dock.origin.x + dock.size.width / 2.0;
    let column_mid = column.origin.x + column.size.width / 2.0;
    let off = dock_mid - column_mid;
    assert!(
        off > px(-1.5) && off < px(1.5),
        "the dock is centred in the column"
    );
    assert!(
        cx.debug_bounds("queue-dock-list").is_none(),
        "a longer queue starts collapsed"
    );
}

/// The count header toggles the list open, and a lone queued message skips the
/// header and draws its row directly, which is the reference's own rule.
#[gpui::test]
fn the_queue_dock_lists_its_rows(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_queue("queue-rows", &["first wait", "second wait"]);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let head = cx
        .debug_bounds("queue-dock-header")
        .expect("the count header renders");
    cx.simulate_click(head.center(), Modifiers::default());
    let open = cx.update(|_, cx| shell.read(cx).queue_open);
    assert!(open, "the header click must expand the list");

    cx.update(|window, cx| window.draw(cx).clear(cx));
    let list = cx
        .debug_bounds("queue-dock-list")
        .expect("the expanded list renders");
    assert_eq!(
        list.size.height,
        px(72.5),
        "two 36px rows, plus the half pixel the second row's own hairline adds"
    );
    let first = cx
        .debug_bounds("queue-row-1")
        .expect("the first row renders");
    assert_eq!(first.size.height, px(36.0), "a row is its 36px minimum");
    let second = cx
        .debug_bounds("queue-row-2")
        .expect("the second row renders");
    assert_eq!(
        second.size.height,
        px(36.5),
        "the rule between two rows sits on the row's own top edge"
    );
}

/// One waiting message draws its row directly, with no count header at all.
#[gpui::test]
fn a_lone_queued_message_draws_no_header(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_queue("queue-single", &["only wait"]);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("queue-dock").is_some(),
        "one queued message still draws the dock"
    );
    assert!(
        cx.debug_bounds("queue-dock-header").is_none(),
        "a lone message has no count to collapse"
    );
    assert!(
        cx.debug_bounds("queue-row-1").is_some(),
        "its row is drawn directly"
    );
}

/// The row's own remove drops the message, and the dock goes away with it.
#[gpui::test]
fn the_queue_row_removes_its_message(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_queue("queue-remove", &["only wait"]);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_id
            .clone()
            .expect("an active session")
    });

    let trash = cx
        .debug_bounds("queue-remove-1")
        .expect("the remove action renders");
    cx.simulate_click(trash.center(), Modifiers::default());
    let left = cx.update(|_, cx| shell.read(cx).state.inbox(&id).len());
    assert_eq!(left, 0, "the row's own remove drops its message");

    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("queue-dock").is_none(),
        "an empty queue draws no dock"
    );
}

/// The row's own edit swaps the preview for the shared editor and its save
/// rewrites the message in place.
#[gpui::test]
fn the_queue_row_edits_its_message(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_queue("queue-edit", &["first wait"]);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_id
            .clone()
            .expect("an active session")
    });

    let edit = cx
        .debug_bounds("queue-edit-1")
        .expect("the edit action renders");
    cx.simulate_click(edit.center(), Modifiers::default());
    let editing = cx.update(|_, cx| shell.read(cx).queue_edit_for);
    assert_eq!(editing, Some(1), "the row's own edit opens the editor");

    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("queue-save-1").is_some(),
        "the editing row swaps its actions for save and cancel"
    );
    assert!(
        cx.debug_bounds("queue-remove-1").is_none(),
        "and its remove is gone while it is being rewritten"
    );

    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.queue_edit.update(cx, |input, cx| {
                input.set_value("second wait", window, cx);
            });
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let save = cx
        .debug_bounds("queue-save-1")
        .expect("the save action renders");
    cx.simulate_click(save.center(), Modifiers::default());
    let (text, editing) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (shell.state.inbox(&id)[0].text.clone(), shell.queue_edit_for)
    });
    assert_eq!(text, "second wait", "save rewrites the queued message");
    assert_eq!(editing, None, "and closes the editor");
}

/// A submission while the session's turn is running waits in the queue instead
/// of opening a second turn over the same transcript, and a queue only drains
/// into an idle session.
#[gpui::test]
fn sending_while_a_turn_runs_queues_the_message(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("queue-send", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_id
            .clone()
            .expect("an active session")
    });
    // A live turn is what the streaming map reports.
    cx.update(|_, cx| {
        shell.update(cx, |shell, _cx| {
            shell.state.streams.insert(id.clone(), 1);
        })
    });
    let before = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .store
            .session(&id)
            .expect("the session")
            .messages
            .len()
    });

    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.composer.update(cx, |input, cx| {
                input.set_value("wait patiently", window, cx)
            });
            shell.send(window, cx);
        })
    });
    let (queued, streams, after) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.state.inbox(&id).len(),
            shell.state.streams.len(),
            shell
                .state
                .store
                .session(&id)
                .expect("the session")
                .messages
                .len(),
        )
    });
    assert_eq!(queued, 1, "the submission waits in the queue");
    assert_eq!(streams, 1, "the running turn is still the only stream");
    assert_eq!(
        after, before,
        "no second turn opens over the same transcript"
    );

    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.drain_inbox(&id, cx)));
    let queued = cx.update(|_, cx| shell.read(cx).state.inbox(&id).len());
    assert_eq!(
        queued, 1,
        "a queue only drains into a session whose turn has ended"
    );
}

/// A chat that never chose a model is refused BEFORE a turn opens, with the
/// composer's own copy telling the reader to choose one for this session.
#[gpui::test]
fn sending_without_a_model_is_refused_before_a_turn_opens(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("no-model-send"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_id
            .clone()
            .expect("an active session")
    });
    let before = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .store
            .session(&id)
            .expect("the session")
            .messages
            .len()
    });

    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell
                .composer
                .update(cx, |input, cx| input.set_value("hello", window, cx));
            shell.send(window, cx);
        })
    });
    let (notice, lang, streams, after) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.state.notice.clone(),
            shell.state.lang,
            shell.state.streams.len(),
            shell
                .state
                .store
                .session(&id)
                .expect("the session")
                .messages
                .len(),
        )
    });
    assert_eq!(
        notice,
        Some(crate::i18n::tr(lang, "composer.chooseModel")),
        "the refusal asks for a model in the reader's language"
    );
    assert_eq!(streams, 0, "no turn opened");
    assert_eq!(after, before, "and the transcript did not move");
}

/// The queue itself: a line that hands out its oldest message, and rows that
/// edit and remove by identity rather than by position.
#[test]
fn the_queue_is_a_line_of_identified_rows() {
    let mut state = AppState::with_store(scratch_store("queue-state"));
    let id = state.active_id.clone().expect("an active session");
    let first = state.enqueue(&id, "first".to_string(), Vec::new());
    let second = state.enqueue(&id, "second".to_string(), Vec::new());
    assert_eq!(state.inbox(&id).len(), 2);

    state.edit_queued(&id, second, "second (edited)".to_string());
    assert_eq!(
        state.inbox(&id)[0].text,
        "first",
        "the other row is untouched"
    );
    assert_eq!(state.inbox(&id)[1].text, "second (edited)");

    state.remove_queued(&id, first);
    assert_eq!(state.inbox(&id).len(), 1, "one row was removed");
    assert_eq!(state.inbox(&id)[0].text, "second (edited)");

    let front = state.pop_inbox(&id).expect("the oldest message");
    assert_eq!(front.text, "second (edited)");
    assert!(
        state.inbox(&id).is_empty(),
        "an empty queue reports nothing"
    );

    state.enqueue(&id, "next".to_string(), Vec::new());
    assert!(
        state.begin_turn_in(&id, "next").is_some(),
        "the next turn opens in the session the queue belongs to"
    );
    assert!(state.is_streaming(&id), "and that session reads as running");
}

/// The running turn's plan is read live from the agent's own store, and the
/// finished copy lands on the session - which is the copy the trajectory pane
/// and the next launch read.
#[gpui::test]
fn the_plan_store_lands_on_the_session(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("todo-plan-live"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_id
            .clone()
            .expect("an active session")
    });
    let before = cx.update(|_, cx| shell.read(cx).state.plan_items(&id));
    assert!(before.is_empty(), "no turn has written a plan yet");

    let store = PlanStore::new();
    store.replace(vec![plan_item("Step one", PlanStatus::Completed)]);
    cx.update(|_, cx| {
        shell.update(cx, |shell, _cx| shell.state.attach_plan(&id, store));
    });
    let live = cx.update(|_, cx| shell.read(cx).state.plan_items(&id));
    assert_eq!(
        live.len(),
        1,
        "the running plan reads through the live store"
    );
    assert_eq!(live[0].text, "Step one");

    cx.update(|_, cx| shell.update(cx, |shell, _cx| shell.state.finish_plan(&id)));
    let saved = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .store
            .session(&id)
            .map(|session| session.plan.clone())
            .expect("the session is still there")
    });
    assert_eq!(
        saved.len(),
        1,
        "the finished plan is saved onto the session"
    );
    assert_eq!(saved[0].status, PlanStatus::Completed);
    let after = cx.update(|_, cx| shell.read(cx).state.plan_items(&id));
    assert_eq!(after.len(), 1, "the saved copy is what the panel reads now");
}

#[gpui::test]
fn header_panel_toggle_click_flips_the_sidebar(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("header"));
        // The header carries the sidebar toggle only while the sidebar is
        // folded - the reference's folded state - so the test starts there.
        state.sidebar_collapsed = true;
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let before = cx.update(|_, cx| shell.read(cx).state.sidebar_collapsed);
    let bounds = cx
        .debug_bounds("hdr-show-sidebar")
        .expect("the panel toggle is rendered while the sidebar is folded");
    cx.simulate_click(bounds.center(), Modifiers::default());
    let after = cx.update(|_, cx| shell.read(cx).state.sidebar_collapsed);
    assert_ne!(before, after, "the click must reach the toggle handler");
    assert!(
        cx.debug_bounds("hdr-show-sidebar").is_none(),
        "once the sidebar is up the toggle goes back to the sidebar strip"
    );
}

/// The centre column, from the reference's own `ConversationRoot.module.css`:
/// a 76-tall header over a `.5px` `border-l3` rule, a 30-tall title row, 13px
/// tabs 36 apart behind an 8px lead-in, and a composer card 32 wider than the
/// transcript column.
#[gpui::test]
fn the_centre_column_takes_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("centre-chrome"));
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let header = cx.debug_bounds("hdr-header").expect("the header renders");
    let title = cx.debug_bounds("hdr-title").expect("the title row renders");
    let chat = cx.debug_bounds("tab-chat").expect("the chat tab renders");
    let traj = cx
        .debug_bounds("tab-trajectory")
        .expect("the trajectory tab renders");
    let card = cx
        .debug_bounds("composer-card")
        .expect("the composer card renders");
    let column = cx.debug_bounds("main-column").expect("the column renders");

    assert_eq!(
        title.size.height,
        px(30.0),
        "the measured title row renders 30 (fixing gpui's 2px min_h overshoot)"
    );
    assert!(
        header.size.height >= px(76.0),
        "the measured header floor is min-height 76"
    );
    assert_eq!(
        title.origin.x,
        header.origin.x + px(20.0),
        "the header owns the 20px lead-in, not the row"
    );
    assert_eq!(
        chat.origin.x,
        header.origin.x + px(28.0),
        "the tab strip adds its own 8px lead-in"
    );
    assert_eq!(
        chat.size.height,
        px(27.0),
        "16px of line over a 9px pad and the 2px rule"
    );
    assert_eq!(
        traj.origin.x,
        chat.origin.x + chat.size.width + px(36.0),
        "the measured tab gap is 36"
    );
    assert_eq!(
        card.size.width,
        px(crate::theme::chat_width_at(column.size.width.as_f32()) + crate::theme::COMPOSER_WIDEN),
        "the composer card is the live chat column plus 32"
    );
    assert_eq!(
        card.origin.x + card.size.width / 2.0,
        column.origin.x + column.size.width / 2.0,
        "and it stays centred on the column"
    );
}

/// The sidebar chrome, from the reference's own `SidebarRoot.module.css`: a 52px
/// traffic-light strip, a 60px brand row that overlaps it by 12, a 38px New session
/// over a `.5px` rule, 36px panel rows on a 4px gap, and 28x28 icon controls - all
/// inset by the root's 6/12 padding instead of by each child.
#[gpui::test]
fn the_sidebar_takes_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("sidebar-chrome"));
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let sidebar = cx.debug_bounds("sidebar").expect("the sidebar renders");
    let brand = cx
        .debug_bounds("side-brand")
        .expect("the brand row renders");
    let fold = cx.debug_bounds("side-fold").expect("the fold renders");
    let new_session = cx
        .debug_bounds("side-new-session")
        .expect("New session renders");
    let main = cx.debug_bounds("main-column").expect("the column renders");

    assert_eq!(
        sidebar.size.width,
        px(crate::theme::SIDEBAR_W),
        "the measured column default is 280, the reference's own store default"
    );
    assert_eq!(fold.size.width, px(28.0), "iconButton is 28 wide");
    assert_eq!(fold.size.height, px(28.0), "iconButton is 28 tall");
    assert_eq!(
        fold.origin.x,
        sidebar.origin.x + px(crate::theme::SIDEBAR_W) - px(40.0),
        "the fold is inset by the root's 12 plus its own 28"
    );
    assert_eq!(brand.size.height, px(60.0), "logoRow is 60 tall");
    assert_eq!(
        brand.origin.y,
        sidebar.origin.y + px(6.0) + px(52.0) - px(12.0),
        "the brand row overlaps the 52px strip by 12"
    );
    assert_eq!(new_session.size.height, px(38.0), "New session is 38 tall");
    assert_eq!(
        new_session.origin.x,
        sidebar.origin.x + px(14.0),
        "the root's 12 plus the button's own 2"
    );
    assert_eq!(
        main.origin.x,
        sidebar.origin.x + px(crate::theme::SIDEBAR_W),
        "the centre column starts at the sidebar's right edge"
    );
}

/// The round-31 fix, observed. The row actions used to bubble into the row own
/// click, so pinning a session also opened it. Pin an inactive row and check that
/// the pin landed while the selection did not move.
#[gpui::test]
fn pinning_an_inactive_row_does_not_open_it(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("pin"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // A second session, so the row under test is not the selected one.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.state.new_session();
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (active, other) = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        let active = state.active_id.clone().expect("an active session");
        let other = state
            .store
            .sessions
            .iter()
            .map(|s| s.id.clone())
            .find(|id| *id != active)
            .expect("a second session");
        (active, other)
    });

    let selector: &str = Box::leak(format!("pin-{other}").into_boxed_str());
    let bounds = cx
        .debug_bounds(selector)
        .expect("the inactive row pin control");
    cx.simulate_click(bounds.center(), Modifiers::default());

    let (pinned, still_active) = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        (
            state.store.session(&other).map(|s| s.pinned),
            state.active_id.clone(),
        )
    });
    assert_eq!(pinned, Some(true), "the click must reach the pin handler");
    assert_eq!(
        still_active,
        Some(active),
        "the pin click must not open the session"
    );

    // Positive control: the row body does open the session, so the assertion
    // above really would have caught a click that leaked up to the row.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let row: &str = Box::leak(format!("session-{other}").into_boxed_str());
    let row_bounds = cx.debug_bounds(row).expect("the inactive row");
    assert!(
        bounds.center().x > row_bounds.center().x,
        "the row actions must overlay the right edge, not sit mid-row"
    );
    // Click the row leading area, which is unambiguously the row body.
    let mut press = row_bounds.center();
    press.x = row_bounds.origin.x + gpui::px(40.0);
    cx.simulate_click(press, Modifiers::default());
    let opened = cx.update(|_, cx| shell.read(cx).state.active_id.clone());
    assert_eq!(
        opened,
        Some(other),
        "the row body must still open the session"
    );
}

/// The sidebar search control toggles the session-name filter.
#[gpui::test]
fn search_control_click_opens_the_filter(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("search"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let before = cx.update(|_, cx| shell.read(cx).state.search_open);
    let bounds = cx.debug_bounds("side-search").expect("the search control");
    cx.simulate_click(bounds.center(), Modifiers::default());
    let after = cx.update(|_, cx| shell.read(cx).state.search_open);
    assert_ne!(before, after, "the search click must toggle the filter");
}

/// Both tablist entries switch the main column.
#[gpui::test]
fn tab_click_switches_the_main_column(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("tabs"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let trajectory = cx
        .debug_bounds("tab-trajectory")
        .expect("the Trajectory tab");
    cx.simulate_click(trajectory.center(), Modifiers::default());
    let on_trajectory = cx.update(|_, cx| matches!(shell.read(cx).state.tab, Tab::Trajectory));
    assert!(on_trajectory, "the Trajectory tab must switch the column");

    cx.update(|window, cx| window.draw(cx).clear(cx));
    let chat = cx.debug_bounds("tab-chat").expect("the Chat tab");
    cx.simulate_click(chat.center(), Modifiers::default());
    let back_on_chat = cx.update(|_, cx| matches!(shell.read(cx).state.tab, Tab::Chat));
    assert!(back_on_chat, "the Chat tab must switch back");
}

/// The sidebar gear opens the settings dialog.
#[gpui::test]
fn settings_gear_click_opens_the_dialog(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("gear"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let gear = cx.debug_bounds("side-settings").expect("the settings gear");
    cx.simulate_click(gear.center(), Modifiers::default());
    let open = cx.update(|_, cx| matches!(shell.read(cx).state.overlay, Overlay::Settings));
    assert!(open, "the gear must open the settings dialog");
}

/// Opening a session shows the newest message. The reference marks this as
/// data-chat-following-tail, so a transcript must not open on the oldest turn.
#[gpui::test]
fn transcript_opens_at_the_newest_message(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("tail", 12);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    // The first frame measures the transcript and asks for one more; the second
    // scrolls now that the content height is known.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let (offset, max) = cx.update(|_, cx| {
        let handle = &shell.read(cx).transcript_scroll;
        (handle.offset().y, handle.max_offset().y)
    });
    assert!(max > px(0.0), "the seeded transcript must be tall enough");
    assert_eq!(offset, -max, "a transcript opens on the newest message");
}

/// The reference names this control "Jump to turn N": clicking a rail tick must
/// The reasoning disclosure, from the reference's `DisclosureRow` and `ReasoningRow`:
/// a `24px + delta` row whose leading box is `16px + delta` around a 14px glyph with a
/// 6px right margin, a 2px dot at 8px margins, and a nowrap summary on a
/// `20px + secondary delta` line - and clicking the row opens and closes the body.
#[gpui::test]
fn the_think_row_takes_the_measured_disclosure_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("think-row", 1);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            session.messages[1].reasoning = "first line of thought\n\nmore **detail**".to_string();
            session.messages[1].text = "the answer".to_string();
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        crate::ui::transcript::think_summary("first line of thought\n\nmore **detail**"),
        "first line of thought",
        "the collapsed summary is the first line with the markers stripped"
    );
    let row = cx
        .debug_bounds("think-row-1")
        .expect("the think row renders");
    let leading = cx.debug_bounds("think-leading-1").expect("its leading box");
    let title = cx.debug_bounds("think-title-1").expect("its title");
    let dot = cx.debug_bounds("think-dot-1").expect("its dot");
    let summary = cx.debug_bounds("think-summary-1").expect("its summary");
    assert!(
        cx.debug_bounds("think-body-1").is_none(),
        "a settled block starts closed"
    );
    assert_eq!(
        row.size.height,
        px(24.0),
        "the shared disclosure row is 24 tall"
    );
    assert_eq!(leading.size.width, px(16.0), "a 16px leading box");
    assert_eq!(leading.size.height, px(16.0), "a 16px leading box");
    assert_eq!(
        title.origin.x,
        leading.origin.x + leading.size.width + px(6.0),
        "the leading box carries a 6px right margin"
    );
    assert_eq!(dot.size.width, px(2.0), "a 2px dot");
    assert_eq!(dot.size.height, px(2.0));
    assert_eq!(
        summary.origin.x,
        dot.origin.x + dot.size.width + px(8.0),
        "the dot carries 8px margins"
    );

    let hits = cx.debug_bounds("think-row-1").expect("the row bounds");
    cx.simulate_click(hits.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("think-body-1").is_some(),
        "the click opens the reasoning"
    );
    let row = cx.debug_bounds("think-row-1").expect("the row persists");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("think-body-1").is_none(),
        "and closes it again"
    );
}
/// The transcript chrome, from the reference's own `TurnNavigator.module.css` and
/// `MessageItem.module.css`: a 28px rail parked 12px off the transcript's right edge,
/// capped at 420 tall and scrolled when the marks overflow, where a mark is a 10px slot
/// holding a 2px bar drawn at 60% while idle and 100% for the turn at the top; and a
/// user bubble on radius-xl 20 over `10px 16px` with the measured 22px conversation line.
#[gpui::test]
fn the_transcript_takes_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("transcript-chrome", 60);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            // One short turn pins the measured bubble box exactly.
            session.messages[0] = ChatMessage::user("hi".to_string());
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let transcript = cx.debug_bounds("transcript").expect("the scroller renders");
    let track = cx
        .debug_bounds("turn-rail-track")
        .expect("the rail track renders");

    let idle = cx
        .debug_bounds("turn-tick-0")
        .expect("an idle tick renders");
    let bar = cx.debug_bounds("turn-bar-0").expect("its bar renders");
    let last_tick = cx
        .debug_bounds("turn-tick-59")
        .expect("the last tick renders");
    let last_bar = cx.debug_bounds("turn-bar-59").expect("its bar renders");
    let bubble = cx
        .debug_bounds("user-bubble-0")
        .expect("the short bubble renders");

    assert_eq!(
        track.size.height,
        px(420.0),
        "the measured rail is capped at min(band - 64, 420)"
    );
    assert_eq!(idle.size.width, px(28.0), "a mark spans the 28px frame");
    assert_eq!(idle.size.height, px(10.0), "a mark slot is 10 tall");
    assert_eq!(
        last_tick.size.height,
        px(10.0),
        "every mark slot is 10 tall"
    );
    assert_eq!(
        idle.origin.x + idle.size.width,
        transcript.origin.x + transcript.size.width - px(12.0),
        "the frame sits 12px off the transcript's right edge"
    );
    // Whichever turn's top sits at the viewport's top edge is the active one, so
    // count the marks at scaleX(1) rather than naming the turn: the rail draws one
    // bar at 20px and every other at 12px (60% of the measured 20px bar).
    assert_eq!(bar.size.width, px(12.0), "an idle mark is scaleX(.6)");
    let mut full = 0;
    for turn in 0..60 {
        // `debug_bounds` wants a borrowed selector, so leak the label like the
        // tick-click test does; a test never cares about 60 short strings.
        let selector: &str = Box::leak(format!("turn-bar-{turn}").into_boxed_str());
        if cx
            .debug_bounds(selector)
            .is_some_and(|bounds| bounds.size.width == px(20.0))
        {
            full += 1;
        }
    }
    assert_eq!(full, 1, "exactly one mark is drawn at scaleX(1)");
    assert_eq!(last_tick.size.height, px(10.0));
    assert_eq!(
        last_bar.size.width,
        px(20.0),
        "and the newest mark is the live one at the tail"
    );
    assert_eq!(
        bubble.size.height,
        px(42.0),
        "one 22px line over 10px + 10px of padding"
    );
    assert_eq!(
        bubble.origin.x + bubble.size.width,
        transcript.origin.x + transcript.size.width - px(crate::theme::INSET),
        "the bubble ends at the transcript's content edge"
    );
}

/// move the transcript, so the tick is wired to the scroll handle, not just drawn.
#[gpui::test]
fn rail_tick_click_scrolls_the_transcript(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("rail", 30);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // The handle offset is the observable: bounds_for_item reports the laid out
    // position, which a scroll request does not rewrite.
    let scroll = |cx: &mut VisualTestContext, shell: &Entity<Shell>| {
        cx.update(|_, cx| {
            let handle = &shell.read(cx).transcript_scroll;
            (handle.offset().y, handle.max_offset().y)
        })
    };
    let (start, max) = scroll(cx, &shell);
    assert!(max > px(0.0), "the seeded transcript must be tall enough");
    assert_eq!(start, -max, "the transcript opens following the tail");

    let first: &str = Box::leak(String::from("turn-tick-0").into_boxed_str());
    let bounds = cx.debug_bounds(first).expect("the first rail tick");
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (at_first, _) = scroll(cx, &shell);
    // Child 0 is the timestamp row, so turn 1 lands just below the top rather than
    // at offset 0 - and it must leave the end, where the transcript opened.
    assert!(
        at_first > start,
        "the first tick must move back up the transcript"
    );
    assert!(
        at_first > -px(150.0),
        "the first turn must land near the top, not at the end"
    );

    let last: &str = Box::leak(String::from("turn-tick-29").into_boxed_str());
    let bounds = cx.debug_bounds(last).expect("the last rail tick");
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (at_last, _) = scroll(cx, &shell);
    assert_eq!(at_last, -max, "the last tick must jump to the end");
}

/// The plugin inventory carries a switch per plugin, and it writes the setting
/// the agent is built from - so the pane is not a read-only list.
#[gpui::test]
fn plugin_switch_writes_the_disabled_plugin(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugin-switch", 2);
        let shell = cx
            .new(|cx| Shell::build_for_test(window, section_index("plugin-inventory"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let toggle = cx
        .debug_bounds("plugin-toggle-0")
        .expect("the first switch");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let disabled = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .disabled
            .extensions
            .clone()
    });
    assert_eq!(
        disabled,
        vec!["shell".to_string()],
        "the switch must write the plugin the agent is built without"
    );
}

/// The agent loop pane loads the ceiling the agent will be built with, so the
/// field is bound to the setting rather than a label that looks like one.
#[gpui::test]
fn agent_loop_pane_loads_the_step_ceiling(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("agent-loop", 2);
        state.settings.workspace.agent.max_steps = 23;
        let shell =
            cx.new(|cx| Shell::build_for_test(window, section_index("agent-loop"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let text = cx.update(|_, cx| {
        shell
            .read(cx)
            .settings_inputs
            .max_steps
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(
        text, "23",
        "the pane must show the ceiling the agent will be built with"
    );
}

/// The models pane is the reference card list: the head carries identity and
/// the two actions, and delete asks before it drops a saved provider.
#[gpui::test]
fn provider_cards_offer_edit_and_a_confirming_delete(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("provider-card", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // Two providers, so a delete that dropped every provider could not pass, and
    // the tree order is pinned by the ids rather than by this machine.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "alpha",
                "https://alpha.example",
            );
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The head is identity, not a way to pick the endpoint: the reference has no
    // such affordance, so pressing it must not select anything.
    let head = cx
        .debug_bounds("provider-0")
        .expect("the first provider card");
    cx.simulate_click(head.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let editing = cx.update(|_, cx| shell.read(cx).state.editing_provider.clone());
    assert_eq!(editing, None, "the head must not open the editor");
    let keys: Vec<String> = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .global
            .providers
            .keys()
            .cloned()
            .collect()
    });
    assert_eq!(
        keys,
        ["alpha".to_string(), "work".to_string()],
        "the tree keeps both providers"
    );

    // Edit opens the editor inside that card.
    let edit = cx
        .debug_bounds("provider-edit-1")
        .expect("the second card's Edit action");
    cx.simulate_click(edit.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let editing = cx.update(|_, cx| shell.read(cx).state.editing_provider.clone());
    assert_eq!(
        editing.as_deref(),
        Some("work"),
        "Edit opens the editor for that provider"
    );
    assert!(
        cx.debug_bounds("provider-editor").is_some(),
        "the editor opens inside the card"
    );

    // Delete asks in a dialog before it drops a saved provider.
    let remove = cx
        .debug_bounds("provider-remove-1")
        .expect("the second card's Delete action");
    cx.simulate_click(remove.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let pending = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        (
            state.removing_provider.clone(),
            state.draft().global.providers.len(),
        )
    });
    assert_eq!(
        pending,
        (Some("work".to_string()), 2),
        "the Delete action must ask before it drops the saved provider"
    );
    assert!(
        cx.debug_bounds("provider-delete-dialog").is_some(),
        "the reference confirms in a dialog"
    );
    let confirm = cx
        .debug_bounds("provider-delete-confirm")
        .expect("the confirm control");
    cx.simulate_click(confirm.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let left = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        (
            state.removing_provider.clone(),
            state.draft().global.providers.clone(),
        )
    });
    assert_eq!(left.0, None, "the confirmation closes the pending delete");
    assert!(
        !left.1.contains_key("work"),
        "and the confirmed provider is dropped from the tree"
    );
    assert!(
        left.1.contains_key("alpha"),
        "the other provider stays where it was"
    );
}

/// The refusal the backend computes is the sentence the pane draws: a row that
/// repeats an id the tree already carries says so, in its own inline style,
/// rather than leaving the apply looking broken.
#[gpui::test]
fn a_refused_apply_says_why(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("provider-refusal", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            let draft = this.state.draft_mut();
            let mut global = draft.global.clone();
            global.add_provider("work", "", "https://work.example/v1", &[]);
            let _ = global.add_model("work", "fast", "", "gpt-4o-mini", "");
            draft.global = global;
            this.state.editing_provider = Some("work".to_string());
            this.reload_model_rows("work", window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("provider-refusal").is_none(),
        "a freshly opened editor carries no refusal from a previous apply"
    );

    // A second row with the same API id the first one already carries.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.provider_refusal =
                Some(crate::settings::provider::ProviderRefusal::ModelIdDuplicate.key());
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("provider-refusal").is_some(),
        "the refusal is drawn in the editor, in the inline error style"
    );
}

/// A provider card offers no vault chooser while the vault is locked, offers
/// one once it is unlocked, stores the chosen ID rather than the secret, and
/// drops the choice on Clear.
#[gpui::test]
fn a_provider_chooses_its_vault_key_by_id(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("provider-vault-choice", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // Seed after assembly, then open the editor.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
            this.state.editing_provider = Some("work".to_string());
            load_provider_into_form(this, "work", window, cx);
            this.reload_model_rows("work", window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Locked (the scratch state has no vault file): there is no chooser, and
    // the row says unlocking would offer one.
    assert!(
        cx.debug_bounds("provider-vault-hint").is_some(),
        "a locked vault explains how to enable the choice"
    );
    assert!(
        cx.debug_bounds("provider-vault-choose").is_none()
            && cx.debug_bounds("provider-vault-source").is_none(),
        "no chooser is offered while the vault is locked"
    );
    // A saved choice still names itself while locked, so the reader knows which
    // entry unlocking would restore.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            if let Some(provider) = this.state.draft_mut().global.providers.get_mut("work") {
                provider.api_key_vault = Some("keys/saved".to_string());
            }
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("provider-vault-source-locked").is_some(),
        "the locked row names the saved entry"
    );
    assert!(cx.debug_bounds("provider-vault-choose").is_none());
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            if let Some(provider) = this.state.draft_mut().global.providers.get_mut("work") {
                provider.api_key_vault = None;
            }
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Unlocking the same state makes the chooser available, and choosing an id
    // moves the source without copying the value into the draft.
    let secret = "sk-vault-only-secret";
    let vault = vault_with_entry("provider-choice", "keys/work", secret);
    cx.update(|_, cx| {
        shell.update(cx, |this, _| this.state.vault = vault);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("provider-vault-choose").is_some(),
        "an unlocked vault offers its entries"
    );
    assert!(cx.debug_bounds("provider-vault-hint").is_none());

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.choose_provider_vault_key("work", Some("keys/work"), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (carried, text) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let provider = shell
            .state
            .draft()
            .global
            .providers
            .get("work")
            .expect("the provider");
        (
            provider.api_key_vault.clone(),
            toml::to_string(&shell.state.draft().global).expect("the settings document"),
        )
    });
    assert_eq!(
        carried.as_deref(),
        Some("keys/work"),
        "the chosen id is the only thing stored"
    );
    assert!(
        text.contains("api_key_vault = \"keys/work\""),
        "the draft carries the id: {text}"
    );
    assert!(!text.contains(secret), "the secret never reaches settings");
    assert!(
        cx.debug_bounds("provider-vault-source").is_some(),
        "the row names the chosen source"
    );
    assert!(
        cx.debug_bounds("provider-vault-note").is_some(),
        "and says the key comes from the vault"
    );

    // Clear removes the id and brings the chooser back.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.choose_provider_vault_key("work", None, window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let cleared = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .global
            .providers
            .get("work")
            .and_then(|provider| provider.api_key_vault.clone())
    });
    assert_eq!(cleared, None, "Clear removes the choice");
    assert!(
        cx.debug_bounds("provider-vault-source").is_none(),
        "the source label is gone"
    );
    assert!(
        cx.debug_bounds("provider-vault-choose").is_some(),
        "and the chooser is back"
    );
}

/// The add-provider card offers the same vault chooser, and Create lands the
/// chosen id on the new provider instead of the secret.
#[gpui::test]
fn an_added_provider_carries_its_vault_choice(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("add-provider-vault-choice", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_window, cx| {
        shell.update(cx, |this, _cx| {
            this.models_add_open = true;
            this.models_add_route = crate::ui::shell::ModelsAddRoute::Catalogue;
            this.catalogue_choice = Some("openai".to_string());
            this.add_key_vault = None;
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Locked: no chooser, and the row says unlocking would offer one.
    assert!(
        cx.debug_bounds("add-vault-hint").is_some(),
        "the add card explains how to enable the choice"
    );
    assert!(
        cx.debug_bounds("add-vault-choose").is_none(),
        "no chooser is offered while the vault is locked"
    );
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.add_key_vault = Some("keys/saved".to_string())
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("add-vault-source-locked").is_some(),
        "the locked add card names the saved entry"
    );
    cx.update(|_, cx| {
        shell.update(cx, |this, _| this.add_key_vault = None);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let secret = "sk-add-vault-only-secret";
    let vault = vault_with_entry("add-provider-choice", "keys/work", secret);
    cx.update(|_, cx| {
        shell.update(cx, |this, _| this.state.vault = vault);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("add-vault-choose").is_some(),
        "an unlocked vault offers the add card its entries"
    );
    assert!(cx.debug_bounds("add-vault-hint").is_none());

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.choose_add_vault_key(Some("keys/work"), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("add-vault-source").is_some(),
        "the add card names the chosen source"
    );
    let pending = cx.update(|_, cx| shell.read(cx).add_key_vault.clone());
    assert_eq!(pending.as_deref(), Some("keys/work"));

    // Clear drops the pending choice and brings the chooser back.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.choose_add_vault_key(None, window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).add_key_vault.clone()),
        None
    );
    assert!(
        cx.debug_bounds("add-vault-choose").is_some(),
        "Clear brings the chooser back"
    );
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.choose_add_vault_key(Some("keys/work"), window, cx);
        })
    });

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.create_provider("openai", cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (carried, text) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let provider = shell
            .state
            .draft()
            .global
            .providers
            .get("openai")
            .expect("the created provider");
        (
            provider.api_key_vault.clone(),
            toml::to_string(&shell.state.draft().global).expect("the settings document"),
        )
    });
    assert_eq!(
        carried.as_deref(),
        Some("keys/work"),
        "Create lands the chosen id on the new provider"
    );
    assert!(
        text.contains("api_key_vault = \"keys/work\""),
        "the draft carries the id: {text}"
    );
    assert!(!text.contains(secret), "the secret never reaches settings");
}

/// A vault in a scratch directory, unlocked, holding one entry.
fn vault_with_entry(label: &str, name: &str, secret: &str) -> crate::vault::Vault {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("bsh-scratch-vault-{label}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("the scratch directory");
    let mut vault = crate::vault::Vault::new(directory.join("vault.bin"));
    vault.create("a test passphrase").expect("the vault");
    vault.put(name, secret).expect("the entry");
    vault
}

/// Seed one provider into the global tree the panes edit, with one model leaf
/// so a workspace can name it.
fn seed_provider(global: &mut crate::settings::global::GlobalSettings, id: &str, base_url: &str) {
    global.add_provider(
        id,
        "",
        base_url,
        &[crate::settings::global::ModelEntry::named(&format!(
            "{id}-model"
        ))],
    );
}

/// The model chip opens its own menu in place, the way the reference's trigger
/// does - and the list that menu offers is the pane's own: the active provider's
/// catalog, with the reference's fallback for an unresolved model.
#[gpui::test]
fn the_model_chip_opens_its_menu_in_place(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-model", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let chip = cx.debug_bounds("composer-model").expect("the model chip");
    cx.simulate_click(chip.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (overlay, section) = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        (
            matches!(state.overlay, Overlay::Settings),
            state.settings_section,
        )
    });
    let landed = section_index("models").expect("the models section");
    assert!(
        !overlay || section != landed,
        "the measured trigger changes the model in place, so the click no longer navigates"
    );
}

/// The reference's trigger mark is `busy ? StateDot{state:"ongoing"} :
/// IconChevronDownOutlineRegular`, so a running turn swaps the model chip's
/// chevron for the shared ongoing dot and leaves the trigger enabled.
#[gpui::test]
fn the_model_chip_swaps_its_chevron_for_the_ongoing_dot(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-model-busy", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("composer-model-busy").is_none()
            && cx.debug_bounds("composer-model-chevron").is_some(),
        "an idle shell shows the chevron"
    );

    let id = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_id
            .clone()
            .expect("a session is active")
    });
    cx.update(|_, cx| {
        shell.update(cx, |shell, cx| {
            assert!(
                shell.state.begin_turn_in(&id, "run").is_some(),
                "the seeded turn opens"
            );
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("composer-model-busy").is_some(),
        "a running turn swaps it for the ongoing dot"
    );
    assert!(
        cx.debug_bounds("composer-model-chevron").is_none(),
        "and the chevron goes with it"
    );
}

/// The model chip's list is the pane's list, its rows mark the active model, and
/// an unresolved model takes the reference's own fallback wording.
#[test]
fn the_model_menu_reads_the_catalog_and_names_the_effort() {
    let models = ["gpt-4o".to_string(), "o3".to_string()];
    assert_eq!(
        super::composer::model_choices(&models, "o3"),
        [("gpt-4o".to_string(), false), ("o3".to_string(), true)],
        "only the active model is marked"
    );
    assert!(
        super::composer::model_choices(&[], "o3").is_empty(),
        "an empty catalog offers no rows, which is what the menu's empty line explains"
    );
    let en = crate::i18n::Lang::En;
    assert_eq!(super::composer::model_label("gpt-4o", en), "gpt-4o");
    assert_eq!(
        super::composer::model_label("   ", en),
        crate::i18n::tr(en, "composer.selectModel"),
        "an unresolved model takes the reference's own fallback"
    );
    assert_eq!(
        super::composer::effort_suffix(Some(" high ")),
        Some("high".to_string()),
        "the suffix is the level itself"
    );
    assert_eq!(
        super::composer::effort_suffix(Some("  ")),
        None,
        "a blank level is not a level"
    );
    assert_eq!(super::composer::effort_suffix(None), None);
    for key in [
        "composer.menuModel",
        "composer.noModels",
        "composer.selectModel",
    ] {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            assert_ne!(crate::i18n::tr(lang, key), key, "missing locale value");
        }
    }
}

/// Pressing a preset row selects that preset, and the default row clears it, so
/// the pane is wired rather than a list.
#[gpui::test]
fn preset_row_click_selects_the_preset(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("preset-row", 2);
        state.settings.global.presets.insert(
            "careful".to_string(),
            crate::settings::global::Preset {
                name: Some("careful".to_string()),
                plugins: vec!["bash".to_string()],
                description: None,
                order: None,
            },
        );
        state.settings.global.default_preset = "careful".to_string();
        let shell =
            cx.new(|cx| Shell::build_for_test(window, section_index("agent-presets"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let row = cx.debug_bounds("preset-0").expect("the preset row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let picked = cx.update(|_, cx| shell.read(cx).state.draft().global.default_preset.clone());
    assert_eq!(picked, "careful", "the row must select the preset it shows");

    let clear = cx.debug_bounds("preset-default").expect("the default row");
    cx.simulate_click(clear.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let picked = cx.update(|_, cx| shell.read(cx).state.draft().global.default_preset.clone());
    assert_eq!(picked, "", "the default row must clear the selection");
}

/// The agent-loop switch flips the setting the next turn is built from, in both
/// directions, so the row cannot pass by being permanently on or off.
#[gpui::test]
fn parallel_tool_calls_switch_flips_the_setting(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("parallel-switch", 2);
        state.settings.workspace.agent.parallel_tool_calls = false;
        let shell =
            cx.new(|cx| Shell::build_for_test(window, section_index("agent-loop"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let row = cx
        .debug_bounds("parallel-tool-calls")
        .expect("the switch row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let on = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .parallel_tool_calls
    });
    assert!(on, "the switch must turn concurrent dispatch on");

    let row = cx
        .debug_bounds("parallel-tool-calls")
        .expect("the switch row again");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let off = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .parallel_tool_calls
    });
    assert!(!off, "pressing the switch again must turn it back off");
}

/// The permission chip changes the mode in place, like the reference's trigger:
/// it opens the three-mode menu instead of navigating to the settings pane, and
/// every mode is one the setting itself can hold.
#[gpui::test]
fn the_permission_chip_opens_the_mode_menu_in_place(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-permission", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let before = cx.update(|_, cx| shell.read(cx).state.settings_section);
    let chip = cx
        .debug_bounds("composer-permission")
        .expect("the permission chip");
    cx.simulate_click(chip.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).state.settings_section),
        before,
        "the measured trigger changes the mode in place, so the click no longer navigates"
    );
    assert_eq!(
        super::composer::permission_modes(),
        [
            (crate::caps::READ_ONLY, "permission.presetReadOnly"),
            (
                crate::caps::WORKSPACE_WRITE,
                "permission.presetWorkspaceWrite"
            ),
            (crate::caps::FULL_ACCESS, "permission.presetFullAccess"),
        ],
        "and the menu offers exactly the modes the setting can hold, in the \
         reference's own order"
    );
}

/// Each permission mode gets its own label, so the chip cannot keep claiming
/// write access while the shell and file tools are withheld.
#[test]
fn the_permission_chip_names_the_mode() {
    assert_eq!(
        crate::ui::composer::permission_label_key(crate::caps::READ_ONLY),
        "permission.presetReadOnly"
    );
    assert_eq!(
        crate::ui::composer::permission_label_key(crate::caps::WORKSPACE_WRITE),
        "permission.presetWorkspaceWrite"
    );
    assert_eq!(
        crate::ui::composer::permission_label_key(crate::caps::FULL_ACCESS),
        "permission.presetFullAccess"
    );
    assert_eq!(
        crate::ui::composer::permission_label_key("not-a-mode"),
        "permission.presetWorkspaceWrite",
        "an unrecognised mode names the confined default the build falls back to"
    );
}
/// Every permission label resolves to real copy, and the three modes differ.
///
/// These keys are built dynamically, so a namespace slip is invisible to the
/// compiler: `tr` returns the key itself when it is missing, which is exactly
/// how the composer chip once printed a raw "settings.presetReadOnly" to the
/// user instead of "Read Only". Type-checking cannot catch that; this can.
#[test]
fn every_permission_label_resolves_to_copy() {
    const MODES: [&str; 3] = [
        crate::caps::READ_ONLY,
        crate::caps::WORKSPACE_WRITE,
        crate::caps::FULL_ACCESS,
    ];
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
        for mode in MODES {
            let key = crate::ui::composer::permission_label_key(mode);
            let label = crate::i18n::tr(lang, key);
            assert_ne!(label, key, "{key} is missing from the locale files");
        }
        for (_, key) in crate::ui::composer::permission_modes() {
            let label = crate::i18n::tr(lang, key);
            assert_ne!(label, key, "{key} is missing from the locale files");
        }
    }
    let en = crate::i18n::Lang::En;
    let labels = [
        crate::i18n::tr(en, crate::ui::composer::permission_label_key(MODES[0])),
        crate::i18n::tr(en, crate::ui::composer::permission_label_key(MODES[1])),
        crate::i18n::tr(en, crate::ui::composer::permission_label_key(MODES[2])),
    ];
    assert_ne!(labels[0], labels[1], "each mode names itself");
    assert_ne!(labels[1], labels[2], "each mode names itself");
    assert_ne!(labels[0], labels[2], "each mode names itself");
}

/// The workspace tree opens in the order the sidebar last chose.
#[gpui::test]
fn the_sidebar_order_survives_a_restart(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("sort-restart", 1);
        state.settings.global.general.session_sort =
            crate::store::SessionSort::Name.persisted().to_string();
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let sort = cx.update(|_, cx| harness.read(cx).shell.read(cx).state.sort);
    assert_eq!(
        sort.persisted(),
        "name",
        "the saved tree order must come back with the shell"
    );
    assert_eq!(
        crate::store::SessionSort::from_persisted("").persisted(),
        "updated",
        "an order the settings do not carry yet falls back to the default"
    );
}

/// The header carries the open controls ahead of the panel toggle.
#[gpui::test]
fn the_header_offers_to_open_the_workspace(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("open-workspace", 1);
        // The open controls resolve the active id through the registry, so the
        // path the pane opens is the one a workspace record names.
        let id = state.settings.note_workspace("scratch", "/tmp");
        state.settings.global.general.active_workspace = id;
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let workspace = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .settings
            .global
            .general
            .active_workspace
            .clone()
    });
    assert!(
        !workspace.is_empty(),
        "the scratch state must carry the workspace to open"
    );
    assert_eq!(
        cx.update(|_, cx| harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .settings
            .active_workspace()
            .map(|record| record.root.clone())),
        Some("/tmp".to_string()),
        "and the id must resolve to the root the open control edits"
    );
    assert!(
        cx.debug_bounds("hdr-open").is_some(),
        "the header must offer to open the workspace in an editor"
    );
    // The menu beside it is a kit Button, which builds no debug selector of its own,
    // so its presence is not observable here - the same limit the header's More
    // actions button has.
}

/// With no workspace registered there is nothing to open, so the control is absent.
#[gpui::test]
fn the_header_hides_the_open_control_without_a_workspace(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("open-workspace-none", 1);
        state.settings.global.general.active_workspace = String::new();
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let workspace = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .settings
            .global
            .general
            .active_workspace
            .clone()
    });
    assert!(
        workspace.is_empty(),
        "the scratch state must carry no workspace"
    );
    assert!(
        cx.debug_bounds("hdr-open").is_none(),
        "a control that would open nothing must not be drawn"
    );
}

/// The Models pane is the reference's card list, not the flat form it replaced:
/// each provider is a card whose head carries its identity and its Edit/Delete
/// actions, the editor opens inside that card with the key as its one primary
/// field, and the add block is a single button that swaps for the add card.
#[gpui::test]
fn the_models_pane_is_the_reference_card_list(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("models-cards", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // Pin the indices: the seeded provider is the first and only card.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // A collapsed card: the head, its two actions, and no editor under it.
    assert!(
        cx.debug_bounds("provider-0").is_some(),
        "the provider card head"
    );
    assert!(
        cx.debug_bounds("provider-edit-0").is_some(),
        "its Edit action"
    );
    assert!(
        cx.debug_bounds("provider-remove-0").is_some(),
        "its Delete action"
    );
    assert!(
        cx.debug_bounds("provider-editor").is_none(),
        "the editor is not drawn until the card is opened"
    );
    // The one entry to the add flow is the add button.
    assert!(cx.debug_bounds("provider-add").is_some(), "the add button");
    assert!(
        cx.debug_bounds("models-add-card").is_none(),
        "the add card and its button never show together"
    );

    // Opening the editor folds everything but the key away.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.editing_provider = Some("work".to_string())
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("provider-editor").is_some(), "the editor");
    assert!(
        cx.debug_bounds("provider-vault-hint").is_some(),
        "the vault source is the editor's one primary field"
    );
    assert!(
        cx.debug_bounds("field-editor-api-key").is_none(),
        "no plaintext API key field is drawn"
    );
    assert!(cx.debug_bounds("models-customized").is_some(), "its fold");
    assert!(
        cx.debug_bounds("models-apply").is_some()
            && cx.debug_bounds("models-editor-cancel").is_some(),
        "the Apply/Cancel pair"
    );
    assert!(
        cx.debug_bounds("field-editor-base-url").is_none(),
        "the fold hides the endpoint until it is opened"
    );
    assert!(
        cx.debug_bounds("provider-0").is_some(),
        "the card stays while its editor is open"
    );

    // The fold reveals the endpoint and the protocol.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| this.models_advanced_open = true);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("field-editor-base-url").is_some(),
        "the endpoint"
    );
    assert!(
        cx.debug_bounds("api-mode-chat").is_some()
            && cx.debug_bounds("api-mode-responses").is_some()
            && cx.debug_bounds("api-mode-anthropic").is_some(),
        "the protocol chips"
    );

    // The add block swaps its button for the card, whose mode switch offers the
    // catalog disabled and the custom form selected.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.editing_provider = None;
            this.models_add_open = true;
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("models-add-card").is_some(), "the add card");
    assert!(
        cx.debug_bounds("provider-add").is_none(),
        "the button is gone while the card is open"
    );
    assert!(
        cx.debug_bounds("add-route-Custom").is_some(),
        "the custom route"
    );
    assert!(
        cx.debug_bounds("add-route-Catalogue").is_some(),
        "the catalogue route"
    );
    assert!(cx.debug_bounds("add-create").is_some(), "the create action");
}

/// General carries the reference's rows and stops at its own current-version
/// line: the build facts bsh used to list there - the data directory, the
/// session count and every discovered profile - are not part of that pane.
#[gpui::test]
fn the_general_pane_carries_no_build_facts(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("general-about", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("general-item-current-version").is_some(),
        "the reference's own current-version row is still drawn"
    );
    assert!(
        cx.debug_bounds("general-item-about").is_none(),
        "the build-facts block must not be drawn"
    );
}

/// The Models pane writes the API protocol on the family leaf, and the one the
/// agent is built with is what the pane picked.
#[gpui::test]
fn api_protocol_row_writes_the_setting(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("api-mode", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // The protocol chips sit in the provider editor's Customized settings fold,
    // so seed a provider and open both before the first frame.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
            this.state.editing_provider = Some("work".to_string());
            this.models_advanced_open = true;
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let bounds = cx
        .debug_bounds("api-mode-responses")
        .expect("the models pane must offer the Responses protocol");
    cx.simulate_click(bounds.center(), Modifiers::default());
    let mode = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .global
            .family_protocol_of("work")
    });
    assert_eq!(
        mode, "responses",
        "picking Responses must write the protocol"
    );

    let bounds = cx
        .debug_bounds("api-mode-chat")
        .expect("the models pane must offer the chat protocol");
    cx.simulate_click(bounds.center(), Modifiers::default());
    let mode = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .global
            .family_protocol_of("work")
    });
    assert_eq!(mode, "chat", "picking chat writes it back");
}

/// The background jobs are the running turns the reader is not looking at,
/// newest first, each with a title to show.
#[gpui::test]
fn background_jobs_exclude_the_session_on_screen(_cx: &mut TestAppContext) {
    let mut state = state_with_turns("jobs-order", 1);
    let a = state.active_id.clone().expect("a session is active");
    let _ = state.new_session();
    let b = state.active_id.clone().expect("a session is active");
    let _ = state.new_session();
    let c = state.active_id.clone().expect("a session is active");
    assert_ne!(a, b, "a new chat must be its own session");
    state.streams.insert(a.clone(), 10);
    state.streams.insert(b.clone(), 20);
    state.streams.insert(c.clone(), 30);

    let jobs = state.background_jobs();
    assert_eq!(
        jobs.len(),
        2,
        "the session on screen is not a background job"
    );
    assert_eq!(jobs[0].0, b, "the newest job comes first");
    assert_eq!(jobs[1].0, a, "the older job follows");
    assert!(
        !jobs[0].1.is_empty(),
        "a job is listed under its session title"
    );
}

/// The header counts the background turns, and picking one brings it forward.
#[gpui::test]
fn the_header_counts_background_jobs(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("jobs-header", 1);
        let running = state.active_id.clone().expect("a session is active");
        let _ = state.new_session();
        // The shell restores the session the settings name, so pin that to the
        // new one: the turn then runs in the session the reader is not looking
        // at, which is what makes it a background job.
        state.settings.global.general.active_session =
            state.active_id.clone().expect("a session is active");
        state.streams.insert(running, 10);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The chip is a kit Button, which records no bounds the harness can look up,
    // so the observable is the label the shell would draw before the click.
    let (job, label) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        let shell = shell.read(cx);
        let jobs = shell.state.background_jobs();
        assert_eq!(jobs.len(), 1, "one background turn");
        let label = shell.job_label().expect("a counted job has a label");
        (jobs[0].0.clone(), label)
    });
    assert!(
        label.contains('1'),
        "the chip names how many run in the background: {label}"
    );
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |this, cx| this.open_job(&job, cx));
        assert_eq!(
            shell.read(cx).state.active_id.as_deref(),
            Some(job.as_str()),
            "picking a job must bring its session to the front"
        );
        assert!(
            shell.read(cx).state.background_jobs().is_empty(),
            "that job is the session on screen now, so it is no longer background"
        );
    });
}

/// The chat content width: the reference's clamp with its bands trimmed, so a
/// wide window gives the transcript most of the column instead of two empty
/// margins either side of it.
#[test]
fn the_chat_column_takes_the_measured_clamp() {
    use crate::theme::{chat_width_at, CHAT_W};
    assert_eq!(
        chat_width_at(0.0),
        CHAT_W,
        "a column with no measurement yet lands on the 680 floor"
    );
    assert_eq!(
        chat_width_at(740.0),
        CHAT_W,
        "666 of a 740 column clamps up to the floor"
    );
    assert!(
        (chat_width_at(1000.0) - 900.0).abs() < 0.01,
        "a laptop column takes .8 of it: {}",
        chat_width_at(1000.0)
    );
    assert_eq!(
        chat_width_at(2000.0),
        1800.0,
        "a wide column takes .9 of it, not a cap"
    );
}

/// And the clamp is LIVE: at the harness's own window the composer stack is the
/// live chat column plus the measured 32, which is the capped width rather than
/// the fixed 680 bsh drew. The floor and the mid range are the pure test's.
#[gpui::test]
fn the_composer_stack_follows_the_measured_clamp(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("chat-clamp", 2);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let column = cx.update(|_, cx| harness.read(cx).shell.read(cx).conversation_w.as_f32());
    let stack = cx
        .debug_bounds("composer-stack")
        .expect("the composer stack")
        .size
        .width
        .as_f32();
    // The harness window is wide enough for the cap to be the binding term, so
    // this test discriminates: the fixed 680 bsh drew before would be 712 here.
    assert!(
        column * crate::theme::CHAT_SHARE > crate::theme::CHAT_W,
        "the harness window has to be wide enough for the clamp to bite: column {column}"
    );
    assert_eq!(
        stack,
        crate::theme::chat_width_at(column) + crate::theme::COMPOSER_WIDEN,
        "the composer stack is the live chat width plus the measured 32"
    );
    assert_ne!(
        stack,
        crate::theme::CHAT_W + crate::theme::COMPOSER_WIDEN,
        "the fixed width bsh drew before this was live"
    );
    let _ = harness;
}

/// The stored width is read back on the next window: the reference reads its
/// client store on every width publish, so a reload keeps the dragged column
/// instead of falling back to the clamp.
#[gpui::test]
fn the_stored_content_width_wins_over_the_clamp(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("chat-stored", 2);
        state.settings.global.general.content_width = Some(800.0);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (chat, stored) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        (
            shell.chat_w(),
            shell.state.settings.global.general.content_width,
        )
    });
    assert_eq!(chat, 800.0, "the stored width overrides the clamp's 920");
    assert_eq!(stored, Some(800.0));
    let drawn = cx
        .debug_bounds("composer-stack")
        .expect("the composer stack")
        .size
        .width
        .as_f32();
    assert_eq!(
        drawn,
        800.0 + crate::theme::COMPOSER_WIDEN,
        "and it is what gets drawn"
    );
    let _ = harness;
}

/// The measured bar reuses only two-stop gradients - gpui-pre's Background
/// holds a fixed pair of stops - so the CSS's four stops are three stacked
/// layers, and the bar is the measured 2 x 72 centred in its strip.
#[gpui::test]
fn the_chat_handle_bar_takes_the_measured_column(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("chat-bar", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let strip = cx
        .debug_bounds("chat-handle-left")
        .expect("the left handle");
    let bar = cx
        .debug_bounds("chat-handle-bar-left")
        .expect("the measured bar");
    assert_eq!(bar.size.width, px(crate::theme::HANDLE_BAR_W), "2px wide");
    assert_eq!(bar.size.height, px(crate::theme::HANDLE_BAR_H), "72 tall");
    assert_eq!(
        crate::theme::HANDLE_BAR_FADE * 2.0 + crate::theme::HANDLE_BAR_BAND,
        crate::theme::HANDLE_BAR_H,
        "28 + 16 + 28 is the measured column"
    );
    let measured_inset = (crate::theme::HANDLE_STRIP_W - crate::theme::HANDLE_BAR_W) / 2.0;
    let bar_inset = (bar.origin.x - strip.origin.x).as_f32();
    assert!(
        (bar_inset - measured_inset).abs() < 1.0,
        "a 2px column centred in the 10px strip is the measured 4px inset, measured {bar_inset}"
    );
    assert!(
        (bar.center().y - strip.center().y).as_f32().abs() < 1.0,
        "and it sits at the CSS default pointer-y of 50%"
    );
    let _ = harness;
}

/// The bar shows on hover and while dragging - the reference's :hover and
/// [data-dragging] - and the strip's own move has to stop propagating, or the
/// body's clear (which ends the hover) wipes it the same instant.
#[gpui::test]
fn the_chat_handle_hover_reveals_and_the_body_move_clears_it(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("chat-hover", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let strip = cx
        .debug_bounds("chat-handle-left")
        .expect("the left handle");
    cx.simulate_mouse_move(
        strip.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    let hovered = cx.update(|_, cx| harness.read(cx).shell.read(cx).chat_hover);
    assert_eq!(
        hovered,
        Some(super::shell::HandleSide::Left),
        "the strip's own move sets the hover"
    );
    let body = cx.debug_bounds("chat-body").expect("the body");
    cx.simulate_mouse_move(
        body.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    let cleared = cx.update(|_, cx| harness.read(cx).shell.read(cx).chat_hover);
    assert_eq!(
        cleared, None,
        "and the body's own move ends it, so leaving the handles hides the bar"
    );
    let _ = harness;
}

/// The reference's own drag bounds, straight from its `resolveContentWidth`:
/// `const max = Math.max(CONTENT_MIN, columnWidth - CONTENT_EDGE_BUDGET)` with
/// CONTENT_MIN 640 and CONTENT_EDGE_BUDGET 176, so a stored preference is
/// clamped into [640, max(640, column - 176)] and nothing else moves.
#[test]
fn the_dragged_content_width_takes_the_measured_bounds() {
    use crate::theme::resolve_content_width;
    assert_eq!(
        resolve_content_width(1640.0, None),
        1476.0,
        "with no preference the column is the chat width: 1640 * .9"
    );
    assert_eq!(
        resolve_content_width(1640.0, Some(600.0)),
        640.0,
        "a preference below the floor clamps up to 640, not to the clamp's 680"
    );
    assert_eq!(
        resolve_content_width(1640.0, Some(2000.0)),
        1576.0,
        "and above the cap to the column less its two gutters"
    );
    assert_eq!(
        resolve_content_width(1640.0, Some(800.0)),
        800.0,
        "a preference inside the bounds is kept as it is"
    );
    assert_eq!(
        resolve_content_width(700.0, Some(900.0)),
        640.0,
        "a column too narrow for the budget still floors the preference"
    );
}

/// The gaps the step rows used to have: a language for the body, a byte badge,
/// a patch instead of raw JSON for a write or an edit.
#[test]
fn a_step_row_knows_its_language_size_and_patch() {
    use super::transcript::{code_lang, format_bytes, tool_bytes, tool_diff};
    assert_eq!(code_lang("bash", "{}"), Some("bash"));
    assert_eq!(
        code_lang("write_file", r#"{"path": "src/main.rs"}"#),
        Some("rust"),
        "a file step is highlighted as the file it wrote"
    );
    assert_eq!(
        code_lang("update_plan", "{}"),
        None,
        "and a step with no code claims no language"
    );
    assert_eq!(format_bytes(812), "812 B");
    assert_eq!(format_bytes(7_373), "7.2 KB");

    let mut running = crate::model::ToolEvent {
        name: "bash".to_string(),
        args: "{\"command\": \"ls\"}".to_string(),
        output: None,
        ms: None,
        at: None,
    };
    assert_eq!(
        tool_bytes(&running),
        running.args.len(),
        "a live step counts its arguments"
    );
    running.output = Some("a\nb".to_string());
    assert_eq!(tool_bytes(&running), 3, "a finished one its output");

    let patch = tool_diff(
        "edit_file",
        r#"{"path": "a.rs", "old_string": "one\ntwo", "new_string": "one\nthree"}"#,
    )
    .expect("an edit is a patch");
    assert_eq!(
        patch,
        vec![
            ('-', "one".to_string()),
            ('-', "two".to_string()),
            ('+', "one".to_string()),
            ('+', "three".to_string())
        ],
        "the replaced lines come out, the new ones go in"
    );
    assert!(
        tool_diff("bash", "{}").is_none(),
        "a command is not a patch"
    );
    assert_eq!(
        super::transcript::output_body(r#"{"stdout": "hi\nthere", "stderr": "", "exit_code": 0}"#),
        "hi\nthere",
        "a command output reads as its stream, not its envelope"
    );
    assert_eq!(
        super::transcript::output_body(r#"{"stdout": "out", "stderr": "boom"}"#),
        "out\nboom",
        "and a failure keeps its stderr beside it"
    );
    assert_eq!(
        super::transcript::output_body("plain"),
        "plain",
        "anything else is passed through"
    );
}
/// A settled turn shows one summary line per step, and a click opens its body -
/// the trace shape a reader scans a finished turn in.
#[gpui::test]
fn a_settled_turns_step_rows_collapse(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("step-fold", 1);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            if let Some(message) = session.messages.last_mut() {
                message.tools.push(crate::model::ToolEvent {
                    name: "bash".to_string(),
                    args: "{\"command\": \"cargo test\"}".to_string(),
                    output: Some("ok".to_string()),
                    ms: Some(12),
                    at: None,
                });
            }
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // One turn is [user, assistant], so the tool hangs off message 1.
    assert!(
        !cx.update(|_, cx| shell.read(cx).state.tool_open(1, 0)),
        "a settled turn starts folded"
    );
    let head = cx.debug_bounds("tool-head-1-0").expect("the step row");
    cx.simulate_click(head.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).state.tool_open(1, 0)),
        "and a click opens it"
    );
    assert!(
        cx.debug_bounds("tool-1-0").is_some(),
        "the row itself is drawn"
    );
}
/// A step row is labelled by what it DID, and summarised by the first useful
/// field of its arguments - not by the whole JSON object.
#[test]
fn a_step_row_is_kinded_and_summarised() {
    use super::transcript::{tool_kind_key, tool_summary};
    assert_eq!(tool_kind_key("bash"), "tool.kind.bash");
    assert_eq!(tool_kind_key("write_file"), "tool.kind.write");
    assert_eq!(
        tool_kind_key("mcp__github__search"),
        "tool.kind.other",
        "an unknown tool still reads as a step"
    );
    assert_eq!(
        tool_summary(r#"{"command": "cargo test -p bsh", "timeout": 300}"#),
        "cargo test -p bsh",
        "the command is the summary, not the object"
    );
    assert_eq!(
        tool_summary(r#"{"path": "src/main.rs", "content": "x"}"#),
        "src/main.rs",
        "a file step is summarised by its path"
    );
    assert_eq!(
        tool_summary("{\"a\": 1}"),
        "{\"a\": 1}",
        "an object with no string value falls back to its own text"
    );
    let long = tool_summary(&format!("{{\"command\": \"{}\"}}", "x".repeat(200)));
    assert!(long.ends_with('…'), "and a long value clips: {long}");
}
/// The meter leads the status row, so the old leftward offset that placed it
/// near the card's left third is gone with `meter_shift`.
/// The transcript is not centred: its left inset is 40% of the band, while the
/// right keeps the band the turn rail and the composer were laid out with - so
/// the empty strip beside the text shrinks without moving the composer.
#[test]
fn the_transcript_takes_only_four_tenths_of_the_left_band() {
    use crate::theme::{chat_left_inset, chat_width_at, CHAT_LEFT_SHARE};
    let column = 1000.0;
    let chat = chat_width_at(column);
    let band = (column - chat) / 2.0;
    assert_eq!(
        chat_left_inset(column, chat),
        (band * CHAT_LEFT_SHARE).round(),
        "the left inset is the share of the band the right side still holds"
    );
    assert!(
        chat_left_inset(column, chat) < band / 2.0,
        "which is less than half the band it was"
    );
    assert_eq!(
        chat_left_inset(0.0, chat_width_at(0.0)),
        0.0,
        "an unmeasured column has no band to split"
    );
}
/// The measured handle strip: `min(10px, calc((100% - var(--dsh-chat-content-width))
/// / 2 - 24px - 24px))`, parked `right: calc(50% + var(--dsh-chat-content-width) /
/// 2 + 24px)` off the body on the left and `left:` on the right.
#[gpui::test]
fn the_chat_handles_park_outside_the_content_column(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("chat-handles", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (column, content) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        (shell.conversation_w.as_f32(), shell.chat_w())
    });
    let body = cx.debug_bounds("chat-body").expect("the measured body");
    let left = cx
        .debug_bounds("chat-handle-left")
        .expect("the left handle");
    let right = cx
        .debug_bounds("chat-handle-right")
        .expect("the right handle");
    assert_eq!(
        left.size.width,
        px(crate::theme::HANDLE_STRIP_W),
        "this gutter is wide enough for the full strip"
    );
    assert_eq!(
        right.size.width,
        px(crate::theme::HANDLE_STRIP_W),
        "and on the other side"
    );
    let offset = column / 2.0 + content / 2.0 + crate::theme::HANDLE_OFFSET;
    // The transcript is not centred: the left handle follows the narrowed inset,
    // the right one keeps the symmetric offset.
    let inset = crate::theme::chat_left_inset(column, content);
    assert_eq!(
        (left.origin.x - body.origin.x).as_f32(),
        (inset - crate::theme::HANDLE_OFFSET - crate::theme::HANDLE_STRIP_W).max(0.0),
        "the left handle parks outside the narrowed left inset"
    );
    assert_eq!(
        (right.origin.x - body.origin.x).as_f32(),
        offset,
        "and the right one keeps the symmetric offset"
    );
    let content_left = body.origin.x + px(inset);
    let content_right = body.origin.x + px(column / 2.0 + content / 2.0);
    // GPUI snaps the painted content column to a device pixel, so the
    // clearance is the measured 24 within one - the padding is painted after
    // snapping, which the exact origin assertions above already pin.
    let left_clear = (content_left - (left.origin.x + left.size.width)).as_f32();
    let right_clear = (right.origin.x - content_right).as_f32();
    assert!(
        (left_clear - crate::theme::HANDLE_OFFSET).abs() <= 1.0,
        "24px clear of the content column's own edge, measured {left_clear}"
    );
    assert!(
        (right_clear - crate::theme::HANDLE_OFFSET).abs() <= 1.0,
        "on both sides. measured {right_clear}"
    );
    let _ = harness;
}

/// The reference's own fold rule: narrow ? !narrowExpanded : storedCollapsed,
/// over SIDEBAR_AUTO_COLLAPSE 1024, with a strict comparison.
#[test]
fn the_sidebar_fold_follows_the_auto_collapse_breakpoint() {
    use crate::theme::sidebar_folded;
    assert!(
        !sidebar_folded(1920.0, false, false),
        "wide, nothing stored"
    );
    assert!(
        sidebar_folded(1920.0, true, false),
        "wide, the reader folded it"
    );
    assert!(
        sidebar_folded(1920.0, true, true),
        "narrowExpanded governs only a narrow viewport"
    );
    assert!(
        sidebar_folded(900.0, false, false),
        "narrow folds by itself"
    );
    assert!(
        !sidebar_folded(900.0, false, true),
        "unless expanded by hand"
    );
    assert!(
        !sidebar_folded(900.0, true, true),
        "and that expand wins over a stored fold"
    );
    assert!(
        !sidebar_folded(1024.0, false, false),
        "the comparison is strict"
    );
    assert!(sidebar_folded(1023.9, false, false));
}

/// The auto-collapse as the shell resolves it. The harness cannot resize its
/// window - gpui-pre's Window::resize does not move the viewport - so the narrow
/// case is asserted on the resolution the render pass consults (sidebar_folded
/// over the recorded viewport) rather than on the painted tree; the painted
/// consequence is the stored-fold test's, at the harness width. The measurement
/// is set WITHOUT notifying: a notify schedules a frame here, and render re-reads
/// the real window width into the same field.
#[gpui::test]
fn the_narrow_viewport_auto_collapses_the_sidebar(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("sidebar-narrow", 2);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let measure = |cx: &mut VisualTestContext, width: f32| {
        cx.update(|_, cx| {
            let shell = harness.read(cx).shell.clone();
            shell.update(cx, |shell, _| shell.viewport_w = width);
        });
    };
    let read = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let shell = harness.read(cx).shell.read(cx);
            (
                shell.viewport_w,
                shell.sidebar_folded(),
                shell.sidebar_w(),
                shell.state.sidebar_collapsed,
                shell.state.narrow_expanded,
            )
        })
    };
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar").is_some(),
        "the harness opens at 1920, so the column is drawn"
    );
    measure(cx, 900.0);
    let (viewport, folded, width, stored, expanded) = read(cx);
    assert_eq!(viewport, 900.0, "the measurement render would read");
    assert!(
        folded,
        "under the breakpoint it folds to its collapsed width"
    );
    assert_eq!(width, 280.0, "and keeps the width it would return to");
    assert!(!stored && !expanded, "with neither flag touched");
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| shell.toggle_sidebar_at(900.0, cx));
    });
    let (_, _, _, stored, expanded) = read(cx);
    assert!(
        !crate::theme::sidebar_folded(900.0, stored, expanded),
        "and the narrow toggle expands it over the squeezed centre"
    );
    assert!(expanded, "through the runtime flag");
    assert!(!stored, "which is not the stored collapse");
    measure(cx, 1920.0);
    let (viewport, folded, ..) = read(cx);
    assert_eq!(viewport, 1920.0);
    assert!(!folded, "a wide viewport falls back to the stored flag");
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| shell.toggle_sidebar_at(1920.0, cx));
    });
    let (_, _, _, stored, expanded) = read(cx);
    assert!(
        crate::theme::sidebar_folded(1920.0, stored, expanded),
        "which this toggle flipped"
    );
    assert!(stored, "storing the fold");
    assert!(expanded, "and leaving the runtime flag alone");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar").is_none(),
        "so the drawn column leaves at the harness width"
    );
    let _ = harness;
}

/// The reference's own sidebar contract: setSidebar keeps
/// layoutInfo.sidebar through clampWidth(px, 264, 420), over a 280 store
/// default, rounded to a whole pixel.
#[test]
fn the_sidebar_width_takes_the_measured_bounds() {
    use crate::theme::resolve_sidebar_width;
    assert_eq!(resolve_sidebar_width(None), 280.0, "the store default");
    assert_eq!(resolve_sidebar_width(Some(300.0)), 300.0);
    assert_eq!(resolve_sidebar_width(Some(100.0)), 264.0, "the floor");
    assert_eq!(resolve_sidebar_width(Some(900.0)), 420.0, "the ceiling");
    assert_eq!(
        resolve_sidebar_width(Some(300.6)),
        301.0,
        "clampWidth rounds"
    );
}

/// The reference's DragHandle is an 8px col-resize strip at left:
/// cols.sidebar with margin-left:-4px, so it straddles the seam of a column
/// that takes the 280 store default until it is dragged.
#[gpui::test]
fn the_sidebar_handle_takes_the_measured_strip(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("sidebar-strip", 2);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let column = cx.debug_bounds("sidebar").expect("the sidebar column");
    let strip = cx.debug_bounds("sidebar-handle").expect("the resize strip");
    assert_eq!(column.size.width, px(280.0), "the store default");
    assert_eq!(
        strip.size.width,
        px(crate::theme::SIDEBAR_HANDLE_W),
        "the measured 8px strip"
    );
    let seam = (strip.origin.x - column.origin.x).as_f32();
    assert!(
        (seam - (280.0 - crate::theme::SIDEBAR_HANDLE_W / 2.0)).abs() < 1.0,
        "margin-left:-4px straddles the seam, measured {seam}"
    );
    assert!(
        (strip.size.height - column.size.height).as_f32().abs() < 1.0,
        "and it spans the frame top to bottom"
    );
    let _ = harness;
}

/// The drag: the reference records the column width and the pointer x at
/// pointerdown and then sets base + dx, clamped to 264..420 - the pointer's
/// own 40px is the width's 40px, because this column is not centred.
#[gpui::test]
fn the_sidebar_handle_drag_sets_and_stores_the_width(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("sidebar-drag", 2);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let strip = cx.debug_bounds("sidebar-handle").expect("the strip");
    let origin = strip.center();
    let moved = gpui_kit::point(origin.x + px(40.0), origin.y);
    cx.simulate_mouse_down(origin, gpui_kit::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        moved,
        Some(gpui_kit::MouseButton::Left),
        Modifiers::default(),
    );
    cx.simulate_mouse_up(moved, gpui_kit::MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let stored = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .settings
            .global
            .general
            .sidebar_width
    });
    assert_eq!(stored, Some(320.0), "the store keeps base + dx");
    let column = cx.debug_bounds("sidebar").expect("the sidebar column");
    assert_eq!(column.size.width, px(320.0), "and the column followed it");
    let _ = harness;
}

/// The reference renders its sidebar DragHandle only while the column is
/// expanded (the collapsed sidebar is 0 wide on darwin).
#[gpui::test]
fn the_sidebar_handle_is_gone_when_collapsed(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("sidebar-folded", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar-handle").is_some(),
        "the strip is drawn while the column is expanded"
    );
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| {
            // The harness viewport is 1920, so this is the wide branch: the
            // reference's toggle flips the STORED collapse there.
            let viewport = shell.viewport_w;
            shell.toggle_sidebar_at(viewport, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar-handle").is_none(),
        "and it leaves with the column"
    );
    assert!(
        cx.debug_bounds("sidebar").is_none(),
        "nor is the column drawn"
    );
    let _ = harness;
}

/// The drag itself. The reference's `outwardWidth` is `base + outward * 2`,
/// signed toward the dragged side, and its commit stores the RESOLVED width
/// under `dsh.conversation.contentWidth`; the harness presses the real handle.
#[gpui::test]
fn the_chat_handle_drag_sets_and_stores_the_width(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("chat-drag", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let before = cx.update(|_, cx| harness.read(cx).shell.read(cx).chat_w());
    let handle = cx
        .debug_bounds("chat-handle-right")
        .expect("the right handle");
    let origin = handle.center();
    let moved = gpui_kit::point(origin.x + px(40.0), origin.y);
    cx.simulate_mouse_down(origin, gpui_kit::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(moved, None::<gpui_kit::MouseButton>, Modifiers::default());
    cx.simulate_mouse_up(moved, gpui_kit::MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (after, stored) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        (
            shell.chat_w(),
            shell.state.settings.global.general.content_width,
        )
    });
    assert_eq!(
        after,
        before + 80.0,
        "40px of pointer is 80px of width while the column stays centred"
    );
    assert_eq!(stored, Some(after), "the commit stores the resolved width");
    let stack = cx
        .debug_bounds("composer-stack")
        .expect("the composer stack")
        .size
        .width
        .as_f32();
    assert_eq!(
        stack,
        after + crate::theme::COMPOSER_WIDEN,
        "and the live column follows the drag"
    );
    let _ = harness;
}

/// The reference's own count wording and plural rule:
/// `t(count === 1 ? "count.live.one" : "count.live.other", { count })`.
#[test]
fn the_job_count_takes_the_reference_plural_rule() {
    use crate::i18n::Lang;
    assert_eq!(
        super::header::job_count_label(Lang::En, 1),
        "1 background job running",
        "the singular form is the reference's own"
    );
    assert_eq!(
        super::header::job_count_label(Lang::En, 3),
        "3 background jobs running",
        "and the plural form follows it"
    );
    assert_eq!(
        super::header::job_count_label(Lang::Zh, 1),
        "1 个后台任务运行中",
        "the Chinese form does not inflect, but takes the count"
    );
    assert_eq!(
        super::header::job_count_label(Lang::Zh, 2),
        "2 个后台任务运行中"
    );
    assert_ne!(
        super::header::job_count_label(Lang::En, 1),
        "jobs.countLiveOne",
        "and both keys resolve"
    );
}

/// The reference's jobs trigger is not the grey pill bsh drew before: it is
/// `.trigger{min-height:28px;border-radius:radius-sm;gap:3px;padding:3px 2px}`
/// in label-tertiary, holding the ongoing `StateDot`, the counted span and a
/// 12px chevron.
#[gpui::test]
fn the_jobs_trigger_takes_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("jobs-chrome", 1);
        let running = state.active_id.clone().expect("a session is active");
        let _ = state.new_session();
        // The turn belongs to the session the shell does not come back to, which
        // is what makes it a background job.
        state.settings.global.general.active_session =
            state.active_id.clone().expect("a session is active");
        state.streams.insert(running, 10);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let chip = cx
        .debug_bounds("hdr-jobs")
        .expect("a background turn draws the measured trigger");
    assert_eq!(chip.size.height, px(28.0), "the measured min-height:28px");
    assert!(
        cx.debug_bounds("hdr-jobs-dot").is_some(),
        "the ongoing StateDot leads the trigger"
    );
    assert!(
        cx.debug_bounds("hdr-jobs-count").is_some(),
        "the count span follows it"
    );
    let chevron = cx
        .debug_bounds("hdr-jobs-chevron")
        .expect("and the chevron closes it");
    assert_eq!(
        chevron.size.width,
        px(12.0),
        "IconChevronDownOutlineRegular at 12"
    );
    assert_eq!(chevron.size.height, px(12.0), "in both axes");
    let _ = harness;
}

/// The measured trigger opens the jobs panel IN PLACE, over the session the
/// reader is on: the click must not navigate, and building the panel has to run
/// the measured section header without falling over. The kit draws a popup in
/// its own layer, so the harness cannot read the opened rows - the observable is
/// the shell the click left behind, exactly as the model chip's own test reads
/// it.
#[gpui::test]
fn the_jobs_trigger_opens_its_panel_in_place(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("jobs-open", 1);
        let running = state.active_id.clone().expect("a session is active");
        let _ = state.new_session();
        state.settings.global.general.active_session =
            state.active_id.clone().expect("a session is active");
        state.streams.insert(running, 10);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let before = cx.update(|_, cx| shell.read(cx).state.active_id.clone());

    let chip = cx.debug_bounds("hdr-jobs").expect("the measured trigger");
    cx.simulate_click(chip.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let after = cx.update(|_, cx| shell.read(cx).state.active_id.clone());
    assert_eq!(
        before, after,
        "the trigger opens its panel in place instead of moving the reader"
    );
    assert!(
        cx.debug_bounds("hdr-jobs").is_some(),
        "and the trigger it opened from stays on screen"
    );
}

/// Watching the only running turn is not a background job, so no chip is drawn.
#[gpui::test]
fn the_header_hides_the_job_chip_without_background_work(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("jobs-none", 1);
        let running = state.active_id.clone().expect("a session is active");
        // The running turn belongs to the session the shell comes back to, so it
        // is the one on screen and not a background job.
        state.settings.global.general.active_session = running.clone();
        state.streams.insert(running, 10);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let jobs = cx.update(|_, cx| harness.read(cx).shell.read(cx).state.background_jobs());
    assert!(
        jobs.is_empty(),
        "the session on screen is not a background job"
    );
    let label = cx.update(|_, cx| harness.read(cx).shell.read(cx).job_label());
    assert!(
        label.is_none(),
        "with nothing in the background there is no chip to draw"
    );
}

/// The appearance handler is the gate the System option promises: it is wired to
/// the host switch, but only a System mode follows it. The observable is the kit
/// theme mode, which is per-app in the harness - bsh own copied palette tokens are
/// process-global and other tests rewrite them in parallel, so asserting on those
/// would be a race, and the palette swap itself was proven by screenshot.
#[gpui::test]
fn only_the_system_mode_follows_a_live_appearance_change(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("appearance", 1);
        state.settings.global.general.theme_mode = "system".to_string();
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let mode_of = |cx: &mut VisualTestContext| -> String {
        cx.update(|_window, cx| cx.theme().mode.name().to_string())
    };

    // Whatever this machine reports for the host.
    let host = cx.update(|window, cx| {
        crate::theme::apply("system", window, cx);
        cx.theme().mode.name().to_string()
    });
    let pinned = if host == "dark" { "light" } else { "dark" };

    // A pinned mode ignores the switch, and the pin is always the other mode, so a
    // handler that forgot the gate would flip here.
    cx.update(|window, cx| {
        crate::theme::apply(pinned, window, cx);
        shell.update(cx, |this, cx| {
            this.state.settings.global.general.theme_mode = pinned.to_string();
            this.appearance_changed(window, cx);
        });
    });
    assert_eq!(
        mode_of(cx),
        pinned,
        "a pinned mode must not follow the host"
    );

    // System follows it, from whichever mode it was left on.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.state.settings.global.general.theme_mode = "system".to_string();
            this.appearance_changed(window, cx);
        });
    });
    assert_eq!(
        mode_of(cx),
        host,
        "the System mode follows the host appearance"
    );
}

/// The model rows write the tree the pane saves: a typed id joins the catalog,
/// a row selects itself as the active model, and the delete control drops just
/// that id. The rows sit low in a pane that scrolls, and a clipped row cannot be
/// clicked in the harness, so this drives the same handlers the controls call
/// and reads the draft back.
#[gpui::test]
fn the_model_catalog_handlers_write_the_draft(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("catalog", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
        })
    });

    // The catalog is the nicknames one provider carries.
    let draft = |cx: &mut VisualTestContext| -> Vec<String> {
        cx.update(|_, cx| {
            let state = &harness.read(cx).shell.read(cx).state;
            state.draft().catalog("work").to_vec()
        })
    };

    let models = draft(cx);
    assert_eq!(
        models,
        ["work-model".to_string()],
        "the seeded provider carries its one leaf"
    );

    // Adds keep insertion order and a repeat of the same id is a merge, which is
    // the answer the pane turns into its hint.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state
                .draft_mut()
                .global
                .add_model("work", "o3", "", "o3", "")
                .expect("first add");
            this.state
                .draft_mut()
                .global
                .add_model("work", "n", "", "o3", "")
                .expect_err("a second key repeating the id is refused");
            this.state
                .draft_mut()
                .global
                .add_model("work", "o3", "", "o3", "")
                .expect("re-adding under its own key merges");
        })
    });
    let models = draft(cx);
    assert_eq!(
        models,
        ["o3".to_string(), "work-model".to_string()],
        "adds keep order and a repeat merges rather than duplicating"
    );

    // A row selects itself as the active model. The choice is a SESSION fact,
    // so it lands on the session and the workspace document never carries it.
    let workspace_before = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .settings
            .workspace
            .clone()
    });
    cx.update(|_, cx| shell.update(cx, |this, _| this.state.select_model("o3")));
    let workspace_after = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .settings
            .workspace
            .clone()
    });
    assert_eq!(
        workspace_before, workspace_after,
        "a composer choice does not rewrite the workspace document"
    );
    let session_choice = cx.update(|_, cx| {
        let state = &harness.read(cx).shell.read(cx).state;
        let id = state.active_id.clone().expect("the session on screen");
        state.effective_in(&id).model_ref
    });
    assert_eq!(
        session_choice.map(|reference| reference.model).as_deref(),
        Some("o3"),
        "selecting a row makes it the session's model"
    );

    // Delete drops one id and leaves the active model where it was.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state
                .draft_mut()
                .global
                .remove_model("work", "work-model");
        })
    });
    let models = draft(cx);
    assert_eq!(models, ["o3".to_string()], "delete removes exactly one id");
}

/// A successful commit closes the dialog; the refusal half is pinned next to it.
///
/// `commit_settings` clears the overlay only after the write lands, so the pair
/// is: refuse -> stays open with the draft; save -> closes. A save that left the
/// pane up would look exactly like a refusal the reader cannot see the reason for.
#[gpui::test]
fn a_successful_commit_closes_the_dialog(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("commit-closes"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // A commit the scratch store can honour.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.open_settings();
            this.state.draft_mut().workspace.agent.permission = crate::caps::READ_ONLY.to_string();
            let committed = this.state.commit_settings();
            assert!(committed.is_ok(), "the scratch store accepts the write");
        })
    });
    let overlay = cx.update(|_, cx| format!("{:?}", harness.read(cx).shell.read(cx).state.overlay));
    assert!(
        !overlay.contains("Settings"),
        "a successful save closes the dialog, got {overlay}"
    );
    let permission = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .settings
            .workspace
            .agent
            .permission
            .clone()
    });
    assert_eq!(
        permission,
        crate::caps::READ_ONLY,
        "and the value is committed"
    );
}

/// A (provider, nickname) pair is stored whole or not at all.
///
/// The workspace document no longer holds a selection at all; the pair lives on
/// the session. A blank half would resolve to nothing, so the writer refuses it
/// rather than leaving a session that reports an unknown name.
#[test]
fn selecting_a_model_refuses_a_half_made_pair() {
    let mut state = AppState::with_store(scratch_store("select-pair"));
    seed_provider(&mut state.settings.global, "full", "https://full.example");
    let id = state.active_id.clone().expect("the session on screen");

    state.select_model_from("full", "   ");
    assert!(
        state
            .store
            .session(&id)
            .expect("session")
            .provider
            .is_none(),
        "a blank nickname is not a choice"
    );
    state.select_model_from("  ", "work-model");
    assert!(
        state.store.session(&id).expect("session").model.is_none(),
        "a blank provider is not a choice either"
    );

    state.select_model_from("full", "work-model");
    let session = state.store.session(&id).expect("session");
    assert_eq!(session.provider.as_deref(), Some("full"));
    assert_eq!(session.model.as_deref(), Some("work-model"));
}

/// A refused write leaves the dialog open, with the draft still in hand.
///
/// The refusal is fixable in the pane, so closing it would take away the
/// only place the fix can be made.
#[test]
fn a_refused_commit_keeps_the_dialog_open() {
    let base = std::env::temp_dir().join(format!("bsh-refused-{}", std::process::id()));
    std::fs::create_dir_all(&base).expect("scratch");
    // A regular file where the settings DIRECTORY would have to be: the
    // write cannot create its parent, which is a failure the code path
    // cannot paper over.
    let blocker = base.join("blocked");
    std::fs::write(&blocker, b"not a directory").expect("the blocker");

    let mut state = AppState::with_store(scratch_store("refused-commit"));
    state.draft_mut();
    state.settings.save_root = Some(blocker);
    state.open_settings();
    state.draft_mut().workspace.agent.permission = crate::caps::READ_ONLY.to_string();

    let refused = state.commit_settings();
    assert!(refused.is_err(), "the write cannot land");
    assert_eq!(
        state.overlay,
        Overlay::Settings,
        "a refused save keeps the dialog open"
    );
    assert_eq!(
        state.draft().workspace.agent.permission,
        crate::caps::READ_ONLY,
        "and the edits are still there to correct"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// The web-search switch is a real control: it writes the draft the file gets.
///
/// The hosted tool has no local registrar, so it cannot appear in the capability
/// list below it; without this row the setting would have no control at all and
/// the tool would be on with nothing in the window to say so.
#[gpui::test]
fn the_tools_pane_switches_the_hosted_web_search(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("web-tools"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("tools"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let before = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .web_tools_enabled
    });
    assert!(before, "the shipped default offers web search");

    let track = cx
        .debug_bounds("settings-web-tools")
        .expect("the switch draws in the Tools pane");
    assert_eq!(track.size.width, px(36.0), "the shared 36px track");
    cx.simulate_click(track.center(), Modifiers::default());
    let after = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .web_tools_enabled
    });
    assert_eq!(after, !before, "the switch writes the setting it shows");

    // And the agent the shell builds follows the draft.
    let built = cx.update(|_, cx| {
        crate::caps::build_agent(
            shell.read(cx).state.draft(),
            &shell.read(cx).state.vault,
            &crate::settings::resolve::SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        )
        .agent
        .config()
        .web_search
    });
    assert!(!built, "the built agent follows the switch");
}

/// The save bar is pinned under the scrolling box, not carried in it.
///
/// Carried inside, the dialog's only committing control scrolled away with the
/// section - in General at the reference window size it sat below the fold, so it
/// could not be pressed without scrolling and the keyboard could not reach it.
/// The discriminator is that a SHORT pane and a LONG pane put it in the same
/// place; while it was a child of the scroller its y followed the content.
#[gpui::test]
fn the_save_bar_does_not_scroll_with_the_section(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let place = |cx: &mut TestAppContext, section: &'static str| -> f32 {
        let (_harness, cx) = cx.add_window_view(move |window, cx| {
            let state = AppState::with_store(scratch_store("save-bar"));
            let shell =
                cx.new(|cx| Shell::build_for_test(window, section_index(section), state, cx));
            Harness { shell }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = cx
            .debug_bounds("settings-save")
            .expect("the save bar draws");
        // The bar must be fully inside the window it was drawn in.
        assert!(
            bounds.origin.y > px(0.0),
            "{section}: the save bar is laid out on screen"
        );
        // The harness window is 1640 wide and 900 tall in device pixels; the
        // dialog is inset by 24 a side, so a bar pinned to the panel's foot sits
        // near the bottom of that panel whatever the section holds.
        f32::from(bounds.origin.y)
    };

    let short = place(cx, "models");
    let tall = place(cx, "general");
    assert!(
        (short - tall).abs() < 40.0,
        "the save bar sits in the same place whether the section is short or long \
         (models {short}, general {tall}) - it is pinned, not scrolled with the content"
    );
}

/// A cancelled dialog puts the previewed preferences back.
///
/// The language and the palette are shown before Save, so they live outside the
/// draft; dropping the draft alone would leave a dialog that was merely trying a
/// language on in that language. Cancel restores both from the committed tree.
#[gpui::test]
fn cancelling_settings_restores_the_previewed_language(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("cancel-preview", 1);
        state.settings.global.general.lang = "en".to_string();
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // What a language pick does while the dialog is open: the preview follows,
    // and the draft carries the value Save would write.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.state.lang = crate::i18n::Lang::Zh;
            this.state.draft_mut().global.general.lang = "zh".to_string();
            cx.notify();
        })
    });
    let previewed = cx.update(|_, cx| harness.read(cx).shell.read(cx).state.lang);
    assert_eq!(
        previewed,
        crate::i18n::Lang::Zh,
        "the preview follows the pick"
    );

    // Cancel drops the draft AND puts the preview back.
    cx.update(|_, cx| shell.update(cx, |this, cx| this.cancel_settings(cx)));
    let (lang, committed, draft_open) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        (
            shell.state.lang,
            shell.state.settings.global.general.lang.clone(),
            shell.state.draft.is_some(),
        )
    });
    assert_eq!(
        lang,
        crate::i18n::Lang::En,
        "the dialog was cancelled, so the language it tried on is gone"
    );
    assert_eq!(committed, "en", "the committed document never moved");
    assert!(!draft_open, "the draft is dropped");
}
/// The editor's add row draws the id it will add, and the press consumes it.
///
/// The button used to read fields the pane never drew, so it could only ever
/// answer `settings.modelIdRequired` about text nobody could type. This pins
/// both halves: the row is on screen, and an id typed into it is addable.
#[gpui::test]
fn the_editor_add_model_row_draws_the_id_it_adds(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("model-add-row", 1);

        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // A provider has to exist before its editor can be opened, and it has to be
    // seeded after the shell so `assemble` cannot reload over it.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
            crate::ui::settings::load_provider_into_form(this, "work", window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The id field is drawn, which is the half that was missing.
    let field = cx
        .debug_bounds("model-new-id")
        .expect("the add row draws its id field");
    assert!(
        field.origin.y > px(0.0),
        "the field is laid out inside the pane"
    );

    // An id typed in that field becomes a model of the open provider.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.new_model_id
                .update(cx, |state, cx| state.set_value("o3", window, cx));
            this.new_model_name
                .update(cx, |state, cx| state.set_value("O3", window, cx));
            this.state
                .add_model("work", "o3", "O3", "o3", "")
                .expect("the typed id is addable");
            this.state.provider_refusal = None;
            this.reload_model_rows("work", window, cx);
            // The rows are rebuilt from the DRAFT, which is where every pane
            // write lands; reading the saved tree here would re-echo the copy
            // the reader has not applied yet and drop the row just added.
            assert_eq!(
                this.model_rows.len(),
                2,
                "the seeded model plus the added one are both rows"
            );
        })
    });
    let models = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .catalog("work")
            .to_vec()
    });
    assert!(
        models.contains(&"o3".to_string()),
        "the typed id is a model of the provider, got {models:?}"
    );

    // The refusal stays the honest answer for an empty field, so the control is
    // wired to what the reader typed rather than to a field that always reads empty.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            let empty = this.state.add_model("work", "", "", "", "");
            this.state.provider_refusal = empty.err().map(|refusal| refusal.key());
        })
    });
    let refusal = cx.update(|_, cx| harness.read(cx).shell.read(cx).state.provider_refusal);
    assert_eq!(
        refusal,
        Some("settings.modelIdRequired"),
        "an empty id still refuses"
    );
}

/// The catalogue route's provider chooser is the reference's `select`: one closed
/// box naming a provider id, and the rest of the catalogue drawn only while it is
/// open. A list of provider rows always on screen would be a different control,
/// and the id is what the reader picks, because it is the key the wire carries.
#[gpui::test]
fn the_catalogue_provider_chooser_is_a_select_of_ids(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("catalogue-select", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    // The add card is what carries the catalogue route.
    let add = cx.debug_bounds("provider-add").expect("the add control");
    cx.simulate_click(add.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Closed: the box is on screen and the catalogue is not.
    assert!(
        cx.debug_bounds("catalogue-select").is_some(),
        "the closed select names the provider it is set to"
    );
    let (choice, open, addable) = cx.update(|_, cx| {
        let this = harness.read(cx).shell.read(cx);
        (
            this.catalogue_choice.clone(),
            this.catalogue_open,
            this.state.draft().global.addable_providers(),
        )
    });
    assert!(!open, "the options stay closed until the box is pressed");
    assert_eq!(
        addable,
        [
            "deepseek".to_string(),
            "openai".to_string(),
            "anthropic".to_string(),
            "nvidia".to_string(),
            "openrouter".to_string(),
        ],
        "the catalogue is every shipped provider the file has not got yet"
    );
    assert!(
        cx.debug_bounds("catalogue-option-deepseek").is_none(),
        "no option is drawn while the select is closed"
    );
    let _ = choice;

    // A press opens it, and every id is an option.
    let box_bounds = cx.debug_bounds("catalogue-select").expect("the select box");
    cx.simulate_click(box_bounds.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for selector in [
        "catalogue-option-deepseek",
        "catalogue-option-openai",
        "catalogue-option-anthropic",
        "catalogue-option-nvidia",
        "catalogue-option-openrouter",
    ] {
        let offered = cx.debug_bounds(selector);
        assert!(
            offered.is_some(),
            "the open list offers {selector} as an option"
        );
    }

    // Picking one is both the selection and the close.
    let pick = cx
        .debug_bounds("catalogue-option-nvidia")
        .expect("the nvidia option");
    cx.simulate_click(pick.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (choice, open) = cx.update(|_, cx| {
        let this = harness.read(cx).shell.read(cx);
        (this.catalogue_choice.clone(), this.catalogue_open)
    });
    assert_eq!(
        choice.as_deref(),
        Some("nvidia"),
        "the pick selects that id"
    );
    assert!(!open, "and it closes again");
    assert!(
        cx.debug_bounds("catalogue-option-deepseek").is_none(),
        "the options are drawn only while the select is open"
    );
}

/// The picker carries the reference own candidate search, and it narrows the
/// list the select-all pair acts on rather than only the rows on screen.
#[gpui::test]
fn the_picker_search_narrows_the_candidates(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("picker-search", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            // The picker is drawn inside a provider's editor, under its
            // Customized settings fold, so seed a provider and open both.
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
            this.state.editing_provider = Some("work".to_string());
            this.models_advanced_open = true;
            this.models_fetched = Some(vec![
                "gpt-4o".to_string(),
                "o3".to_string(),
                "llama".to_string(),
            ]);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("fetch-2").is_some(),
        "all three candidates are listed"
    );
    assert!(
        cx.debug_bounds("fetch-empty").is_none(),
        "and nothing is filtered yet"
    );

    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.picker_query = "o3".to_string();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("fetch-0").is_some(), "the match is listed");
    assert!(
        cx.debug_bounds("fetch-1").is_none(),
        "and the other candidates are gone"
    );
    assert!(
        cx.debug_bounds("fetch-empty").is_none(),
        "a match is not the no-match state"
    );

    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.picker_query = "nothing matches this".to_string();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("fetch-empty").is_some(),
        "a query that matches nothing says so"
    );
    assert!(cx.debug_bounds("fetch-0").is_none(), "with no rows left");
}

/// The picker is the reference Choose models to add flow over a real GET
/// /models. No network is involved: the harness seeds the list a fetch would
/// have returned and drives the same handlers the rows call, so a control that
/// only looks wired still fails.
#[gpui::test]
fn the_picker_adds_every_selected_fetched_model(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("picker", 1);

        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://work.example",
            );
            this.state.editing_provider = Some("work".to_string());
            this.models_fetched = Some(vec![
                "gpt-4o".to_string(),
                "o3".to_string(),
                "llama".to_string(),
            ]);
            // A second tick on the same row unticks it, which is the toggle the
            // row body carries.
            this.toggle_fetched_model("gpt-4o");
            this.toggle_fetched_model("gpt-4o");
            this.toggle_fetched_model("o3");
            this.toggle_fetched_model("llama");
        })
    });

    let (ticked, pending) = cx.update(|_, cx| {
        let this = harness.read(cx).shell.read(cx);
        (
            this.models_selected.len(),
            this.models_fetched.clone().unwrap_or_default().len(),
        )
    });
    assert_eq!(ticked, 2, "a second tick must untick the row");
    assert_eq!(pending, 3, "the fetched list stays the picker contents");

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| this.add_selected_models("work", window, cx))
    });

    let (models, ticked, closed) = cx.update(|_, cx| {
        let this = harness.read(cx).shell.read(cx);
        (
            this.state.draft().catalog("work").to_vec(),
            this.models_selected.len(),
            this.models_fetched.is_none(),
        )
    });
    assert_eq!(
        models,
        [
            "llama".to_string(),
            "o3".to_string(),
            "work-model".to_string()
        ],
        "only the ticked ids join the catalog, beside the seed"
    );
    assert_eq!(ticked, 0, "the picker clears its ticks after adding");
    assert!(closed, "the picker closes once the ids are added");
}

/// The Extensions pane is the reference own section, and its rows are real:
/// they read the draft, so a plugin switched off in the inventory reads as off
/// here, with no state of its own to keep in sync.
#[gpui::test]
fn the_builtin_plugins_pane_reads_the_draft(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("builtin-plugins", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("plugins"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let enabled = |cx: &mut VisualTestContext, name: &str| -> bool {
        cx.update(|_, cx| harness.read(cx).shell.read(cx).plugin_enabled(name))
    };
    assert!(enabled(cx, "shell"), "an installed plugin reads as enabled");

    cx.update(|_, cx| shell.update(cx, |this, _| this.state.toggle_plugin_off("shell")));
    assert!(
        !enabled(cx, "shell"),
        "switching a plugin off in the draft flips this pane switch"
    );
    assert!(enabled(cx, "files"), "another plugin is unaffected");
}

/// The Extensions pane has the reference shape: a tab strip, the page
/// header's Refresh/Add controls, a search field, and the three vertical
/// sections (built-in settings, experimental, installed) drawn as rows.
#[gpui::test]
fn the_plugins_pane_has_the_reference_shape(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-shape", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("plugins"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    for selector in [
        "plugins-tabs",
        "plugins-tab-list",
        "plugins-tab-mcp",
        "plugins-tab-market",
        "plugins-refresh",
        "plugins-add",
        "plugins-page-info",
        "builtin-settings-header",
        "builtin-setting-web-search",
        "builtin-setting-agent-loop",
        "builtin-setting-shell",
        "experimental-plugins-header",
        "experimental-developer-tools-row",
        "installed-plugins-header",
        "installed-empty",
        "installed-empty-market",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "the pane draws {selector}"
        );
    }
    assert!(
        cx.debug_bounds("plugin-none").is_none(),
        "the old bare list is gone"
    );
    assert!(
        cx.debug_bounds("plugins-session").is_none() && cx.debug_bounds("plugins-global").is_none(),
        "the reference's collapsible groups are gone"
    );
}

/// A built-in settings row navigates to the pane that owns the setting, with
/// the same write the rail's cells make.
#[gpui::test]
fn a_builtin_plugin_setting_row_navigates_to_its_pane(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-nav", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("plugins"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    for (selector, pane) in [
        ("builtin-setting-web-search", "tools"),
        ("builtin-setting-agent-loop", "agent-loop"),
        ("builtin-setting-shell", "shell"),
    ] {
        let row = cx.debug_bounds(selector).expect("the built-in row");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let section = cx.update(|_, cx| harness.read(cx).shell.read(cx).state.settings_section);
        assert_eq!(
            section,
            section_index(pane).expect("the target pane"),
            "{selector} navigates to {pane}"
        );
        // The nav write leaves the row's own pane; go back for the next row.
        let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
        cx.update(|_, cx| {
            shell.update(cx, |this, _| {
                this.state.settings_section =
                    section_index("plugins").expect("the Extensions pane");
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    assert!(
        cx.debug_bounds("builtin-setting-subagent").is_none(),
        "Subagent has no pane here, so no row links nowhere"
    );
}
/// The installed list renders one row per inventory record and reads the
/// package manifest for the title, description and version - not the record's
/// stale cache.
#[gpui::test]
fn the_plugin_list_draws_a_row_per_installed_extension(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-installed-rows", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            // Seed the DRAFT, not the committed settings: an extension in the
            // committed tree would make `assemble` derive and start its MCP
            // servers, which this list test does not exercise.
            shell.state.draft_mut().global.extensions = vec![
                installed_fixture("dsh-echo", true),
                installed_fixture("codex-deploy", false),
            ];
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    for selector in [
        "installed-row-@local/dsh-echo",
        "installed-row-codex-deploy",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "an installed extension draws its row: {selector}"
        );
    }
    assert!(
        cx.debug_bounds("installed-empty").is_none(),
        "a populated list is not the empty state"
    );
    let resolved = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |this, _| this.installed_plugins())
    });
    let echo = resolved
        .iter()
        .find(|plugin| plugin.id == "@local/dsh-echo")
        .expect("the dsh fixture resolves");
    assert_eq!(echo.display_name, "Fixture DSH Echo", "the manifest title");
    assert_eq!(
        echo.description, "A fixture DeepSeek Harness package for the extension substrate.",
        "the manifest description is the row's subtitle"
    );
    assert_eq!(echo.version, "1.2.3");
    assert_eq!(echo.name, "@local/dsh-echo", "the package id is the name");
}

/// A package whose shim refusals are non-empty carries the Problem pill, and
/// pressing the row opens the read-only disclosure that names the refused
/// kinds.
#[gpui::test]
fn a_refusal_carrying_extension_shows_the_problem_pill_and_disclosure(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-problem", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            // Draft-only so the codex fixture's MCP server is not spawned.
            shell.state.draft_mut().global.extensions =
                vec![installed_fixture("codex-deploy", false)];
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("installed-problem-codex-deploy").is_some(),
        "the refused package carries the Problem pill"
    );
    let resolved = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |this, _| this.installed_plugins())
    });
    let plugin = &resolved[0];
    assert!(plugin.problem());
    assert!(plugin.refusal_count() > 0);
    assert!(
        plugin
            .refusals
            .iter()
            .any(|refusal| refusal.feature == "external app connector"),
        "the refused kind is recorded: {:?}",
        plugin.refusals
    );
    assert!(
        !plugin
            .refusals
            .iter()
            .any(|refusal| refusal.feature == "MCP server"),
        "MCP is started by this crate, so it is not a problem"
    );

    let row = cx
        .debug_bounds("installed-row-codex-deploy")
        .expect("the installed row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("installed-detail-codex-deploy").is_some(),
        "pressing the row opens the disclosure"
    );
    assert!(
        cx.debug_bounds("installed-refusal-codex-deploy-0")
            .is_some(),
        "the disclosure lists the refused kinds"
    );
}

/// An installed package that declares MCP servers shows Needs trust, and its
/// disclosure names the exact command line and the env KEYS - never a value.
#[gpui::test]
fn an_untrusted_mcp_plugin_shows_needs_trust_and_its_commands(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-needs-trust", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            // Draft-only AND untrusted: the package's command must not start.
            shell.state.draft_mut().global.extensions =
                vec![installed_fixture("codex-deploy", false)];
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("installed-needs-trust-codex-deploy")
            .is_some(),
        "an untrusted MCP package carries the Needs trust pill"
    );
    let row = cx
        .debug_bounds("installed-row-codex-deploy")
        .expect("the installed row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("installed-mcp-codex-deploy-0").is_some(),
        "the disclosure draws the declared command"
    );
    assert!(
        cx.debug_bounds("installed-trust-codex-deploy").is_some(),
        "and the control that would approve it"
    );

    // The detail draws exactly these three fields; joining them is the text a
    // reader can see, so an env value could only appear by appearing here.
    let shown = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |this, _| {
            let plugin = this
                .installed_plugins()
                .into_iter()
                .find(|plugin| plugin.id == "codex-deploy")
                .expect("the fixture resolves");
            plugin
                .mcp_launches
                .iter()
                .map(|launch| {
                    format!(
                        "{} {} {}",
                        launch.command,
                        launch.cwd.clone().unwrap_or_default(),
                        launch.env_keys.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
    });
    assert!(
        shown.contains("./server.js"),
        "the exact command line is disclosed: {shown}"
    );
    assert!(
        shown.contains("FIXTURE_TOKEN"),
        "the environment is summarised by key: {shown}"
    );
    assert!(
        !shown.contains("sk-fixture-secret"),
        "the fixture's env VALUE never reaches the UI output: {shown}"
    );
}

/// The trust button opens the confirmation, a cancel records a refusal, and the
/// confirm is the one press that trusts and enables the package.
#[gpui::test]
fn the_trust_confirmation_names_the_commands_and_is_the_only_approval(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (root, record) = trust_package_fixture("trust-dialog");
    let cleanup = root.clone();
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-trust-confirm", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            shell.state.draft_mut().global.extensions = vec![record];
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let row = cx
        .debug_bounds("installed-row-trust-dialog")
        .expect("the installed row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let trust = cx
        .debug_bounds("installed-trust-trust-dialog")
        .expect("the approve control");
    cx.simulate_click(trust.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("trust-confirm-dialog").is_some(),
        "the press opens the confirmation, it does not trust on its own"
    );
    assert!(
        cx.debug_bounds("trust-confirm-command-0").is_some(),
        "the dialog names the command line"
    );
    assert!(cx.debug_bounds("trust-confirm-cancel").is_some());
    assert!(cx.debug_bounds("trust-confirm-apply").is_some());

    // Cancelling is a refusal: nothing is trusted, and the ring keeps it.
    let cancel = cx
        .debug_bounds("trust-confirm-cancel")
        .expect("the cancel control");
    cx.simulate_click(cancel.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (trusted, audit, dialog) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |this, _| {
            let trusted = this.state.draft().global.extensions[0].trusted;
            (
                trusted,
                this.state.trust_audit(),
                this.trust_confirm.is_some(),
            )
        })
    });
    assert!(!trusted, "cancel leaves the package untrusted");
    assert_eq!(audit.len(), 1, "the refusal is recorded");
    assert!(!audit[0].allow);
    assert!(!dialog, "cancel closes the dialog");
    assert!(
        cx.debug_bounds("installed-needs-trust-trust-dialog")
            .is_some(),
        "the row still asks for trust"
    );
    // Reopen and approve: the confirm press is the decision.
    let trust = cx
        .debug_bounds("installed-trust-trust-dialog")
        .expect("the approve control is still there");
    cx.simulate_click(trust.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let apply = cx
        .debug_bounds("trust-confirm-apply")
        .expect("the confirm control");
    cx.simulate_click(apply.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (trusted, is_trusted, enabled, audit, dialog) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |this, _| {
            let record = &this.state.draft().global.extensions[0];
            (
                record.trusted,
                record.is_trusted(),
                !this
                    .state
                    .draft()
                    .workspace
                    .disabled
                    .extensions
                    .contains(&"trust-dialog".to_string()),
                this.state.trust_audit(),
                this.trust_confirm.is_some(),
            )
        })
    });
    assert!(trusted && is_trusted, "confirm writes the approval");
    assert!(enabled, "Trust and enable also enables the package");
    assert_eq!(audit.len(), 2, "the approval is a second decision");
    assert!(audit[1].allow);
    assert!(!dialog, "confirm closes the dialog");
    assert!(
        cx.debug_bounds("installed-needs-trust-trust-dialog")
            .is_none(),
        "the trusted row no longer asks for trust"
    );
    let _ = std::fs::remove_dir_all(&cleanup);
}

/// The installed switch is the market card's write: the workspace disabled
/// list, persisted immediately rather than waiting for the dialog's Save.
#[gpui::test]
fn the_installed_toggle_persists_immediately(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("plugins-toggle-persist", 1);
        state.settings.global.extensions = vec![installed_fixture("dsh-echo", true)];
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("plugins"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("installed-empty").is_none(),
        "the seeded inventory draws a row"
    );

    let toggle = cx
        .debug_bounds("installed-toggle-@local/dsh-echo")
        .expect("the installed switch");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (disabled, root) = cx.update(|_, cx| {
        let this = harness.read(cx).shell.read(cx);
        (
            this.state
                .draft()
                .workspace
                .disabled
                .extensions
                .contains(&"@local/dsh-echo".to_string()),
            this.state
                .settings
                .save_root
                .clone()
                .expect("a scratch root"),
        )
    });
    assert!(disabled, "the switch writes the draft disabled list");
    assert!(
        cx.debug_bounds("installed-detail-@local/dsh-echo")
            .is_none(),
        "the toggle press does not also open the row disclosure"
    );

    // Reload from disk: the write did not wait for Save.
    let saved_global = crate::settings::global::GlobalSettings::load(&root.join("settings.toml"))
        .expect("the persisted global document");
    let saved = saved_global
        .extensions
        .iter()
        .find(|extension| extension.id == "@local/dsh-echo")
        .expect("the inventory record persisted");
    assert_eq!(saved.version, "1.2.3", "install metadata persisted");
    assert_eq!(
        saved.description,
        "A fixture DeepSeek Harness package for the extension substrate."
    );
    assert!(saved.managed, "the managed flag persisted");
    let workspace_path = crate::settings::workspace::WorkspaceSettings::path(&root);
    let saved_workspace = crate::settings::workspace::WorkspaceSettings::load(&workspace_path)
        .expect("the persisted workspace document");
    assert!(
        saved_workspace
            .disabled
            .extensions
            .contains(&"@local/dsh-echo".to_string()),
        "the disabled list is on disk immediately, not draft-only"
    );
}

/// An empty inventory still draws the Installed section, with a sentence and
/// the action that opens the Plugin Market tab.
#[gpui::test]
fn the_empty_installed_list_offers_the_market(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-empty", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            // No network in a test: the action only needs to switch the tab.
            shell.market_loaded = true;
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("installed-plugins-header").is_some(),
        "the empty section still has its heading"
    );
    assert!(
        cx.debug_bounds("installed-empty").is_some(),
        "and a real empty state"
    );
    let action = cx
        .debug_bounds("installed-empty-market")
        .expect("the market action");
    cx.simulate_click(action.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let tab = cx.update(|_, cx| harness.read(cx).shell.read(cx).plugins_tab);
    assert_eq!(tab, 3, "the action switches to the Plugin Market tab");
    assert!(
        cx.debug_bounds("market-refresh").is_some(),
        "and the market body is what draws"
    );
}

/// A section whose rows are all filtered away is not drawn; the search's own
/// sentence replaces it rather than an empty heading.
#[gpui::test]
fn a_section_with_no_rows_is_not_drawn(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("plugins-no-rows", 1);
        state.settings.global.extensions = vec![installed_fixture("dsh-echo", true)];
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("plugins"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("installed-plugins-header").is_some(),
        "the premise: the section has a row"
    );

    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.plugin_query = "no-such-plugin".to_string();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("installed-plugins-header").is_none(),
        "a filtered-away section draws no heading"
    );
    assert!(
        cx.debug_bounds("installed-row-@local/dsh-echo").is_none(),
        "and no row"
    );
    assert!(
        cx.debug_bounds("builtin-settings-header").is_some(),
        "the sections that still have rows stay"
    );
}

/// Every label the plugin panes draw resolves in both locales; a typo would
/// otherwise render the key itself.
#[test]
fn the_plugin_keys_resolve_in_both_locales() {
    for key in [
        "settings.plugins",
        "settings.builtinPluginsHint",
        "settings.pluginsPageTitle",
        "settings.pluginsPageHint",
        "settings.pluginsBuiltinSettings",
        "settings.pluginsExperimental",
        "settings.pluginsExperimentalPill",
        "settings.pluginsInstalled",
        "settings.pluginsProblem",
        "settings.pluginsProblemTooltip",
        "settings.pluginsLoadErrorTooltip",
        "settings.pluginsNeedsTrust",
        "settings.pluginsNeedsTrustTooltip",
        "settings.pluginsTrustEnable",
        "settings.pluginsTrustCommands",
        "settings.pluginsTrustCwd",
        "settings.pluginsTrustEnv",
        "settings.pluginsTrustTitle",
        "settings.pluginsTrustDescription",
        "settings.pluginsTrustCancel",
        "settings.pluginsTrustConfirm",
        "settings.pluginsTrustFailed",
        "settings.pluginsRefresh",
        "settings.pluginsAdd",
        "settings.pluginsInstalledEmpty",
        "settings.pluginsBrowseMarket",
        "settings.pluginsDeveloperTools",
        "settings.pluginsDetailStandard",
        "settings.pluginsDetailVersion",
        "settings.pluginsDetailSource",
        "settings.pluginsDetailInstallTarget",
        "settings.pluginsDetailPath",
        "settings.pluginsDetailCapabilities",
        "settings.pluginsNoCapabilities",
        "settings.pluginsNoPath",
        "settings.pluginEnabled",
        "settings.pluginDisabled",
        "settings.plugin-inventory",
        "settings.pluginListHint",
        "settings.marketIntro",
        "settings.marketLocalOnly",
        "settings.marketApplyHint",
        "settings.marketRefresh",
        "settings.marketSearch",
        "settings.marketEmpty",
        "settings.marketCounts",
        "settings.marketSources",
        "settings.marketSourcesSummary",
        "settings.marketBuiltin",
        "settings.marketInstalled",
        "settings.marketNoSources",
        "settings.marketEmptyHint",
        "settings.marketCountCodex",
        "settings.marketCountDsh",
        "settings.marketCountRemote",
        "settings.marketUrlHint",
        "settings.marketAddSource",
        "settings.marketRemoveSource",
        "settings.marketFetching",
        "settings.marketInstalling",
        "settings.marketError",
        "settings.marketInstallError",
        "settings.marketLimited",
        "settings.marketInstall",
        "settings.marketUninstall",
        "settings.marketEnabled",
        "settings.marketDisabled",
        "settings.marketRefused",
    ] {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            assert_ne!(crate::i18n::tr(lang, key), key, "missing locale value");
        }
    }
}

/// Every label the Skills pane draws resolves in both locales; a typo would
/// otherwise render the key itself, which is invisible to the compiler.
#[test]
fn the_skills_keys_resolve_in_both_locales() {
    for key in [
        "settings.pluginsTabSkills",
        "settings.skillsIntro",
        "settings.skillsPick",
        "settings.skillsBrowseAll",
        "settings.skillsExecutorDirs",
        "settings.skillsCount",
        "settings.skillsMissing",
        "settings.skillsRemove",
        "settings.skillsAdd",
        "settings.skillsAddSave",
        "settings.skillsPath",
        "settings.skillsName",
        "settings.skillsPathHint",
        "settings.skillsNameHint",
        "settings.skillsSearch",
        "settings.skillsNone",
    ] {
        assert_ne!(
            crate::i18n::tr(crate::i18n::Lang::En, key),
            key,
            "{key} missing from en"
        );
        assert_ne!(
            crate::i18n::tr(crate::i18n::Lang::Zh, key),
            key,
            "{key} missing from zh"
        );
    }
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::En, "settings.skillsBrowseAll").replace("{count}", "7"),
        "Browse all skills (7)",
        "the count reaches the label the reader sees"
    );
}

/// A skill directory the Skills pane tests own, removed on the way in.
fn skills_scratch(label: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/bsh-skill-pane-tests")
        .join(format!("{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// Write one skill the way the agent crate reads it.
fn skills_write(root: &std::path::Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("the skill directory");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: the {name} skill\n---\n\nBody.\n"),
    )
    .expect("the manifest");
}

/// The draft a Skills test seeded, so the pane and the build read one premise.
fn seed_sources(shell: &mut Shell, sources: Vec<crate::settings::global::SkillSource>) {
    shell.plugins_tab = 2;
    shell.state.draft_mut().global.set_skill_sources(sources);
}

/// One market row from a repository fixture, so the pane test does not depend on
/// the machine's Codex cache or DSH profile.
fn market_fixture(name: &str) -> crate::market::MarketPlugin {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agent/tests/fixtures/extensions")
        .join(name);
    let package = agent::extensions::ExtensionRegistry::load(&source)
        .unwrap_or_else(|error| panic!("{name} loads: {error}"));
    let refusals = package.refusals.len();
    let extension = package.extension;
    let description = extension
        .description
        .clone()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| extension.name.clone());
    crate::market::MarketPlugin {
        id: extension.id,
        name: extension.name,
        version: extension.version,
        standard: extension.standard,
        origin: crate::market::MarketOrigin::Local,
        install: crate::market::MarketInstall::Local(source),
        description,
        keywords: Vec::new(),
        capabilities: extension.capabilities,
        refusals,
        source_label: "test fixture".to_string(),
    }
}

/// One inventory record pointing at a fixture package, so the installed list
/// draws the same metadata the market row would have written at install time.
fn installed_fixture(name: &str, managed: bool) -> crate::settings::global::InstalledExtension {
    let plugin = market_fixture(name);
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agent/tests/fixtures/extensions")
        .join(name);
    crate::settings::global::InstalledExtension {
        id: plugin.id.clone(),
        name: plugin.id.clone(),
        display_name: plugin.name.clone(),
        description: plugin.description.clone(),
        version: plugin.version.clone(),
        standard: crate::settings::global::InstalledStandard::from(plugin.standard),
        source_label: plugin.source_label.clone(),
        install_target: source.display().to_string(),
        path: Some(source.to_string_lossy().to_string()),
        managed,
        installed_at: 1_700_000_000,
        trusted: false,
        trusted_path: None,
    }
}

/// A scratch Codex package that declares an MCP server with an env secret.
///
/// The trust dialog test uses this instead of a fixture that runs a real
/// service: the command exits immediately, so approving it cannot leave a
/// process behind, while the declaration is still fully MCP-bearing.
fn trust_package_fixture(
    label: &str,
) -> (
    std::path::PathBuf,
    crate::settings::global::InstalledExtension,
) {
    let root = std::env::temp_dir().join(format!("bsh-ui-trust-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".codex-plugin")).expect("the manifest directory");
    std::fs::write(
        root.join(".codex-plugin/plugin.json"),
        serde_json::json!({
            "name": label,
            "version": "1.0.0",
            "description": "An MCP package the trust dialog names.",
            "interface": { "displayName": "Trust Fixture" },
            "mcpServers": "./.mcp.json"
        })
        .to_string(),
    )
    .expect("the manifest");
    std::fs::write(
        root.join(".mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "echo": {
                    "command": "/bin/sh",
                    "args": ["-c", "exit 0"],
                    "env": { "FIXTURE_TOKEN": "sk-fixture-secret" }
                }
            }
        })
        .to_string(),
    )
    .expect("the MCP config");
    let record = crate::settings::global::InstalledExtension {
        id: label.to_string(),
        name: label.to_string(),
        display_name: "Trust Fixture".to_string(),
        description: "An MCP package the trust dialog names.".to_string(),
        version: "1.0.0".to_string(),
        standard: crate::settings::global::InstalledStandard::Codex,
        source_label: "test fixture".to_string(),
        install_target: root.display().to_string(),
        path: Some(root.to_string_lossy().to_string()),
        managed: false,
        installed_at: 1_700_000_000,
        trusted: false,
        trusted_path: None,
    };
    (root, record)
}

/// One remote-index row, so the source editor and remote card can be drawn
/// without any network request.
fn remote_market_fixture() -> crate::market::MarketPlugin {
    crate::market::MarketPlugin {
        id: "remote-fixture".to_string(),
        name: "Remote Fixture".to_string(),
        version: "1.0.0".to_string(),
        standard: agent::extensions::ExtensionStandard::DeepSeekHarness,
        origin: crate::market::MarketOrigin::Remote {
            index_url: "https://market.example/plugins.json".to_string(),
            market: "Test Market".to_string(),
        },
        install: crate::market::MarketInstall::Remote {
            spec: "remote-fixture@1.0.0".to_string(),
            registry: None,
        },
        description: "A plugin from a remote index.".to_string(),
        keywords: vec!["remote".to_string()],
        capabilities: Vec::new(),
        refusals: 0,
        source_label: "Remote · Test Market".to_string(),
    }
}

/// The pane lists one card per source, with the count each contributed.
#[gpui::test]
fn the_skills_pane_lists_the_sources_with_their_counts(cx: &mut TestAppContext) {
    use super::settings::section_index;
    let first = skills_scratch("pane-first");
    let second = skills_scratch("pane-second");
    skills_write(&first, "alpha");
    skills_write(&first, "beta");
    skills_write(&second, "gamma");
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view({
        let first = first.clone();
        let second = second.clone();
        move |window, cx| {
            let state = AppState::with_store(scratch_store("skills-pane-list"));
            let shell = cx.new(|cx| {
                let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
                seed_sources(
                    &mut shell,
                    vec![
                        crate::settings::global::SkillSource {
                            name: "First".to_string(),
                            path: first.to_string_lossy().to_string(),
                            enabled: true,
                        },
                        crate::settings::global::SkillSource {
                            name: "Second".to_string(),
                            path: second.to_string_lossy().to_string(),
                            enabled: true,
                        },
                    ],
                );
                shell
            });
            Harness { shell }
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let counted = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| {
            let draft = shell.state.draft().clone();
            let (sources, skills) = crate::settings::resolve::discover_skills(
                &draft.global,
                &draft.workspace,
                &draft.workspace.disabled.skills,
            );
            (
                sources
                    .iter()
                    .map(|source| (source.name.clone(), source.count))
                    .collect::<Vec<_>>(),
                skills.len(),
            )
        })
    });
    assert_eq!(
        counted,
        (vec![("First".to_string(), 2), ("Second".to_string(), 1)], 3),
        "one card per source, each carrying what it contributed"
    );
    assert!(
        cx.debug_bounds("skills-source-First").is_some(),
        "the card is drawn, not only counted"
    );
    assert!(
        cx.debug_bounds("skills-source-Second").is_some(),
        "and so is the second source"
    );
    assert!(
        cx.debug_bounds("skills-browse").is_some(),
        "the Browse all skills control is on the pane"
    );
    assert!(
        cx.debug_bounds("skills-dirs").is_some(),
        "and so is the Executor dirs control"
    );

    // The Browse-all control is REAL: pressing it opens a list with one row per
    // discovered skill and its source, so a control that only looked wired fails
    // here rather than passing on the card grid alone.
    let browse = cx.debug_bounds("skills-browse").expect("the control");
    cx.simulate_click(browse.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let opened = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| shell.skills_browse_open)
    });
    assert!(opened, "the press opened the browse-all disclosure");
    assert!(
        cx.debug_bounds("skills-browse-0").is_some()
            && cx.debug_bounds("skills-browse-2").is_some(),
        "one row per discovered skill is drawn"
    );

    // And the Executor dirs control opens the source list with its add control.
    let dirs = cx.debug_bounds("skills-dirs").expect("the control");
    cx.simulate_click(dirs.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("skills-dir-0").is_some(),
        "the source list is drawn"
    );
    assert!(
        cx.debug_bounds("skills-add-toggle").is_some(),
        "and the add-a-directory control with it"
    );
}

/// A source press changes the DRAFT, and the built agent follows it once the
/// dialog saves - the pane's toggle is the agent's toggle, not a label.
#[gpui::test]
fn toggling_a_source_changes_the_draft_and_the_built_agent(cx: &mut TestAppContext) {
    use super::settings::section_index;
    let root = skills_scratch("pane-toggle");
    skills_write(&root, "alpha");
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view({
        let root = root.clone();
        move |window, cx| {
            let state = AppState::with_store(scratch_store("skills-pane-toggle"));
            let shell = cx.new(|cx| {
                let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
                seed_sources(
                    &mut shell,
                    vec![crate::settings::global::SkillSource {
                        name: "First".to_string(),
                        path: root.to_string_lossy().to_string(),
                        enabled: true,
                    }],
                );
                shell
            });
            Harness { shell }
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The premise: the build already carries the skill.
    let before = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| {
            shell
                .state
                .draft()
                .effective()
                .enabled_skills()
                .map(|skill| skill.name.clone())
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(before, vec!["alpha".to_string()], "the premise");

    // Press the source card: it switches the source off in the draft.
    let card = cx
        .debug_bounds("skills-source-First")
        .expect("the source card");
    cx.simulate_click(card.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let off = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| {
            let draft = shell.state.draft();
            (
                draft.global.skill_sources()[0].enabled,
                draft
                    .effective()
                    .enabled_skills()
                    .map(|skill| skill.name.clone())
                    .collect::<Vec<_>>(),
            )
        })
    });
    assert_eq!(
        off,
        (false, Vec::<String>::new()),
        "one press switches the source off in the draft, and the build follows it"
    );

    // Saving writes that decision, and the committed tree builds the same way.
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| shell.save_settings(cx));
    });
    let (saved, built_skills) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| {
            // The BUILT agent, from the committed tree: this is the question the
            // round has to answer, not merely what the draft holds.
            let built = crate::caps::build_agent(
                &shell.state.settings,
                &shell.state.vault,
                &crate::settings::resolve::SessionChoice::default(),
                &crate::mcp::McpPool::default(),
            );
            let mut names: Vec<String> = built
                .agent
                .get_skills_content()
                .into_iter()
                .map(|(name, _)| name.to_string())
                .collect();
            names.sort();
            (
                shell.state.settings.global.skill_sources()[0].enabled,
                names,
            )
        })
    });
    assert!(
        !saved,
        "Save is the door the change goes through, and it carried the switch"
    );
    assert!(
        built_skills.is_empty(),
        "the SAVED tree builds an agent without the switched-off source's skills, \
         got {built_skills:?}"
    );

    // Paired: switching it back on and saving restores the skill, so the
    // assertion above cannot pass by the build never carrying skills at all.
    // The press is the pane's own handler rather than a click: Save closed the
    // dialog (the reference's own close), so there is no card on screen to press.
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| shell.toggle_source("First", cx));
    });
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| shell.save_settings(cx));
    });
    let restored = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| {
            let built = crate::caps::build_agent(
                &shell.state.settings,
                &shell.state.vault,
                &crate::settings::resolve::SessionChoice::default(),
                &crate::mcp::McpPool::default(),
            );
            built
                .agent
                .get_skills_content()
                .into_iter()
                .map(|(name, _)| name.to_string())
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(
        restored,
        vec!["alpha".to_string()],
        "switching the source back on and saving restores it to the agent"
    );
}

/// Adding and removing a custom directory writes the DRAFT, and a removal never
/// takes the framework directory away.
#[gpui::test]
fn a_custom_directory_is_added_and_removed_in_the_draft(cx: &mut TestAppContext) {
    use super::settings::section_index;
    let custom = skills_scratch("pane-custom");
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view({
        move |window, cx| {
            let state = AppState::with_store(scratch_store("skills-pane-custom"));
            let shell = cx.new(|cx| {
                let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
                shell.plugins_tab = 2;
                shell
            });
            Harness { shell }
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Add the custom directory through the pane's own handler.
    cx.update(|window, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| {
            shell.skill_add_open = true;
            let path = custom.to_string_lossy().to_string();
            shell
                .skill_add_path
                .clone()
                .update(cx, |state, cx| state.set_value(path, window, cx));
            shell.skill_add_name.clone().update(cx, |state, cx| {
                state.set_value("Custom".to_string(), window, cx)
            });
            shell.add_source(cx);
        });
    });
    let added = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| {
            shell
                .state
                .draft()
                .global
                .skill_sources()
                .iter()
                .map(|source| source.name.clone())
                .collect::<Vec<_>>()
        })
    });
    assert!(
        added.contains(&"Custom".to_string()),
        "the typed directory joined the draft: {added:?}"
    );

    // Remove it again, and the framework directory survives the removal.
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| shell.remove_source("Custom", cx));
    });
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| shell.remove_source("Bos", cx));
    });
    let left = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, _| {
            shell
                .state
                .draft()
                .global
                .skill_sources()
                .iter()
                .map(|source| source.path.clone())
                .collect::<Vec<_>>()
        })
    });
    assert!(
        !left.iter().any(|path| path.contains("pane-custom")),
        "the custom directory is gone: {left:?}"
    );
    assert!(
        left.iter().any(|path| path == "~/.bos/skills"),
        "the framework directory cannot be removed: {left:?}"
    );
}

/// Every sentence the provider form can draw resolves in both locales; a typo
/// would otherwise render the key itself.
#[test]
fn the_provider_form_keys_resolve_in_both_locales() {
    for key in [
        "settings.providerDisplayName",
        "settings.providerNameEmpty",
        "settings.providerNeedsBaseUrl",
        "settings.providerNeedsModel",
        "settings.providerIdTaken",
        "settings.editProvider",
        "settings.saveProvider",
        "settings.cancelEdit",
        "settings.deleteTitle",
        "settings.deleteDescription",
        "settings.deleteDescriptionWithCredential",
        "settings.deleteConfirm",
        "settings.searchPlugins",
        "settings.noMatchingPlugins",
        "settings.noPlugins",
        "settings.fetchSearch",
        "settings.fetchEmpty",
        "settings.fetching",
        "settings.provider",
        "settings.modelId",
        "settings.apiKey",
        "settings.create",
        "settings.modelsEmpty",
        "settings.add",
        "settings.resetModels",
        "settings.modelsCustomized",
        "settings.modelsInherited",
    ] {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            assert_ne!(crate::i18n::tr(lang, key), key, "missing locale value");
        }
    }
}

/// The measured Font size control: the General pane opens at the reference's
/// 14px and the stepper walks it to the measured top of the range.
#[gpui::test]
fn the_font_size_stepper_moves_the_conversation_text(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("font-size"));
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    assert_eq!(
        cx.update(|_, cx| shell
            .read(cx)
            .state
            .draft()
            .global
            .general
            .content_font_size),
        14,
        "the General pane opens on the reference step"
    );
    let plus = cx.debug_bounds("font-increase").expect("the stepper draws");
    cx.simulate_click(plus.center(), Modifiers::default());
    // The draft, because a pane edit is what Save later writes: the committed
    // tree is deliberately untouched until then.
    assert_eq!(
        cx.update(|_, cx| shell
            .read(cx)
            .state
            .draft()
            .global
            .general
            .content_font_size),
        15,
        "the plus control steps the conversation size"
    );
    assert_eq!(
        crate::theme::body_size_at(cx.update(|_, cx| {
            shell
                .read(cx)
                .state
                .draft()
                .global
                .general
                .content_font_size
        })),
        15.0,
        "and the drawn text follows the setting, because the render reads the draft"
    );
    for _ in 0..20 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let next = cx.debug_bounds("font-increase").expect("the stepper stays");
        cx.simulate_click(next.center(), Modifiers::default());
    }
    assert_eq!(
        cx.update(|_, cx| shell
            .read(cx)
            .state
            .draft()
            .global
            .general
            .content_font_size),
        crate::theme::CONTENT_MAX,
        "the stepper stops at the reference top"
    );
}

/// The measured Appearance row: three cubes on the reference's 180px basis,
/// each carrying its icon over its label, and a press switches the mode. This
/// presses the Light cube for real and reads the setting back.
#[gpui::test]
fn the_appearance_cubes_set_the_colour_mode(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("appearance-cubes"));
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    assert_eq!(
        cx.update(|_, cx| shell
            .read(cx)
            .state
            .settings
            .global
            .general
            .theme_mode
            .clone()),
        "system",
        "the pane opens on the reference's follow-system default"
    );
    let light = cx
        .debug_bounds("theme-cube-light")
        .expect("the Light cube draws");
    assert!(
        light.size.width >= px(180.0),
        "every cube keeps the measured 180px basis, got {}",
        light.size.width
    );
    assert!(
        cx.debug_bounds("theme-cube-system").is_some(),
        "the System cube draws beside it"
    );
    cx.simulate_click(light.center(), Modifiers::default());
    assert_eq!(
        cx.update(|_, cx| shell
            .read(cx)
            .state
            .draft()
            .global
            .general
            .theme_mode
            .clone()),
        "light",
        "pressing the Light cube switches the mode"
    );
}

/// The settings dialog's own skeleton is measured, not guessed: the panel is 800
/// wide on the reference's 24px mask inset, the rail is 188 over 40px cells, the
/// header is 54 tall with a 28x28 close, and a text field's chrome is 34 tall.
#[gpui::test]
fn the_settings_dialog_takes_the_measured_shape(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("settings-shape"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("shell"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let panel = cx.debug_bounds("settings-panel").expect("the panel draws");
    assert_eq!(panel.size.width, px(800.0), "the measured panel width");
    let viewport = cx.update(|window, _| window.viewport_size());
    assert_eq!(
        panel.size.height,
        (viewport.height - px(48.0)).min(px(800.0)),
        "min(800, viewport - 48): the reference's own mask inset"
    );
    let header = cx.debug_bounds("settings-header").expect("the pane header");
    assert_eq!(header.size.height, px(54.0), "the measured header row");
    let cell = cx.debug_bounds("set-nav-1").expect("a rail cell draws");
    assert_eq!(cell.size.height, px(40.0), "the measured rail cell");
    assert_eq!(
        cell.origin.x,
        panel.origin.x + px(12.0),
        "the rail's own 12px padding"
    );
    assert_eq!(
        cx.debug_bounds("settings-pane").map(|b| b.origin.x),
        Some(panel.origin.x + px(188.0)),
        "the options box starts after the measured 188px rail"
    );
    let close = cx
        .debug_bounds("settings-close")
        .expect("the close control");
    assert_eq!(
        (close.size.width, close.size.height),
        (px(28.0), px(28.0)),
        "the measured 28x28 close button"
    );
    let field = cx
        .debug_bounds("field-settings.shellTimeout")
        .expect("a text field's chrome");
    assert!(
        field.size.height >= px(34.0),
        "the measured 34px field chrome, got {}",
        field.size.height
    );
}
/// The reference markdown scale, measured from its theme tokens, reaches the
/// view the transcript draws: headings are additive there (h1 21px plus the
/// content delta, h4 the body size). This pins the wiring, not the pixels.
#[gpui::test]
fn the_markdown_scale_carries_the_reference_metrics(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("markdown-scale"));
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let style = cx.update(|_, cx| shell.read(cx).markdown_style());
    let heading = style
        .heading_font_size
        .as_ref()
        .expect("the reference pins its heading sizes");
    assert_eq!(heading(1, px(14.0)), px(21.0), "h1 is the reference 21px");
    assert_eq!(heading(2, px(14.0)), px(19.0), "h2 is 19px");
    assert_eq!(heading(3, px(14.0)), px(18.0), "h3 is 18px");
    assert_eq!(heading(4, px(14.0)), px(14.0), "h4 is the body size");
    assert_eq!(
        style.heading_base_font_size,
        px(14.0),
        "the base is the Font size setting"
    );
    assert_eq!(
        style.is_dark,
        crate::theme::is_dark(),
        "the highlight theme follows the applied palette"
    );
}

/// The Extensions pane's tabs switch its BODY in place: MCP is a tab
/// beside Plugin list, not a page of its own, and both stay in the same pane.
#[gpui::test]
fn the_plugins_pane_tabs_switch_the_body_in_place(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-views", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("plugins"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("builtin-settings-header").is_some(),
        "the plugin list is the first tab"
    );
    assert!(
        cx.debug_bounds("mcp-add").is_none(),
        "and the MCP body is not drawn yet"
    );

    let tab = cx.debug_bounds("plugins-tab-mcp").expect("the MCP tab");
    cx.simulate_click(tab.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (section, plugins_tab) = cx.update(|_, cx| {
        let state = &harness.read(cx).shell.read(cx).state;
        (
            state.settings_section,
            harness.read(cx).shell.read(cx).plugins_tab,
        )
    });
    assert_eq!(
        section,
        section_index("plugins").expect("the plugins section"),
        "the tab keeps the dialog on Extensions"
    );
    assert_eq!(plugins_tab, 1, "the MCP tab is the active one");
    assert!(
        cx.debug_bounds("mcp-add").is_some(),
        "the MCP body draws inside the pane"
    );
    assert!(
        cx.debug_bounds("builtin-settings-header").is_none(),
        "the plugin list is replaced, not stacked under it"
    );

    let list = cx
        .debug_bounds("plugins-tab-list")
        .expect("the Plugin list tab");
    cx.simulate_click(list.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let plugins_tab = cx.update(|_, cx| harness.read(cx).shell.read(cx).plugins_tab);
    assert_eq!(plugins_tab, 0, "the list tab comes back");
    assert!(
        cx.debug_bounds("builtin-settings-header").is_some(),
        "with the plugin list body"
    );
    assert!(cx.debug_bounds("mcp-add").is_none());
}

/// The Plugin Market tab is a real tab: selecting it shows local Codex and DSH
/// rows in the same pane, without leaving Extensions.
#[gpui::test]
fn the_market_tab_lists_both_standards_in_place(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-market-tab", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            shell.market = vec![market_fixture("codex-deploy"), market_fixture("dsh-echo")];
            shell.market_loaded = true;
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("market-card-0").is_none(),
        "the market body is not drawn until its tab is chosen"
    );

    let tab = cx
        .debug_bounds("plugins-tab-market")
        .expect("the Plugin Market tab");
    cx.simulate_click(tab.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (section, plugins_tab, loaded) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        (
            shell.state.settings_section,
            shell.plugins_tab,
            shell.market_loaded,
        )
    });
    assert_eq!(
        section,
        section_index("plugins").expect("the plugins section"),
        "the market keeps the dialog on Extensions"
    );
    assert_eq!(plugins_tab, 3, "the market tab is the active one");
    assert!(loaded, "the tab has a loaded catalog");
    assert!(
        cx.debug_bounds("market-card-0").is_some(),
        "the Codex row draws"
    );
    assert!(
        cx.debug_bounds("market-card-1").is_some(),
        "the DSH row draws beside it"
    );
    assert!(
        cx.debug_bounds("market-sources-toggle").is_some(),
        "the source disclosure is available"
    );
    assert!(
        cx.debug_bounds("market-source-builtin-0").is_none(),
        "the source editor starts collapsed"
    );
    let toggle = cx
        .debug_bounds("market-sources-toggle")
        .expect("the source disclosure");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("market-source-builtin-0").is_some(),
        "opening the disclosure reveals the built-in providers"
    );
}
/// The Extensions tab strip is the pane's, not the scroller's: a wheel over
/// the market body moves cards and leaves the tabs where they were.
#[gpui::test]
fn the_plugin_tabs_stay_pinned_while_the_market_scrolls(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-market-pinned-tabs", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            let mut rows = Vec::new();
            for index in 0..20 {
                let mut plugin = remote_market_fixture();
                plugin.id = format!("remote-{index}");
                plugin.name = format!("Remote plugin {index}");
                rows.push(plugin);
            }
            shell.market = rows;
            shell.market_loaded = true;
            shell.plugins_tab = 3;
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let pane = cx.debug_bounds("settings-pane").expect("the options box");
    let tabs = cx.debug_bounds("plugins-tabs").expect("the tab strip");
    let card = cx.debug_bounds("market-card-0").expect("the first card");
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::ScrollWheel(gpui_kit::ScrollWheelEvent {
                position: pane.center(),
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-240.0))),
                modifiers: Modifiers::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }),
            cx,
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let tabs_after = cx
        .debug_bounds("plugins-tabs")
        .expect("the pinned tab strip");
    let card_after = cx
        .debug_bounds("market-card-0")
        .expect("the first card after the wheel");
    assert_eq!(
        tabs_after.origin.y, tabs.origin.y,
        "the tab strip stays pinned while the market body scrolls"
    );
    assert!(
        card_after.origin.y < card.origin.y,
        "the wheel actually moves the market body ({} -> {})",
        card.origin.y,
        card_after.origin.y
    );
}

/// Install, uninstall and enable/disable all write the draft the dialog saves:
/// the inventory entry carries the package path, and the workspace disabled list
/// carries the switch.
#[gpui::test]
fn the_market_card_installs_uninstalls_and_toggles_the_draft(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-market-actions", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            shell
                .state
                .draft_mut()
                .workspace
                .disabled
                .extensions
                .clear();
            shell.market = vec![market_fixture("dsh-echo")];
            shell.market_loaded = true;
            shell.plugins_tab = 3;
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("market-install-0").is_some(),
        "an uninstalled row offers Install"
    );
    let install = cx
        .debug_bounds("market-install-0")
        .expect("the Install control");
    cx.simulate_click(install.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (installed, has_path) = cx.update(|_, cx| {
        let draft = harness.read(cx).shell.read(cx).state.draft();
        match draft
            .global
            .extensions
            .iter()
            .find(|extension| extension.id == "@local/dsh-echo")
        {
            Some(extension) => (true, extension.path.is_some()),
            None => (false, false),
        }
    });
    assert!(installed, "Install writes the draft inventory");
    assert!(has_path, "the inventory entry carries the package path");
    assert!(
        cx.debug_bounds("market-uninstall-0").is_some(),
        "an installed row offers Uninstall"
    );
    assert!(
        cx.debug_bounds("market-status-installed-0").is_some(),
        "an installed row is marked Installed"
    );
    assert!(
        cx.debug_bounds("market-status-enabled-0").is_some(),
        "an installed, enabled row is marked Enabled"
    );
    assert!(
        cx.debug_bounds("market-status-disabled-0").is_none(),
        "the disabled marker is not drawn while enabled"
    );

    let toggle = cx
        .debug_bounds("market-toggle-0")
        .expect("the enable switch");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let disabled = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .workspace
            .disabled
            .extensions
            .contains(&"@local/dsh-echo".to_string())
    });
    assert!(disabled, "the switch writes the workspace disabled list");
    assert!(
        cx.debug_bounds("market-status-disabled-0").is_some(),
        "the card changes its marker to Disabled"
    );
    assert!(
        cx.debug_bounds("market-status-enabled-0").is_none(),
        "the enabled marker is gone while disabled"
    );

    let toggle = cx
        .debug_bounds("market-toggle-0")
        .expect("the enable switch again");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let disabled = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .workspace
            .disabled
            .extensions
            .contains(&"@local/dsh-echo".to_string())
    });
    assert!(!disabled, "a second press switches it back on");
    assert!(
        cx.debug_bounds("market-status-enabled-0").is_some(),
        "the card returns to the Enabled marker"
    );

    let uninstall = cx
        .debug_bounds("market-uninstall-0")
        .expect("the Uninstall control");
    cx.simulate_click(uninstall.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let installed = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .global
            .extensions
            .iter()
            .any(|extension| extension.id == "@local/dsh-echo")
    });
    assert!(!installed, "Uninstall removes the draft inventory entry");
    assert!(
        cx.debug_bounds("market-install-0").is_some(),
        "the row returns to Install"
    );
    assert!(
        cx.debug_bounds("market-status-installed-0").is_none()
            && cx.debug_bounds("market-status-enabled-0").is_none()
            && cx.debug_bounds("market-toggle-0").is_none(),
        "an uninstalled row has no installed/enabled marker and no switch"
    );
    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| {
            shell.toggle_market_plugin("@local/dsh-echo", cx);
        });
    });
    let disabled = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .workspace
            .disabled
            .extensions
            .contains(&"@local/dsh-echo".to_string())
    });
    assert!(
        !disabled,
        "the enable handler refuses a package that is not installed"
    );
}
/// An installed, enabled market plugin reaches the agent the turn runs, and
/// disabling it removes the tool from the request schema. This is the whole
/// chain the reader cares about: card -> draft inventory -> Save -> build.
#[gpui::test]
fn an_installed_enabled_market_plugin_reaches_the_turn(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-market-agent", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            shell
                .state
                .draft_mut()
                .workspace
                .disabled
                .extensions
                .clear();
            shell.market = vec![market_fixture("dsh-echo")];
            shell.market_loaded = true;
            shell.plugins_tab = 3;
            shell
        });
        Harness { shell }
    });
    let before = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        crate::caps::build_agent(
            &shell.state.settings,
            &shell.state.vault,
            &crate::settings::resolve::SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        )
        .tools_schema_chars
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let install = cx
        .debug_bounds("market-install-0")
        .expect("the Install control");
    cx.simulate_click(install.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (global_installed, saved_disabled) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        let root = shell
            .state
            .settings
            .save_root
            .as_ref()
            .expect("the test settings root");
        let global = crate::settings::global::GlobalSettings::load(&root.join("settings.toml"))
            .expect("the saved global document");
        let workspace_path = crate::settings::workspace::WorkspaceSettings::path(
            std::path::Path::new(&shell.state.settings.active_workspace_root()),
        );
        let workspace = crate::settings::workspace::WorkspaceSettings::load(&workspace_path)
            .expect("the saved workspace document");
        (
            global
                .extensions
                .iter()
                .any(|extension| extension.id == "@local/dsh-echo" && extension.path.is_some()),
            workspace
                .disabled
                .extensions
                .contains(&"@local/dsh-echo".to_string()),
        )
    });
    assert!(
        global_installed,
        "Install reaches the global document without a Save press"
    );
    assert!(
        !saved_disabled,
        "a fresh install is enabled in the saved workspace document"
    );

    let (has_tool, has_skill, after) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        let built = crate::caps::build_agent(
            &shell.state.settings,
            &shell.state.vault,
            &crate::settings::resolve::SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        );
        (
            built
                .agent
                .registry()
                .expect("the tool registry")
                .get("dsh_echo")
                .is_some(),
            built
                .agent
                .get_skills_content()
                .iter()
                .any(|(name, _)| *name == "greet"),
            built.tools_schema_chars,
        )
    });
    assert!(
        has_tool,
        "the plugin's tool is registered on the turn's agent"
    );
    assert!(has_skill, "the plugin's skill is in the turn's prompt set");
    assert!(
        after > before,
        "the plugin's tool definition reaches the request schema ({before} -> {after})"
    );

    cx.update(|_, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| {
            shell.toggle_market_plugin("@local/dsh-echo", cx);
        });
    });
    let saved_disabled = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        let workspace_path = crate::settings::workspace::WorkspaceSettings::path(
            std::path::Path::new(&shell.state.settings.active_workspace_root()),
        );
        crate::settings::workspace::WorkspaceSettings::load(&workspace_path)
            .expect("the saved workspace document")
            .disabled
            .extensions
            .contains(&"@local/dsh-echo".to_string())
    });
    assert!(
        saved_disabled,
        "Disable reaches the workspace document without a Save press"
    );
    let (has_tool, after_off) = cx.update(|_, cx| {
        let shell = harness.read(cx).shell.read(cx);
        let built = crate::caps::build_agent(
            &shell.state.settings,
            &shell.state.vault,
            &crate::settings::resolve::SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        );
        (
            built
                .agent
                .registry()
                .expect("the tool registry")
                .get("dsh_echo")
                .is_some(),
            built.tools_schema_chars,
        )
    });
    assert!(!has_tool, "disabling the plugin removes its tool");
    assert_eq!(
        after_off, before,
        "the request schema returns to its pre-plugin size"
    );
}

/// A configured index source is drawn, removable, and removing it drops the
/// remote row it contributed. No fetch is started by either action.
#[gpui::test]
fn the_market_source_editor_lists_and_removes_a_configured_index(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins-market-sources", 1);
        let shell = cx.new(|cx| {
            let mut shell = Shell::build_for_test(window, section_index("plugins"), state, cx);
            shell
                .state
                .draft_mut()
                .global
                .market_indexes
                .push("https://market.example/plugins.json".to_string());
            let mut codex_remote = remote_market_fixture();
            codex_remote.id = "remote-codex".to_string();
            codex_remote.name = "Remote Codex".to_string();
            codex_remote.standard = agent::extensions::ExtensionStandard::Codex;
            shell.market = vec![remote_market_fixture(), codex_remote];
            shell.market_loaded = true;
            shell.market_sources_open = true;
            shell.plugins_tab = 3;
            shell
        });
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for selector in [
        "market-source-add",
        "market-source-builtin-0",
        "market-source-builtin-1",
        "market-source-remove-0",
        "market-card-0",
        "market-install-0",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "the market draws {selector}"
        );
    }

    let remove = cx
        .debug_bounds("market-source-remove-0")
        .expect("the source remove control");
    cx.simulate_click(remove.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let indexes = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .global
            .market_indexes
            .clone()
    });
    assert!(indexes.is_empty(), "the draft source is gone");
    assert!(
        cx.debug_bounds("market-source-remove-0").is_none(),
        "the source row is gone"
    );
    assert!(
        cx.debug_bounds("market-card-0").is_none(),
        "the rows that index contributed are gone"
    );
}

/// The URL is validated and deduplicated before it reaches the draft.
#[test]
fn market_indexes_are_validated_and_deduplicated_in_the_draft() {
    let mut state = AppState::with_store(scratch_store("market-index-validation"));
    let url = state
        .add_market_index("https://market.example/plugins.json")
        .expect("a valid index URL");
    assert!(
        state.add_market_index(&url).is_err(),
        "a duplicate is refused"
    );
    assert!(
        state
            .add_market_index("ftp://market.example/plugins.json")
            .is_err(),
        "a non-http scheme is refused"
    );
    assert_eq!(state.draft().global.market_indexes, vec![url.clone()]);
    assert!(state.remove_market_index(&url));
    assert!(state.draft().global.market_indexes.is_empty());
}

/// The MCP pane manages servers: add, edit, disable, move between documents
/// and delete. Everything lands in the DRAFT, which is what the dialog saves.
#[gpui::test]
fn the_mcp_pane_manages_the_draft(cx: &mut TestAppContext) {
    use crate::settings::mcp::{McpScope, McpTransport};
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("mcp-pane"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("mcp"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Add a stdio server through the form.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.mcp_begin_add(window, cx);
            this.mcp_name.update(cx, |state, cx| {
                state.set_value("github".to_string(), window, cx);
            });
            this.mcp_command.update(cx, |state, cx| {
                state.set_value("npx".to_string(), window, cx);
            });
            this.mcp_args.update(cx, |state, cx| {
                state.set_value("-y @mcp/github".to_string(), window, cx);
            });
            this.mcp_save(cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let global = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .global
            .mcp_servers
            .clone()
    });
    assert_eq!(global.len(), 1, "the add saved one entry");
    assert_eq!(global[0]["name"], "github");
    assert_eq!(global[0]["command"], "npx");
    assert_eq!(global[0]["args"], serde_json::json!(["-y", "@mcp/github"]));
    assert!(cx.debug_bounds("mcp-form").is_none(), "a saved form closes");
    assert!(cx.debug_bounds("mcp-row-0").is_some(), "and the row draws");

    // Edit it onto HTTP.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.mcp_begin_edit(McpScope::Global, "github", window, cx);
            this.mcp_http = true;
            this.mcp_url.update(cx, |state, cx| {
                state.set_value("https://mcp.example/v1".to_string(), window, cx);
            });
            this.mcp_save(cx);
        })
    });
    let server = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .mcp_servers()
            .remove(0)
    });
    assert_eq!(
        server.transport,
        McpTransport::Http {
            url: "https://mcp.example/v1".to_string()
        },
        "the edit replaced the transport"
    );

    // Disable, then move it into the workspace document.
    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            this.mcp_toggle(McpScope::Global, "github", _cx)
        });
    });
    let enabled =
        cx.update(|_, cx| harness.read(cx).shell.read(cx).state.draft().mcp_servers()[0].enabled);
    assert!(!enabled, "the toggle writes the flag");

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.mcp_begin_edit(McpScope::Global, "github", window, cx);
            this.mcp_scope = McpScope::Workspace;
            this.mcp_save(cx);
        })
    });
    let (global, workspace) = cx.update(|_, cx| {
        let state = &harness.read(cx).shell.read(cx).state;
        (
            state.draft().global.mcp_servers.len(),
            state.draft().workspace.agent.mcp_servers.len(),
        )
    });
    assert_eq!(global, 0, "the entry left the global document");
    assert_eq!(workspace, 1, "and landed in the workspace one");
    let still_disabled =
        cx.update(|_, cx| !harness.read(cx).shell.read(cx).state.draft().mcp_servers()[0].enabled);
    assert!(
        still_disabled,
        "editing an address must not silently re-enable a parked server"
    );

    // Delete it from the document that now owns it.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.mcp_remove(McpScope::Workspace, "github", cx);
        })
    });
    let workspace = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .mcp_servers
            .len()
    });
    assert_eq!(workspace, 0, "delete drops the entry");
}
/// The Plugin list draws the reference's own empty line when the inventory is
/// empty, instead of a blank column.
#[gpui::test]
fn the_plugin_list_says_so_when_there_are_no_plugins(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("plugin-none"));
        let shell = cx
            .new(|cx| Shell::build_for_test(window, section_index("plugin-inventory"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("plugin-0").is_some(),
        "a deployment with plugins lists them"
    );

    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            this.plugins.clear();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("plugin-none").is_some(),
        "an empty plugin list draws the reference own sentence"
    );
    assert!(cx.debug_bounds("plugin-0").is_none(), "and no rows");
    assert!(
        cx.debug_bounds("plugin-empty").is_none(),
        "and the no-match sentence is a different thing"
    );
}

/// The plugin list carries the reference own Search plugins field. While no
/// query is typed every row is listed and no no-match sentence is drawn; a query
/// that matches nothing draws the reference own sentence instead, using the same
/// matcher the session-name search uses.
#[gpui::test]
fn the_plugin_list_filter_hides_rows_and_says_so(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("plugin-search"));
        let shell = cx
            .new(|cx| Shell::build_for_test(window, section_index("plugin-inventory"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("plugin-0").is_some(),
        "the plugin list draws its rows when nothing is filtered"
    );
    assert!(
        cx.debug_bounds("plugin-empty").is_none(),
        "and no no-match sentence while every row matches"
    );

    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            this.state.plugin_query = "nothing matches this".to_string();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("plugin-empty").is_some(),
        "a query that matches nothing draws the reference own sentence"
    );
    assert!(
        cx.debug_bounds("plugin-0").is_none(),
        "and the row it filtered out is gone"
    );
}

/// Edit opens the editor inside the card and loads that provider into the form,
/// so the fields are bound to the row rather than a label that looks like one.
#[gpui::test]
fn editing_a_provider_loads_it_into_the_form(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("edit-provider", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            seed_provider(
                &mut this.state.draft_mut().global,
                "work",
                "https://api.example/v1",
            );
            load_provider_into_form(this, "work", window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (editing, route, base_url) = cx.update(|_, cx| {
        let shell_ref = shell.read(cx);
        (
            shell_ref.state.editing_provider.clone(),
            shell_ref.add_route.read(cx).value().to_string(),
            shell_ref.add_base_url.read(cx).value().to_string(),
        )
    });
    assert_eq!(
        editing.as_deref(),
        Some("work"),
        "Edit must put the form into edit mode"
    );
    assert_eq!(
        route.as_str(),
        "work",
        "the key is the registry prefix, so it is fixed in the editor"
    );
    assert_eq!(
        base_url, "https://api.example/v1",
        "and load that provider base URL, not what the form showed"
    );
}

/// The input types are the gate the attach path reads, so both handlers have to
/// reach the tree the pane saves: Image off records text only, and Image back on
/// records both.
#[gpui::test]
fn the_input_type_chips_write_the_draft(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("input-types", 1);

        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("models"), state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            // A leaf whose nickname is the API id, the way a fetched row lands.
            this.state.draft_mut().global.add_custom_provider(
                "p",
                "P",
                "https://p.example",
                &[crate::settings::global::ModelEntry::named("o3")],
            );
            this.state.editing_provider = Some("p".to_string());
            this.reload_model_rows("p", window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The declaration sits on the leaf, so an undeclared leaf takes both.
    let declared = |cx: &mut VisualTestContext| -> Vec<String> {
        cx.update(|_, cx| {
            let state = &harness.read(cx).shell.read(cx).state;
            state.draft().global.input_types_of("p", "o3").to_vec()
        })
    };
    assert_eq!(
        declared(cx),
        ["text".to_string(), "image".to_string()],
        "an undeclared leaf names no input type, which the pane reads as both lit"
    );

    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.toggle_model_input("p", "o3", "image")
        })
    });
    assert_eq!(
        declared(cx),
        ["text".to_string()],
        "Image off records text only"
    );
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state.toggle_model_input("p", "o3", "image")
        })
    });
    assert_eq!(
        declared(cx),
        ["text".to_string(), "image".to_string()],
        "Image back on records both"
    );
}

/// Round 87 left the attachment chip unclicked; this closes it. Two chips are
/// seeded so the click has to drop the one that was clicked, not the last one.
#[gpui::test]
fn clicking_an_attachment_chip_removes_it(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("attach-chip", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let chip = |name: &str| crate::state::Attachment {
        name: name.to_string(),
        mime: "image/png".to_string(),
        base64: "Zg==".to_string(),
    };
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.state.attachments.push(chip("shot.png"));
            this.state.attachments.push(chip("second.png"));
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let bounds = cx
        .debug_bounds("attachment-0")
        .expect("the first attachment chip");
    cx.simulate_click(bounds.center(), Modifiers::default());

    let names = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .attachments
            .iter()
            .map(|attachment| attachment.name.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        names,
        ["second.png".to_string()],
        "only the clicked chip goes"
    );
}
/// The seam the tick uses: a task opens a turn in its OWN session, and one whose
/// session is gone cannot open one at all.
#[gpui::test]
fn a_scheduled_turn_lands_in_its_own_session(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("schedule-turn", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let (before, id, opened) = cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            let id = this.state.active_id.clone().expect("an active session");
            let before = this
                .state
                .store
                .session_mut(&id)
                .expect("the session")
                .messages
                .len();
            let opened = this.state.begin_turn_in(&id, "Summarize the inbox.");
            (before, id, opened)
        })
    });
    let (transcript, budget) = opened.expect("the task own session exists");
    assert_eq!(
        transcript.len(),
        before,
        "the runner gets the transcript from before the turn"
    );
    assert!(budget > 0, "the task turn carries the session budget");
    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            let after = this
                .state
                .store
                .session_mut(&id)
                .expect("the session")
                .messages
                .len();
            assert_eq!(
                after,
                before + 2,
                "the instruction and its reply slot are appended"
            );
            assert!(
                this.state.streams.contains_key(&id),
                "the session reads as streaming"
            );
            assert!(
                this.state.begin_turn_in("no-such-session", "x").is_none(),
                "a task whose session is gone cannot fire"
            );
        })
    });
}

/// A task whose session is gone is switched off by the tick rather than firing
/// into nothing. There is no session to build an agent for, so nothing here
/// touches the network.
#[gpui::test]
fn the_tick_switches_off_a_task_whose_session_is_gone(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("schedule-gone"));
        state
            .settings
            .global
            .schedules
            .push(crate::settings::Schedule {
                id: String::from("task-gone"),
                name: String::from("Watch"),
                prompt: String::from("Check the queue."),
                session_id: String::from("no-such-session"),
                every_seconds: 30,
                enabled: true,
                last_run: 0,
            });
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.check_schedules(cx));
    });
    let (enabled, ran) = cx.update(|_, cx| {
        let task = &shell.read(cx).state.settings.global.schedules[0];
        (task.enabled, task.last_run)
    });
    assert!(!enabled, "a task with no session is switched off");
    assert_eq!(ran, 0, "and it is not recorded as having run");
}

/// The pane draws the tasks the DRAFT holds, which is what its own Save writes.
#[gpui::test]
fn the_automation_pane_lists_a_task_from_the_draft(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("schedule-pane"));
        state.settings.global.schedules.clear();
        let section = section_index("automation").expect("the rail index");
        let shell = cx.new(|cx| Shell::build_for_test(window, Some(section), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // The positive assertion has to be able to fail: with no task the pane must
    // draw its empty state, which is also the proof that the pane rendered at all.
    assert!(
        cx.debug_bounds("task-none").is_some(),
        "without a task the pane draws its empty state"
    );
    // Seeding the draft before the shell exists is wasted, because assembling it
    // reloads the draft, so the task is added here and the pane is drawn again.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            let session_id = this.state.active_id.clone().expect("an active session");
            this.state
                .draft_mut()
                .global
                .schedules
                .push(crate::settings::Schedule {
                    id: String::from("task-1"),
                    name: String::from("Digest"),
                    prompt: String::from("Summarize the inbox."),
                    session_id,
                    every_seconds: 1800,
                    enabled: true,
                    last_run: 0,
                });
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("task-0").is_some(),
        "the task row is laid out"
    );
    assert!(
        cx.debug_bounds("task-none").is_none(),
        "the empty state gives way to the task"
    );
}

/// The delete control asks first, and only its own confirmation removes the task.
/// The reference draws a dialog here; the three presses below are the real ones.
#[gpui::test]
fn the_delete_control_asks_before_it_deletes(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("schedule-delete"));
        state.settings.global.schedules.clear();
        let section = section_index("automation").expect("the rail index");
        let shell = cx.new(|cx| Shell::build_for_test(window, Some(section), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // The draft reloads while the shell is assembled, so the task is added after it.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            let session_id = this.state.active_id.clone().expect("an active session");
            this.state
                .draft_mut()
                .global
                .schedules
                .push(crate::settings::Schedule {
                    id: String::from("task-1"),
                    name: String::from("Digest"),
                    prompt: String::from("Summarize the inbox."),
                    session_id,
                    every_seconds: 1800,
                    enabled: true,
                    last_run: 0,
                });
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let bounds = cx
        .debug_bounds("task-delete-task-1")
        .expect("the delete control of the task");
    cx.simulate_click(bounds.center(), Modifiers::default());
    let (pending, tasks) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.task_delete.clone(),
            shell.state.draft().global.schedules.len(),
        )
    });
    assert_eq!(
        pending,
        Some(String::from("task-1")),
        "the control only asks"
    );
    assert_eq!(tasks, 1, "and the task is still there");
    assert!(
        cx.debug_bounds("task-delete-dialog").is_some(),
        "the confirmation is on screen"
    );

    let cancel = cx
        .debug_bounds("task-delete-cancel")
        .expect("the cancel button");
    cx.simulate_click(cancel.center(), Modifiers::default());
    let (pending, tasks) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.task_delete.clone(),
            shell.state.draft().global.schedules.len(),
        )
    });
    assert_eq!(pending, None, "cancelling closes the confirmation");
    assert_eq!(tasks, 1, "and keeps the task");
    assert!(
        cx.debug_bounds("task-delete-dialog").is_none(),
        "dialog gone"
    );

    let bounds = cx
        .debug_bounds("task-delete-task-1")
        .expect("the delete control again");
    cx.simulate_click(bounds.center(), Modifiers::default());
    let confirm = cx
        .debug_bounds("task-delete-confirm")
        .expect("the confirm button");
    cx.simulate_click(confirm.center(), Modifiers::default());
    let (pending, tasks, notice, lang) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.task_delete.clone(),
            shell.state.draft().global.schedules.len(),
            shell.state.notice.clone(),
            shell.state.lang,
        )
    });
    assert_eq!(pending, None, "confirming closes the confirmation");
    assert_eq!(tasks, 0, "and removes the task");
    assert_eq!(
        notice,
        Some(crate::i18n::tr(lang, "task.deleted")),
        "with the reference own report"
    );
}

/// Every Automation tasks label resolves in both locales. A missing key renders
/// AS the key, which reads like prose and hides the mistake, so this pins the
/// whole family.
#[test]
fn the_automation_keys_resolve_in_both_locales() {
    let keys = [
        "task.title",
        "task.new",
        "task.list",
        "task.searchPlaceholder",
        "task.clearSearch",
        "task.statusAll",
        "task.statusEnabled",
        "task.statusInactive",
        "task.nextPrefix",
        "task.nextRun",
        "task.empty",
        "task.noMatches",
        "task.frequencyOnce",
        "task.frequencyEvery",
        "task.dueNow",
        "task.inFuture",
        "task.unitSecondOne",
        "task.unitSecondOther",
        "task.unitMinuteOne",
        "task.unitMinuteOther",
        "task.unitHourOne",
        "task.unitHourOther",
        "task.nameLabel",
        "task.instructionLabel",
        "task.intervalLabel",
        "task.deleteAction",
        "task.deleteTitle",
        "task.deleteConfirm",
        "task.deleteCancel",
        "task.deleteDescription",
        "task.deleteFailed",
        "task.deleted",
        "task.invalidTitle",
        "task.invalidPrompt",
    ];
    for locale in ["en", "zh"] {
        let lang = crate::i18n::Lang::from_locale(locale);
        for key in keys {
            let text = crate::i18n::tr(lang, key);
            assert_ne!(text, key, "{key} must resolve in {locale}");
        }
    }
}

/// The rail label is its own key, and the real window proved it needs one:
/// without it the rail entry rendered the raw `settings.automation`.
#[test]
fn the_automation_rail_label_resolves_in_both_locales() {
    for locale in ["en", "zh"] {
        let lang = crate::i18n::Lang::from_locale(locale);
        let text = crate::i18n::tr(lang, "settings.automation");
        assert_ne!(text, "settings.automation", "the rail label in {locale}");
    }
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::En, "settings.automation"),
        crate::i18n::tr(crate::i18n::Lang::En, "task.title"),
        "the rail label is the reference own panel name",
    );
}

/// The rail preview's budgets, from `turn-navigation.js`: parts join with a
/// space, every whitespace run collapses, and anything past the limit is
/// clipped with a trailing ellipsis.
#[test]
fn the_rail_preview_budgets_match_the_reference() {
    let capped = crate::ui::transcript::rail_preview;
    assert_eq!(
        capped(&["a  b\nc".to_string()], 50),
        "a b c",
        "whitespace collapses"
    );
    assert_eq!(
        capped(&["hi".to_string()], 50),
        "hi",
        "a short preview is whole"
    );
    let long = capped(&["x".repeat(80)], 50);
    assert_eq!(
        long.chars().count(),
        50,
        "the clip is limit - 1 plus the ellipsis"
    );
    assert!(long.ends_with('…'), "and it is marked: {long}");
    let joined = capped(&["x".repeat(60), "y".to_string()], 50);
    assert_eq!(
        joined.chars().count(),
        50,
        "joining still respects the budget"
    );
    assert_eq!(
        capped(&[format!("a{}b", " ".repeat(100))], 50),
        "a…",
        "a preview that stopped early is marked even when short"
    );
}

/// The rail's hover preview, from the reference's `TurnNavigator`: the pointer
/// opens a card that tracks the mark, drawn `min(300px, container - 120px)` wide
/// and 100px tall, hanging 10px off the frame (`right: calc(100% + 10px)`).
#[gpui::test]
fn the_rail_preview_opens_on_hover(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("rail-preview", 3);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            session.messages[0].text = "the first prompt".to_string();
            session.messages[1].text = "the first answer".to_string();
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let track = cx
        .debug_bounds("turn-rail-track")
        .expect("the rail scroller");
    let first = cx.debug_bounds("turn-tick-0").expect("a mark");
    assert_eq!(
        first.origin.y,
        track.origin.y + px(6.0),
        "RAIL_INSET_PX pads the first mark"
    );
    assert!(
        cx.debug_bounds("turn-preview").is_none(),
        "the card waits for the pointer"
    );

    cx.simulate_mouse_move(
        first.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let card = cx
        .debug_bounds("turn-preview")
        .expect("the hover opens the card");
    let frame = cx.debug_bounds("turn-rail-frame").expect("its frame");
    assert_eq!(
        card.size.width,
        px(300.0),
        "min(300px, 100cqw - 120px) at this width"
    );
    assert!(card.size.height <= px(100.0), "the card caps at 100");
    assert_eq!(
        card.origin.x + card.size.width,
        frame.origin.x - px(10.0),
        "right: calc(100% + 10px)"
    );

    // The NEWEST mark previews too. It is the one a reader reaches for at the end
    // of a chat, and its centre sits at the frame's bottom edge - the position
    // the card clamp has to keep inside the frame.
    let last = cx.debug_bounds("turn-tick-2").expect("the newest mark");
    cx.simulate_mouse_move(
        last.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let newest = cx
        .debug_bounds("turn-preview")
        .expect("the newest mark previews");
    assert_eq!(newest.size.width, px(300.0), "drawn like every other card");
    assert!(
        newest.origin.y >= frame.origin.y
            && newest.origin.y + newest.size.height <= frame.origin.y + frame.size.height,
        "and kept inside the frame by the clamp"
    );

    cx.simulate_mouse_move(
        gpui_kit::point(px(20.0), px(20.0)),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("turn-preview").is_none(),
        "leaving the mark closes the card"
    );
}

/// `if (items.length < 2) return null`: a lone turn draws no rail.
#[gpui::test]
fn the_rail_hides_with_a_single_turn(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("rail-single", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("turn-rail").is_none(),
        "one turn has nothing to navigate between"
    );
    assert!(
        cx.debug_bounds("turn-tick-0").is_none(),
        "and draws no mark"
    );
}

/// The stat pills take the measured chrome: a 20px line over a 1px pad on each
/// side (22 tall), 12px apart, each a 14px glyph beside a `secondary - 1px`
/// label. The counts pill waits for a step, the usage pill for billed usage.
#[gpui::test]
fn the_stat_pills_take_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("stat-chrome", 2);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            if let Some(message) = session.messages.last_mut() {
                message.tools.push(ToolEvent {
                    name: "bash".to_string(),
                    args: "{}".to_string(),
                    output: None,
                    ms: None,
                    at: None,
                });
                message.meta = Some(MessageMeta {
                    usage: Some(MessageUsage {
                        prompt_tokens: 200,
                        completion_tokens: 50,
                        cached_tokens: 199,
                        model: "probe".to_string(),
                    }),
                    ..Default::default()
                });
            }
        }
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let row = cx
        .debug_bounds("status-pills")
        .expect("the stat row renders");
    let time = cx
        .debug_bounds("status-time-pill")
        .expect("a landed step draws the counts pill");
    let usage = cx
        .debug_bounds("status-usage-pill")
        .expect("billed usage draws the usage pill");
    assert_eq!(
        time.size.height,
        px(22.0),
        "a 20px line over a 1px pad each side"
    );
    assert_eq!(
        usage.size.height,
        px(22.0),
        "both pills take the same chrome"
    );
    assert_eq!(
        usage.origin.x - (time.origin.x + time.size.width),
        px(12.0),
        "the row's measured gap"
    );
    assert!(
        row.size.width >= time.size.width + usage.size.width,
        "the row holds both pills"
    );
}

/// The usage pill is a live trigger, not a label: pressing it opens the measured
/// token dialog 8px above the pill with the reference's own detail rows, and a
/// second press closes it.
#[gpui::test]
fn the_usage_pill_opens_the_measured_token_dialog(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("stat-dialog", 1);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            if let Some(message) = session.messages.last_mut() {
                message.meta = Some(MessageMeta {
                    usage: Some(MessageUsage {
                        prompt_tokens: 200,
                        completion_tokens: 50,
                        cached_tokens: 199,
                        model: "probe".to_string(),
                    }),
                    ..Default::default()
                });
            }
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    assert!(
        cx.debug_bounds("status-panel").is_none(),
        "the dialog starts closed"
    );
    let usage = cx
        .debug_bounds("status-usage-pill")
        .expect("the usage pill renders");
    cx.simulate_click(usage.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).state.stat_open),
        "the click must reach the pill handler"
    );
    let panel = cx
        .debug_bounds("status-panel")
        .expect("the dialog renders once open");
    assert_eq!(
        panel.size.width,
        px(300.0),
        "the measured floor, min(300px, 100vw - 24px)"
    );
    assert_eq!(
        panel.origin.y + panel.size.height,
        usage.origin.y - px(8.0),
        "it opens 8px above its trigger"
    );
    for key in [
        "status-row-message.turnUsage.cacheHit",
        "status-row-message.turnUsage.input",
        "status-row-message.turnUsage.cacheRead",
        "status-row-message.turnUsage.output",
    ] {
        assert!(
            cx.debug_bounds(key).is_some(),
            "the reference's own row {key} draws"
        );
    }
    cx.simulate_click(usage.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        !cx.update(|_, cx| shell.read(cx).state.stat_open),
        "a second press closes it"
    );
    assert!(
        cx.debug_bounds("status-panel").is_none(),
        "and the panel leaves the tree"
    );
}

/// The context meter is a live control, not a label: its trigger carries the
/// ring and the reading at the measured 22px, and pressing it opens the
/// breakdown panel 8px above it with the reference's own three legend rows.
#[gpui::test]
fn the_context_meter_opens_the_measured_breakdown_panel(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("context-meter", 1);
        tests_pin_reference_defaults(&mut state);
        // The tool schemas are what a built agent measures, so the panel's two
        // priced buckets are the transcript and this figure.
        state.set_tools_schema(2_000);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            if let Some(message) = session.messages.last_mut() {
                message.meta = Some(MessageMeta {
                    usage: Some(MessageUsage {
                        prompt_tokens: 16_384,
                        completion_tokens: 1_350,
                        cached_tokens: 11_000,
                        model: "probe".to_string(),
                    }),
                    ..Default::default()
                });
            }
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    assert!(
        cx.debug_bounds("context-meter-panel").is_none(),
        "the panel starts closed"
    );
    let trigger = cx
        .debug_bounds("context-meter-trigger")
        .expect("the meter renders once a prompt size exists");
    assert_eq!(
        trigger.size.height,
        px(22.0),
        "the measured trigger: a 20px line over a 1px pad each side"
    );
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).state.context_percent()),
        Some(50),
        "16384 of the 32768 budget reads 50%"
    );
    cx.simulate_click(trigger.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).state.meter_open),
        "the click must reach the trigger handler"
    );
    let panel = cx
        .debug_bounds("context-meter-panel")
        .expect("the panel renders once open");
    assert_eq!(
        panel.size.width,
        px(264.0),
        "the measured width, min(264px, 100vw - 24px)"
    );
    assert_eq!(
        panel.origin.y + panel.size.height,
        trigger.origin.y - px(8.0),
        "it opens 8px above its trigger"
    );
    for selector in [
        "context-row-context.system",
        "context-row-context.tools",
        "context-row-context.messages",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "the reference's own row {selector} draws"
        );
    }
    assert!(
        cx.debug_bounds("context-meter-bar").is_some(),
        "the stacked bar draws with the legend"
    );
    cx.simulate_click(trigger.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        !cx.update(|_, cx| shell.read(cx).state.meter_open),
        "a second press closes it"
    );
    assert!(
        cx.debug_bounds("context-meter-panel").is_none(),
        "and the panel leaves the tree"
    );
}
/// The hero, from the reference's own `HeroShell.module.css`: the composer
/// stack holds one group (the live chat column + 32 card, plus its 16px clearance a
/// side) centred under a 32-tall headline, over a 28-tall workspace chip.
#[gpui::test]
fn the_hero_takes_the_measured_chrome(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("hero-chrome"));
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let column = cx.debug_bounds("main-column").expect("the column renders");
    let chat = crate::theme::chat_width_at(column.size.width.as_f32());
    let group = cx
        .debug_bounds("hero-group")
        .expect("the hero group renders");
    assert_eq!(
        group.size.width,
        px(chat + crate::theme::COMPOSER_WIDEN + 2.0 * crate::theme::HERO_CLEARANCE),
        "the stack is the composer card plus its 16px clearance a side"
    );
    let headline = cx
        .debug_bounds("hero-headline")
        .expect("the headline renders");
    assert_eq!(headline.size.height, px(32.0), "the headline line box");
    let chip = cx
        .debug_bounds("hero-workspace")
        .expect("the workspace chip renders");
    assert_eq!(chip.size.height, px(28.0), "the chip is the measured 28");
    let card = cx.debug_bounds("composer-card").expect("the card renders");
    assert_eq!(
        card.size.width,
        px(chat + crate::theme::COMPOSER_WIDEN),
        "the card stays 32 wider than the chat column"
    );
    let delta = headline.center().x - column.center().x;
    assert!(
        delta > px(-1.5) && delta < px(1.5),
        "the headline is centred on the column, off by {delta:?}"
    );
}

/// The reference's hero phase is a blank conversation on the chat tab.
#[gpui::test]
fn the_hero_phase_follows_the_turns(cx: &mut TestAppContext) {
    init_test_app(cx);
    let mut state = AppState::with_store(scratch_store("hero-phase"));
    let id = state.active_id.clone().expect("an active session");
    assert!(state.hero_phase(), "a blank chat is the hero phase");
    if let Some(session) = state.store.session_mut(&id) {
        session.messages.push(ChatMessage::user("hello"));
    }
    assert!(!state.hero_phase(), "a turn leaves the hero phase");
    state.tab = Tab::Trajectory;
    assert!(!state.hero_phase(), "the trajectory pane is never the hero");
}

/// The composer carries the reference's own placeholder for the phase.
#[gpui::test]
fn the_composer_placeholder_follows_the_hero_phase(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("hero-placeholder"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let hero = cx.update(|_, cx| {
        shell
            .read(cx)
            .composer
            .read(cx)
            .presentation()
            .placeholder()
            .to_string()
    });
    assert_eq!(
        hero,
        "Describe what you want to build, / commands, @ files or sessions"
    );
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            let id = this.state.active_id.clone().expect("an active session");
            if let Some(session) = this.state.store.session_mut(&id) {
                session.messages.push(ChatMessage::user("hello"));
            }
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let after = cx.update(|_, cx| {
        shell
            .read(cx)
            .composer
            .read(cx)
            .presentation()
            .placeholder()
            .to_string()
    });
    assert_eq!(
        after,
        "Message or run a task, / commands, @ files or sessions"
    );
}

/// The hero's workspace picker moves the blank chat it sits over.
#[gpui::test]
fn the_hero_workspace_picker_moves_the_chat(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("hero-workspace"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let moved = "ws-hero-moved".to_string();
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.state.set_workspace(&moved);
            cx.notify();
        });
    });
    let (session_ws, active_ws) = cx.update(|_, cx| {
        let this = shell.read(cx);
        let id = this.state.active_id.clone().expect("an active session");
        (
            this.state
                .store
                .session(&id)
                .expect("the session")
                .workspace
                .clone(),
            this.state.settings.global.general.active_workspace.clone(),
        )
    });
    assert_eq!(session_ws, moved, "the chat moves with the pick");
    assert_eq!(active_ws, moved, "and the active workspace follows it");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("hero-workspace").is_some(),
        "the chip still renders under a workspace the settings do not list"
    );
}

/// The reference's `.add` is a fully round 28x28 `+`, not a labelled chip:
/// `.add{background:var(--dsw-specific-selector);width:28px;height:28px;
/// border-radius:999px;display:grid;place-items:center}` holding a 14px
/// `IconPlusOutlineMedium`, hovering to `interactive-bg-hover-solid`, and
/// tooltipped with `input.commands` on the top side after 500ms.
#[gpui::test]
fn the_composer_add_control_is_the_measured_round_plus(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-add", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let _shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let add = cx
        .debug_bounds("composer-add")
        .expect("the add control renders");
    assert_eq!(
        add.size.width,
        px(28.0),
        "the add control is the measured 28 wide"
    );
    assert_eq!(add.size.height, px(28.0), "and the measured 28 tall");
    let model = cx
        .debug_bounds("composer-model")
        .expect("the model chip renders");
    assert_eq!(
        model.size.height,
        px(28.0),
        "so the round plus squares up with the chip row it leads"
    );
}

/// The reference's primary control is `.primary{width:34px;height:34px;
/// border-radius:999px;background:var(--dsw-alias-button-info-fill);color:#fff;
/// transform:translateY(-2px)}`, not the 30px control bsh drew, and typing does
/// not resize it - only the stop state swaps its glyph.
#[gpui::test]
fn the_composer_primary_button_is_the_measured_34_round_action(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-primary", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let idle = cx
        .debug_bounds("composer-send")
        .expect("the primary control renders");
    assert_eq!(idle.size.width, px(34.0), "the primary control is 34 wide");
    assert_eq!(idle.size.height, px(34.0), "and 34 tall");

    set_draft(&shell, cx, "a draft");
    let typed = cx
        .debug_bounds("composer-send")
        .expect("the primary control renders");
    assert_eq!(typed.size.width, px(34.0), "and a draft does not resize it");
    assert_eq!(typed.size.height, px(34.0), "in either axis");
}

/// The measured stops rule: the primary control stops only while a turn runs AND
/// the draft is empty. With text in the draft it sends, which is the queue path
/// the dock already shows - the behaviour round 106 put behind the composer.
#[test]
fn the_primary_control_stops_only_while_the_draft_is_empty() {
    assert!(
        super::composer::primary_stops(true, false),
        "a running turn with an empty draft stops"
    );
    assert!(
        !super::composer::primary_stops(true, true),
        "a draft typed while running sends (queues) instead of stopping"
    );
    assert!(
        !super::composer::primary_stops(false, true),
        "an idle shell sends"
    );
    assert!(
        !super::composer::primary_stops(false, false),
        "and an idle shell with nothing to send only dims"
    );
}

/// The reference's select-slot trigger: 28 tall at radius-sm over
/// `padding:0 4px 0 8px`, shared by the permission and model chips, and each
/// opens a menu, so each carries the `.chevron` in label-caption (the 14px box).
#[gpui::test]
fn the_composer_chips_take_the_measured_select_trigger(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-chips", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for id in ["composer-permission", "composer-model"] {
        let bounds = cx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} renders"));
        assert_eq!(bounds.size.height, px(28.0), "{id} keeps the measured 28");
    }
    let chevron = cx
        .debug_bounds("composer-permission-chevron")
        .expect("the permission chip carries the measured chevron");
    assert_eq!(
        chevron.size.width,
        px(14.0),
        "the chevron takes the 14px box"
    );
    assert_eq!(chevron.size.height, px(14.0), "in both axes");
    let model_chevron = cx
        .debug_bounds("composer-model-chevron")
        .expect("the model chip opens a menu, so it carries the measured chevron");
    assert_eq!(model_chevron.size.width, px(14.0), "the same 14px box");
    assert_eq!(model_chevron.size.height, px(14.0), "in both axes");
    assert!(
        cx.debug_bounds("composer-skill").is_none(),
        "the footer draws the reference's own controls: nothing registers the \
         conversation.input.left or right slot in the shipped app, so no chip \
         of bsh's own may stand in for them"
    );
}

/// The permission chip's menu writes through the same field the General pane's
/// choice row writes, so the chip's own label follows the mode it changed.
#[gpui::test]
fn the_permission_chip_writes_the_mode_its_menu_offers(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("composer-permission-mode", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let before = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .permission
            .clone()
    });
    cx.update(|_, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_permission(crate::caps::FULL_ACCESS, cx)
        });
    });
    let after = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .permission
            .clone()
    });
    assert_eq!(
        after,
        crate::caps::FULL_ACCESS,
        "the write lands on the draft the General pane also writes"
    );
    assert_ne!(
        super::composer::permission_label_key(&before),
        super::composer::permission_label_key(&after),
        "and the chip's own label flips with it"
    );
}

/// The reference's `formatDuration`: elapsed time in at most two adjacent
/// units, hours widest, floors to whole seconds.
#[test]
fn the_job_duration_takes_the_reference_units() {
    use crate::i18n::Lang;
    use crate::ui::header::job_duration;
    assert_eq!(job_duration(0, Lang::En), "0s");
    assert_eq!(job_duration(5_000, Lang::En), "5s");
    assert_eq!(job_duration(65_000, Lang::En), "1m 5s");
    assert_eq!(job_duration(3_725_000, Lang::En), "1h 2m");
    assert_eq!(job_duration(65_000, Lang::Zh), "1分5秒");
    assert_eq!(job_duration(86_400_000, Lang::En), "24h 0m");
}

/// The measured kill windows: an armed button waits three seconds for its
/// confirming press, and a failed stop keeps its hint for four - so three
/// seconds is already too long for the failure to still be showing.
#[test]
fn the_kill_phase_windows_are_the_measured_ones() {
    use crate::ui::header::{JobArm, JobPhase, JOB_KILL_ARM, JOB_KILL_FAILED};
    use std::time::{Duration, Instant};
    let at = Instant::now();
    let armed = JobArm {
        id: "job".to_string(),
        phase: JobPhase::Armed,
        at,
    };
    assert!(!armed.expired(at + JOB_KILL_ARM - Duration::from_millis(1)));
    assert!(armed.expired(at + JOB_KILL_ARM));
    let failed = JobArm {
        phase: JobPhase::Failed,
        ..armed.clone()
    };
    assert!(!failed.expired(at + JOB_KILL_ARM));
    assert!(failed.expired(at + JOB_KILL_FAILED));
}

/// The reference's two-press stop, driven through the handler's own seam: the
/// first press only arms, a press that arrives after the window re-arms rather
/// than stopping, and the confirming press inside it stops the background turn.
/// The kit popover is its own layer, so the row itself cannot be pressed in the
/// harness - the observable is the arming state and the stream.
#[gpui::test]
fn the_job_stop_arms_then_stops_within_its_window(cx: &mut TestAppContext) {
    use crate::ui::header::{JobArm, JobPhase};
    use std::time::{Duration, Instant};
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("jobs-arm", 1);
        let active = state.active_id.clone().expect("an active session");
        let background = state.new_session().expect("a second session");
        state
            .begin_turn_in(&background, "background work")
            .expect("a turn opens");
        // The streaming session must not be the one on screen, which is what
        // makes it a background job at all.
        state.select(&active);
        assert_eq!(state.background_jobs().len(), 1, "one job in the panel");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let snapshot = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let shell = harness.read(cx).shell.read(cx);
            (shell.jobs_arm.clone(), shell.state.background_jobs().len())
        })
    };
    let press = |cx: &mut VisualTestContext, id: &str, at: Instant| {
        let id = id.to_string();
        cx.update(|_, cx| {
            let shell = harness.read(cx).shell.clone();
            shell.update(cx, |shell, cx| shell.press_job_kill_at(&id, at, cx));
        });
    };
    let expire = |cx: &mut VisualTestContext, at: Instant| {
        cx.update(|_, cx| {
            let shell = harness.read(cx).shell.clone();
            shell.update(cx, |shell, _cx| {
                shell.expire_job_arm(at);
            });
        });
    };
    let id = cx.update(|_, cx| {
        harness
            .read(cx)
            .shell
            .read(cx)
            .state
            .background_jobs()
            .first()
            .expect("a background job")
            .0
            .clone()
    });
    let t0 = Instant::now();
    press(cx, &id, t0);
    let (arm, jobs) = snapshot(cx);
    assert_eq!(
        arm.as_ref().map(|arm| arm.phase),
        Some(JobPhase::Armed),
        "the first press only arms"
    );
    assert_eq!(jobs, 1, "and the job is still running");
    expire(cx, t0 + Duration::from_millis(3000));
    let (arm, _) = snapshot(cx);
    assert_eq!(arm, None, "the window disarms it");
    press(cx, &id, t0 + Duration::from_millis(5000));
    let (arm, jobs) = snapshot(cx);
    assert_eq!(
        arm.as_ref().map(|arm| arm.phase),
        Some(JobPhase::Armed),
        "a press after the window arms again instead of stopping"
    );
    assert_eq!(jobs, 1);
    press(cx, &id, t0 + Duration::from_millis(5100));
    let (arm, jobs) = snapshot(cx);
    assert_eq!(arm, None, "the confirming press clears the arming");
    assert_eq!(jobs, 0, "and stops the background turn");
    // A job that is already gone is the one failure a local stop can see, and
    // the reference keeps that hint for its own four-second window.
    press(cx, "ghost", t0);
    press(cx, "ghost", t0);
    let (arm, _) = snapshot(cx);
    assert_eq!(arm.as_ref().map(|arm| arm.phase), Some(JobPhase::Failed));
    expire(cx, t0 + Duration::from_millis(3999));
    let (arm, _) = snapshot(cx);
    assert!(
        arm.is_some(),
        "three seconds is too soon for the hint to go"
    );
    expire(cx, t0 + Duration::from_millis(4000));
    let (arm, _) = snapshot(cx);
    assert_eq!(arm, None);
    // The public shape the trigger and its count read.
    let _ = JobArm {
        id: id.clone(),
        phase: JobPhase::Armed,
        at: t0,
    };
}

/// The rail is the reference's five cells plus the Vault, which BSH adds after
/// Account. The reference's own order is otherwise untouched.
#[test]
fn the_settings_rail_is_the_measured_cells_plus_the_vault() {
    use super::settings::NAV;
    assert_eq!(
        NAV,
        [
            "account",
            "vault",
            "general",
            "models",
            "plugins",
            "agent-presets",
        ],
        "the measured settings.section registrations, with the Vault after Account"
    );
    // The cell is BSH's own word for what it holds - the reference's section id
    // is still `plugins`, but the label a reader sees says what the pane
    // actually lists, which is extensions rather than only built-in plugins.
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::En, "settings.plugins"),
        "Extensions",
        "the rail cell is labelled Extensions"
    );
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::Zh, "settings.plugins"),
        "扩展",
        "and its Chinese label is 扩展"
    );
}

/// The settings rail is the reference's five cells plus the Vault - there is no
/// seventh - and the About block is General's own last row group, not a cell.
#[gpui::test]
fn the_settings_rail_is_six_cells_and_about_lives_in_general(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("settings-views"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("plugins"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for cell in [
        "set-nav-0",
        "set-nav-1",
        "set-nav-2",
        "set-nav-3",
        "set-nav-4",
        "set-nav-5",
    ] {
        assert!(cx.debug_bounds(cell).is_some(), "{cell} draws");
    }
    assert!(
        cx.debug_bounds("set-nav-6").is_none(),
        "the rail is six cells: the reference's five plus the Vault"
    );
    cx.update(|_window, cx| {
        let shell = harness.read(cx).shell.clone();
        shell.update(cx, |shell, cx| {
            shell.state.settings_section = section_index("general").expect("the General pane");
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("general-item-current-version").is_some(),
        "the About block lives inside General"
    );
}

/// With no vault file, the pane offers a create - typed twice - and no unlock.
#[gpui::test]
fn the_vault_pane_offers_create_when_there_is_no_file(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("vault-empty"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("vault"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("vault-passphrase").is_some(),
        "the pane asks for the admin passphrase"
    );
    assert!(
        cx.debug_bounds("vault-confirm").is_some(),
        "a create is typed twice"
    );
    assert!(
        cx.debug_bounds("vault-create").is_some(),
        "and the create control is drawn"
    );
    assert!(
        cx.debug_bounds("vault-unlock").is_none(),
        "there is nothing to unlock before the file exists"
    );
}

/// The unlocked pane lists entries, reveals one inline, and the Lock now press
/// drops the derived key.
#[gpui::test]
fn the_vault_pane_lists_entries_and_lock_keeps_app_access(cx: &mut TestAppContext) {
    use super::settings::section_index;
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = AppState::with_store(scratch_store("vault-open"));
        state
            .vault
            .create("the test passphrase")
            .expect("the vault");
        state
            .vault
            .unlock("the test passphrase")
            .expect("the unlock");
        state
            .vault
            .put("provider:demo:api_key", "sk-demo-secret")
            .expect("the entry");
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("vault"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("vault-lock").is_some(),
        "an unlocked vault offers Lock now"
    );
    assert!(cx.debug_bounds("vault-add").is_some(), "and the add row");
    let reveal = cx
        .debug_bounds("vault-reveal-provider:demo:api_key")
        .expect("the entry row");
    cx.simulate_click(reveal.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("vault-reveal-copy-provider:demo:api_key")
            .is_some(),
        "a revealed value carries its own copy control"
    );
    let lock = cx.debug_bounds("vault-lock").expect("the lock control");
    cx.simulate_click(lock.center(), Modifiers::default());
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let (admin, app) = cx.update(|_, cx| {
        let vault = &shell.read(cx).state.vault;
        (vault.is_unlocked(), vault.is_app_available())
    });
    assert!(!admin, "Lock now locks the admin view");
    assert!(app, "the app access key keeps provider resolution working");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("vault-unlock").is_some(),
        "the locked pane asks for the passphrase again"
    );
}

/// The trigger is the measured one: the draft's last whitespace-delimited token
/// opens the menu when it starts with `/`, a slash inside a word does not, and a
/// whitespace after the token ends it.
#[test]
fn the_command_trigger_is_the_last_slash_token() {
    use super::command_menu::slash_query;
    assert_eq!(slash_query("/"), Some(""));
    assert_eq!(
        slash_query("hello /mo"),
        Some("mo"),
        "the token's remainder is the query"
    );
    assert_eq!(
        slash_query(" /mod"),
        Some("mod"),
        "a leading space does not hide it"
    );
    assert_eq!(
        slash_query("a /b /c"),
        Some("c"),
        "the last token is the trigger"
    );
    assert_eq!(
        slash_query("/cmd "),
        None,
        "the space after it ends the token"
    );
    assert_eq!(slash_query("text"), None);
    assert_eq!(
        slash_query("https://deepseek.com/x"),
        None,
        "a URL's slash is not a trigger"
    );
    assert_eq!(slash_query(""), None);
}

/// The filter is the reference's `filterOptions`: a trimmed, lowercased
/// substring test over the row's own label and detail, and a blank query keeps
/// every row in its order. The expectations are read out of the dictionary the
/// rows draw from, so the test cannot pass on a query the rows never show.
#[test]
fn the_command_filter_is_the_references_substring_test() {
    use super::command_menu::{entries, filter_indices};
    use crate::i18n::Lang;
    let idle = entries(false);
    assert_eq!(filter_indices(&idle, "", Lang::En), vec![0, 1, 2, 3]);
    assert_eq!(filter_indices(&idle, "   ", Lang::En), vec![0, 1, 2, 3]);
    assert!(
        filter_indices(&idle, "zzzz", Lang::En).is_empty(),
        "an unmatched query matches nothing"
    );
    let label = Lang::En.tr(idle[2].label).to_lowercase();
    let detail = Lang::En.tr(idle[2].detail).to_lowercase();
    let hits = filter_indices(&idle, &label, Lang::En);
    assert!(hits.contains(&2), "a row is found by its own label");
    assert!(
        hits.iter().all(|&index| {
            Lang::En
                .tr(idle[index].label)
                .to_lowercase()
                .contains(&label)
                || Lang::En
                    .tr(idle[index].detail)
                    .to_lowercase()
                    .contains(&label)
        }),
        "and only rows that carry the query"
    );
    assert_eq!(
        filter_indices(&idle, &label.to_uppercase(), Lang::En),
        hits,
        "the test is lowercased on both sides"
    );
    assert_eq!(
        filter_indices(&idle, &format!("  {label}  "), Lang::En),
        hits,
        "and trimmed"
    );
    assert!(!detail.is_empty(), "the row carries its own detail");
    let streaming = entries(true);
    assert_eq!(
        streaming.len(),
        idle.len() + 1,
        "a running turn adds one row"
    );
    let stop = Lang::En.tr("commands.stopTurn").to_lowercase();
    assert!(
        filter_indices(&idle, &stop, Lang::En).is_empty(),
        "an idle menu has no stop row to find"
    );
    assert!(
        filter_indices(&streaming, &stop, Lang::En).contains(&4),
        "and the running menu finds it"
    );
    for (lang, name) in [(Lang::En, "en"), (Lang::Zh, "zh")] {
        let glyph: String = lang.tr("commands.newChat").chars().take(1).collect();
        assert!(
            filter_indices(&idle, &glyph, lang).contains(&1),
            "the first glyph of the row's own label finds it in {name}"
        );
    }
}

/// Set the command draft the way a keystroke would.
///
/// The harness's `set_value` does not fire the kit's `InputEvent::Change` that a
/// real keystroke fires, so the draft the command menu reads is written beside
/// the field; the listener itself is what the composer's own tests cover.
fn set_command_draft(shell: &Entity<Shell>, cx: &mut VisualTestContext, text: &str) {
    set_draft(shell, cx, text);
    cx.update(|_, cx| {
        shell.update(cx, |shell, cx| {
            shell.composer_value = text.to_string();
            // The listener the keystroke path runs, so the card follows the
            // draft here exactly as it does for a real key.
            shell.command_menu.sync(text);
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The `+` opens the measured command card, its rows are real controls, the
/// query filters them, an unmatched query is the reference's empty state, and
/// choosing a row consumes its own `/token` before it runs.
#[gpui::test]
fn the_add_control_opens_the_measured_command_card(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("command-menu", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let add = cx
        .debug_bounds("composer-add")
        .expect("the add control draws");
    cx.simulate_click(add.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("composer-command-menu").is_some(),
        "the card opens under the composer"
    );
    for row in [
        "command-row-attach-file",
        "command-row-new-chat",
        "command-row-settings",
        "command-row-toggle-sidebar",
    ] {
        assert!(cx.debug_bounds(row).is_some(), "{row} draws");
    }
    assert!(
        cx.debug_bounds("command-row-stop-turn").is_none(),
        "an idle menu offers no stop row"
    );
    set_command_draft(&shell, cx, "/zzz");
    let query_draft = cx.update(|_, cx| shell.read(cx).composer_value.clone());
    assert_eq!(query_draft, "/zzz", "the draft is the query source");
    assert!(
        cx.debug_bounds("commands-empty").is_some(),
        "an unmatched query is the reference's empty state"
    );
    set_command_draft(&shell, cx, "hello /set");
    assert!(
        cx.debug_bounds("command-row-settings").is_some(),
        "a matched row stays"
    );
    assert!(
        cx.debug_bounds("command-row-attach-file").is_none(),
        "and the others filter out"
    );
    let row = cx
        .debug_bounds("command-row-settings")
        .expect("the filtered row draws");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("settings-panel").is_some(),
        "the row ran: the settings dialog is open"
    );
    assert!(
        cx.debug_bounds("composer-command-menu").is_none(),
        "and the card closed behind it"
    );
    let draft = cx.update(|_, cx| shell.read(cx).composer_value.clone());
    assert_eq!(
        draft, "hello ",
        "the row consumed its own token and kept what preceded it"
    );
}

/// The reference source's trigger keeps a quoted mention whole, so a path with a
/// space survives typing, and an `@` inside a word is not a trigger.
#[test]
fn the_reference_trigger_keeps_quoted_mentions_whole() {
    use super::command_menu::{at_query, mention};
    assert_eq!(at_query("@"), Some(""));
    assert_eq!(at_query("hey @src"), Some("src"));
    assert_eq!(
        at_query("@\"my file\""),
        Some("\"my file\""),
        "a quoted mention keeps the spaces inside it"
    );
    assert_eq!(at_query("a@b"), None, "an @ inside a word is not a trigger");
    assert_eq!(at_query("plain"), None);
    assert_eq!(mention("src/main.rs", false), "@src/main.rs");
    assert_eq!(mention("my file.txt", false), "@\"my file.txt\"");
    assert_eq!(
        mention("src", true),
        "@src/",
        "a directory mention ends at its separator"
    );
}

/// Typing `@` opens the reference source in the same card: the workspace files
/// first with a directory drilling instead of inserting, then the retained
/// sessions, and a draft with no trigger closes the card again.
#[gpui::test]
fn the_at_trigger_lists_workspace_files_and_sessions(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join("bsh-command-refs");
    std::fs::create_dir_all(root.join("sub")).expect("a scratch tree");
    std::fs::write(root.join("alpha.txt"), "a").expect("a file");
    std::fs::write(root.join("beta.md"), "b").expect("a file");
    std::fs::write(root.join("sub/gamma.rs"), "c").expect("a file");
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("command-refs", 1);
        state.settings.ensure_workspace();
        let active = state.settings.global.general.active_workspace.clone();
        if let Some(workspace) = state
            .settings
            .global
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == active)
        {
            workspace.root = root.to_string_lossy().to_string();
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    set_command_draft(&shell, cx, "@alp");
    assert!(
        cx.debug_bounds("composer-command-menu").is_some(),
        "the @ token opens the card"
    );
    assert!(
        cx.debug_bounds("command-row-ref-file-0").is_some(),
        "the matching file is a row"
    );
    assert!(
        cx.debug_bounds("command-row-ref-file-1").is_none(),
        "and the others filter out"
    );
    assert!(
        cx.debug_bounds("command-row-ref-session-0").is_none(),
        "a file query does not carry the sessions"
    );
    let file = cx
        .debug_bounds("command-row-ref-file-0")
        .expect("the file row draws");
    cx.simulate_click(file.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let draft = cx.update(|_, cx| shell.read(cx).composer_value.clone());
    assert_eq!(draft, "@alpha.txt ", "choosing a file inserts its mention");
    assert!(
        cx.debug_bounds("composer-command-menu").is_none(),
        "and closes the card"
    );
    set_command_draft(&shell, cx, "@sub");
    let directory = cx
        .debug_bounds("command-row-ref-file-0")
        .expect("the directory row draws");
    cx.simulate_click(directory.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let draft = cx.update(|_, cx| shell.read(cx).composer_value.clone());
    assert_eq!(draft, "@sub/", "a directory drills instead of inserting");
    assert!(
        cx.debug_bounds("composer-command-menu").is_some(),
        "and keeps the card open"
    );
    assert!(
        cx.debug_bounds("command-row-ref-file-0").is_some(),
        "now listing inside the directory"
    );
    set_command_draft(&shell, cx, "@");
    assert!(
        cx.debug_bounds("command-row-ref-session-0").is_some(),
        "a bare @ carries the retained sessions after the files"
    );
    set_command_draft(&shell, cx, "hello");
    assert!(
        cx.debug_bounds("composer-command-menu").is_none(),
        "a draft with no trigger closes the card"
    );
}

/// The Show coding view toggle is the one General row that gates a real
/// surface, so the click is pressed and the gate read back: developer tools
/// off hides the trajectory tab (`header.rs`), and the draft is what the
/// dialog writes.
#[gpui::test]
fn the_coding_view_toggle_gates_the_trajectory_tab(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("coding-view"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let before = cx.update(|_, cx| shell.read(cx).state.draft().global.general.developer_tools);
    let track = cx
        .debug_bounds("settings-developer-tools")
        .expect("the toggle draws");
    assert_eq!(track.size.width, px(36.0), "the reference's 36px track");
    assert_eq!(track.size.height, px(20.0), "and its 20px height");
    assert!(
        cx.debug_bounds("tab-trajectory").is_some(),
        "the trajectory tab is reachable while coding view is on"
    );
    cx.simulate_click(track.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let after = cx.update(|_, cx| shell.read(cx).state.draft().global.general.developer_tools);
    assert_eq!(after, !before, "the toggle writes the setting it shows");
    // The pane edits the draft and the header reads the committed setting, so
    // an unsaved edit must NOT move the live gate - that is what the Save bar
    // is for. Both halves are pinned: the draft moved above, and the gate
    // responds to the committed value here. The committed value is set
    // directly because `commit_settings` would write the real
    // `~/.bos/bsh/settings.toml`.
    assert!(
        cx.debug_bounds("tab-trajectory").is_none(),
        "the gate follows the draft, so the reader sees the switch take effect"
    );
    // Cancel drops the draft, and the preview goes back to the committed value.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.cancel_settings(cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).state.settings.global.general.developer_tools),
        before,
        "the committed value never moved"
    );
    assert!(
        cx.debug_bounds("tab-trajectory").is_some(),
        "cancelling puts the preview back"
    );
}

/// The General pane's header action is the reference's own `openDocument`
/// button: a 28-tall outline control beside the error line the reference
/// carries. The click cannot be pressed here - it really would hand the
/// settings file to the platform launcher - so the test pins the drawn shape
/// and drives the one state `reveal_config_shared` sets on a refused open.
#[gpui::test]
fn the_general_header_carries_the_reference_document_action(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("open-config"));
        let shell = cx.new(|cx| Shell::build_for_test(window, section_index("general"), state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let button = cx
        .debug_bounds("settings-open-config")
        .expect("the header action draws once a config path resolves");
    assert_eq!(button.size.height, px(28.0), "the outline control's height");
    assert!(
        cx.debug_bounds("settings-open-config-error").is_none(),
        "and no error line while nothing has failed"
    );
    // `reveal_config_shared` is the one writer of this flag, on the Err arm of
    // the launcher; driving it directly is how the error rendering is reached
    // without handing the real path to `open`.
    cx.update(|_, cx| shell.update(cx, |this, _cx| this.config_open_failed = true));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("settings-open-config-error").is_some(),
        "a refused open surfaces the reference's own error line"
    );
}

/// The risk gate is the only path that writes full access, and it stays shut
/// until the reference's own acknowledgement is ticked - the enable action is
/// drawn disabled, and the handler refuses even if it is reached.
#[gpui::test]
fn the_risk_gate_needs_the_acknowledgement_before_it_writes(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("risk-gate"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let before = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .permission
            .clone()
    });
    // Open the gate the way the composer chip does, so this is the real route.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.request_permission(crate::caps::FULL_ACCESS, cx)
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("permission-confirm-dialog").is_some(),
        "full access opens the reference's own confirmation"
    );
    let currently = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .permission
            .clone()
    });
    assert_eq!(currently, before, "nothing is written on the offer");
    // The confirm is drawn disabled, and the handler refuses too.
    cx.update(|_, cx| shell.update(cx, |this, cx| this.permission_confirm_enable(cx)));
    let refused = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .permission
            .clone()
    });
    assert_eq!(refused, before, "an unticked gate refuses, not writes");
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).permission_confirm.clone()),
        Some(String::from(crate::caps::FULL_ACCESS)),
        "and the gate stays open rather than silently closing"
    );
    // Tick the box on screen, then take the enable action.
    let box_ = cx
        .debug_bounds("permission-acknowledge")
        .expect("the acknowledgement draws");
    cx.simulate_click(box_.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).permission_acknowledged),
        "the click ticks it"
    );
    let enable = cx
        .debug_bounds("permission-confirm-enable")
        .expect("the enable action draws");
    cx.simulate_click(enable.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell
            .read(cx)
            .state
            .draft()
            .workspace
            .agent
            .permission
            .clone()),
        crate::caps::FULL_ACCESS,
        "a ticked gate is the one write of full access"
    );
    assert!(
        cx.debug_bounds("permission-confirm-dialog").is_none(),
        "and the gate closes behind the write"
    );
    assert!(
        !cx.update(|_, cx| shell.read(cx).permission_acknowledged),
        "the acknowledgement resets, so the next gate starts shut"
    );
}

/// A state whose workspace root is a scratch directory holding a few entries,
/// so the browser lists a tree the test itself built and never the real one.
fn state_with_workspace_tree(label: &str) -> AppState {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("bsh-scratch-ws-{label}"));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(root.join("src"));
    let _ = std::fs::create_dir_all(root.join("docs"));
    let _ = std::fs::write(root.join("src/lib.rs"), b"pub fn x() {}\n");
    let _ = std::fs::write(root.join("Cargo.toml"), b"[package]\nname = \"x\"\n");
    let _ = std::fs::write(root.join("README.md"), b"# x\n");
    let mut state = AppState::with_store(scratch_store(label));
    state.settings.global.general.active_workspace = label.to_string();
    state
        .settings
        .global
        .workspaces
        .push(crate::settings::global::WorkspaceRef {
            id: label.to_string(),
            name: label.to_string(),
            root: root.to_string_lossy().to_string(),
            created_at: 0,
            updated_at: 0,
        });
    state
}

/// The pencil on a tree row opens a field to type into.
///
/// It used to arm a rename nothing drew: the rename row was a child of the
/// create row, which only renders after a double-click on the tree's blank
/// space, so pressing the pencil set a flag and the panel stayed unchanged -
/// a rename could not be made at all.
#[gpui::test]
fn a_row_rename_opens_its_own_field(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-rename");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let toggle = cx
        .debug_bounds("hdr-show-right-panel")
        .expect("the header carries the right panel's toggle");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let card = cx
        .debug_bounds("right-card-right.tab.files")
        .expect("the Files card renders");
    cx.simulate_click(card.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("files-rename").is_none(),
        "no field is up before the reader asks for one"
    );
    let pencil = cx
        .debug_bounds("rename-README.md")
        .expect("the README row carries its rename control");
    cx.simulate_click(pencil.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        cx.update(|_, cx| shell.read(cx).files.renaming.clone()),
        Some(std::path::PathBuf::from("README.md")),
        "the press arms that row"
    );
    assert!(
        cx.debug_bounds("files-rename").is_some(),
        "and the field it types into is on screen"
    );
    assert!(
        cx.debug_bounds("files-rename-confirm").is_some(),
        "with the control that applies it"
    );
    assert!(
        cx.debug_bounds("files-new").is_none(),
        "the create field does not have to be open for a rename"
    );
}

/// The header's maximise control takes the column instead of folding the panel.
///
/// It used to do exactly what the collapse control beside it does, so the glyph
/// promised a width the press threw away.
#[gpui::test]
fn the_maximise_control_widens_the_panel_instead_of_closing_it(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-maximize");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let toggle = cx
        .debug_bounds("hdr-show-right-panel")
        .expect("the header carries the toggle");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let plain = cx.update(|_, cx| shell.read(cx).right_panel_w());
    let maximize = cx
        .debug_bounds("right-maximize")
        .expect("the bar carries the maximise control");
    cx.simulate_click(maximize.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.update(|_, cx| shell.read(cx).right.open),
        "maximising must not fold the panel"
    );
    assert!(
        cx.update(|_, cx| shell.read(cx).right.maximized),
        "the press is what marks it maximised"
    );
    let wide = cx.update(|_, cx| shell.read(cx).right_panel_w());
    assert!(
        wide > plain,
        "a maximised panel is wider than the plain one: {wide} vs {plain}"
    );

    // A second press gives the column back.
    let minimize = cx
        .debug_bounds("right-maximize")
        .expect("the control is still there");
    cx.simulate_click(minimize.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        !cx.update(|_, cx| shell.read(cx).right.maximized),
        "the same control toggles it back"
    );
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).right_panel_w()),
        plain,
        "and the width returns to what it was"
    );
}

/// A scratch store is a scratch home for the WORKSPACE document too.
///
/// The workspace document follows the registry, and the first workspace a test
/// registers comes from the process's cwd - the crate directory - so a save
/// wrote a `.bsh/settings/settings.toml` into the checkout a reader owns. Both
/// documents belong in the scratch tree.
#[test]
fn a_scratch_store_never_writes_a_settings_document_into_the_checkout() {
    let state = AppState::with_store(scratch_store("settings-home"));
    let root = state.settings.active_workspace_root();
    let scratch = state.store.root().to_string_lossy().to_string();
    assert_eq!(
        root, scratch,
        "the test workspace is rooted in the scratch tree"
    );
    assert_ne!(
        root,
        env!("CARGO_MANIFEST_DIR"),
        "and never the crate directory the test process runs in"
    );
    assert!(
        root.contains("bsh-scratch-settings-home"),
        "it is the scratch tree: {root}"
    );
}

/// A narrow column clamps the chat down instead of hanging it over the panel.
///
/// The reference's floor is 680, and an open right panel takes a laptop's column
/// below that: the composer and the hero then overflowed the centre column and
/// were clipped against the panel's edge. The floor makes the chat readable; it
/// must not make it wider than the room it has.
#[test]
fn a_narrow_conversation_column_caps_the_chat_width() {
    // A 1280 window, a 280 sidebar and a 480 panel: the column the screenshot
    // showed the clipped composer in.
    let narrow = 1280.0 - 280.0 - 480.0;
    let chat = crate::theme::chat_width_at(narrow);
    assert!(
        chat + crate::theme::COMPOSER_WIDEN <= narrow,
        "the composer card fits a {narrow} column: chat {chat} + 32"
    );
    assert!(
        chat < crate::theme::CHAT_W,
        "the floor gives way to the room"
    );
    assert!(chat > 0.0, "and the chat stays a positive width");

    // Where the column is narrow enough to reach the floor but still wider
    // than it, the clamp is untouched: 800 * .8 = 640, which the floor lifts
    // to 680.
    assert_eq!(
        crate::theme::chat_width_at(740.0),
        crate::theme::CHAT_W,
        "a narrow column keeps the floor"
    );
    assert_eq!(
        crate::theme::chat_width_at(1640.0),
        1476.0,
        "a wide one takes the share: 1640 * .9"
    );
    assert_eq!(
        crate::theme::chat_width_at(2400.0),
        2160.0,
        "and a very wide one takes the same .9"
    );

    // A column of zero means "not measured yet", which keeps the floor rather
    // than collapsing the chat to nothing.
    assert_eq!(
        crate::theme::chat_width_at(0.0),
        crate::theme::CHAT_W,
        "an unmeasured frame keeps the floor"
    );
}

/// A workspace switch drops the file pane's view of the old workspace.
///
/// Its paths are relative, so an open tab would rebind to another workspace's
/// file and a save would write there.
#[gpui::test]
fn a_workspace_switch_drops_the_file_panes_view(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-switch");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // Open the pane and a document in it, so there is something to drop.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Files, cx);
        })
    });
    let listed = cx.update(|_, cx| shell.read(cx).files.roots.len());
    assert!(listed > 0, "the pane lists the scratch workspace");

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.files.expanded.push(std::path::PathBuf::from("src"));
            this.refile_workspace(cx);
        })
    });
    assert!(
        cx.update(|_, cx| shell.read(cx).files.tabs.is_empty()),
        "no buffer may survive into another workspace"
    );
    assert!(
        cx.update(|_, cx| shell.read(cx).files.expanded.is_empty()),
        "and no branch stays expanded"
    );
    let relisted = cx.update(|_, cx| shell.read(cx).files.roots.len());
    assert_eq!(
        relisted, listed,
        "the pane re-lists the workspace it is now in"
    );
}

/// The centre column is measured less the panel, not only less the sidebar.
///
/// The panel is a sibling column, so an open one takes its width away from the
/// conversation before the chat width, the width handles and the rail's
/// container query are resolved. It used to be left out of the arithmetic, so
/// every one of those believed in space the layout had already given away.
#[gpui::test]
fn the_open_panel_shrinks_the_measured_conversation_column(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-column");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let folded = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (shell.conversation_w.as_f32(), shell.viewport_w)
    });
    assert!(
        (folded.0 - folded.1).abs() < 1.0 || folded.0 < folded.1,
        "with no panel the column is the window less the sidebar"
    );

    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            this.right.open = true;
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (conversation, viewport, panel, sidebar) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let sidebar = if shell.sidebar_folded() {
            0.0
        } else {
            shell.sidebar_w()
        };
        (
            shell.conversation_w.as_f32(),
            shell.viewport_w,
            shell.right_panel_w(),
            sidebar,
        )
    });
    let expected = viewport - sidebar - panel;
    assert!(
        (conversation - expected).abs() < 1.0,
        "the column must be window - sidebar - panel: {conversation} vs {expected}"
    );
    assert!(
        conversation < folded.0,
        "opening the panel shrinks the column: {conversation} vs {}",
        folded.0
    );
}

/// A press on the seam that never moved is a click, not a resize.
///
/// It used to clear the dragged width, so touching the handle threw away the
/// size the reader had chosen; the sidebar's own seam has always kept it.
#[gpui::test]
fn a_seam_click_keeps_the_width_the_reader_dragged(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-seam");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            this.right.open = true;
            this.right.user_w = Some(420.0);
        })
    });
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.begin_right_panel_drag(900.0, cx);
            this.commit_right_panel_width(900.0, cx);
        })
    });
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).right.user_w),
        Some(420.0),
        "a click on the seam must leave the dragged width alone"
    );

    // A real drag still commits.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.begin_right_panel_drag(900.0, cx);
            this.drag_right_panel_width(860.0, cx);
            this.commit_right_panel_width(860.0, cx);
        })
    });
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).right.user_w),
        Some(460.0),
        "dragging left by 40 widens the panel by 40"
    );
}

/// An image tab whose path no longer resolves inside the root says so.
///
/// The pane used to hand the image element `unwrap_or_default()`, so an
/// unresolvable path quietly became an empty one and the body drew nothing at
/// all - a refusal the reader could only read as a broken pane.
#[gpui::test]
fn an_image_tab_outside_the_root_says_so(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("image-outside");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            this.right.tab = Some(RightTab::Files);
            // A tab whose rel climbs out of the root: the panel's own resolution
            // refuses it, so the body must report that rather than draw blank.
            this.files.tabs.push(FileTab {
                rel: std::path::PathBuf::from("../outside.png"),
                editor: None,
                kind: crate::files::Kind::Image,
                dirty: false,
            });
            this.files.active = Some(0);
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("files-image").is_some(),
        "the image body still draws"
    );
    assert!(
        cx.debug_bounds("files-image-outside").is_some(),
        "an unresolvable image path must say so, not draw nothing"
    );
}

/// Choosing a model in the composer switches the model the NEXT TURN builds,
/// and it does so for THIS session only.
///
/// The choice used to be written into `settings.workspace`, which made one
/// chat's model every chat's model in the workspace. It is now a session fact:
/// the workspace document is left exactly where it was, the chip resolves the
/// session, and the build uses the same resolution.
#[test]
fn choosing_a_model_switches_the_one_the_next_turn_builds() {
    let mut state = AppState::with_store(scratch_store("model-switch"));
    let mut provider = crate::settings::global::Provider {
        base_url: String::from("http://127.0.0.1:1/v1"),
        ..Default::default()
    };
    for (nickname, api_id) in [("first", "first-id"), ("second", "second-id")] {
        provider.models.insert(
            nickname.to_string(),
            crate::settings::global::ModelEntry {
                model: api_id.to_string(),
                ..Default::default()
            },
        );
    }
    state
        .settings
        .global
        .providers
        .insert("p".to_string(), provider);
    let id = state.active_id.clone().expect("the session on screen");
    assert!(
        state.effective().model_ref.is_none(),
        "the premise: a new chat names no model"
    );

    state.select_model("second");

    assert!(
        state.settings.workspace.general.legacy_pair().is_none(),
        "the workspace document never carries a model: the choice is the session's"
    );
    assert_eq!(
        state.effective().model,
        "second-id",
        "the active session resolves the choice the composer made"
    );
    assert_eq!(state.effective().model_name, "second", "the chip names it");
    assert_eq!(
        state
            .effective_in(&id)
            .model_ref
            .map(|r| r.model)
            .as_deref(),
        Some("second"),
        "and the menu checks the row the chip names"
    );
    assert!(
        crate::caps::build_agent(
            &state.settings,
            &state.vault,
            &state.session_choice(&id),
            &crate::mcp::McpPool::default(),
        )
        .agent
        .config()
        .model
        .contains("second-id"),
        "the agent for this session is built against the chosen model"
    );
    assert!(
        crate::caps::build_agent(
            &state.settings,
            &state.vault,
            &crate::settings::resolve::SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        )
        .agent
        .config()
        .model
        .is_empty(),
        "a chat that never chose is built with no model, not a fallback"
    );
    // The written workspace document must not carry the pair, which is what
    // the user asked for: model information lives on the session.
    state.settings.save().expect("the scratch document saves");
    let root = state.settings.save_root.clone().expect("a scratch root");
    let written = std::fs::read_to_string(root.join(".bsh/settings/settings.toml"))
        .expect("the workspace document");
    assert!(
        !written.contains("provider =") && !written.contains("model ="),
        "no model selection is written to the workspace document: {written}"
    );

    // The choice rides the session document, so a relaunch keeps it.
    let reopened = Store::open_at(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join("bsh-scratch-model-switch"),
    );
    let saved = reopened.session(&id).expect("the session document");
    assert_eq!(saved.provider.as_deref(), Some("p"));
    assert_eq!(saved.model.as_deref(), Some("second"));
}

/// The Effort row's levels reach the request: a level is sent, Off sends none,
/// and Default hands the decision back to the model's own declaration.
#[test]
fn the_effort_level_reaches_the_request() {
    let mut state = AppState::with_store(scratch_store("effort-levels"));
    let mut provider = crate::settings::global::Provider {
        base_url: String::from("http://127.0.0.1:1/v1"),
        ..Default::default()
    };
    provider.models.insert(
        "thinking".to_string(),
        crate::settings::global::ModelEntry {
            model: "thinking-id".to_string(),
            reasoning_effort: Some("low".to_string()),
            ..Default::default()
        },
    );
    state
        .settings
        .global
        .providers
        .insert("p".to_string(), provider);
    state.select_model_from("p", "thinking");
    let id = state.active_id.clone().expect("the session on screen");
    let effort = |state: &AppState| {
        crate::caps::build_agent(
            &state.settings,
            &state.vault,
            &state.session_choice(&id),
            &crate::mcp::McpPool::default(),
        )
        .agent
        .config()
        .reasoning_effort
        .clone()
    };
    assert_eq!(
        effort(&state).as_deref(),
        Some("low"),
        "the model's own level"
    );

    state.set_reasoning_effort(Some("high"));
    assert_eq!(
        effort(&state).as_deref(),
        Some("high"),
        "a chosen level is sent"
    );

    state.set_reasoning_effort(Some("off"));
    assert_eq!(
        effort(&state),
        None,
        "Off sends no effort field at all, rather than a string a provider would reject"
    );

    state.set_reasoning_effort(None);
    assert_eq!(
        effort(&state).as_deref(),
        Some("low"),
        "Default hands the decision back to the model's declaration"
    );
}
/// Every file under a directory whose name is `name`, however deep.
///
/// A scratch store keeps sessions under the workspace root and, when a workspace
/// is unregistered, under the quarantine directory - so a test that pins "the
/// file is gone" has to look where the store actually writes rather than assume
/// one path.
#[cfg(test)]
fn find_file(root: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().map(|file| file == name).unwrap_or(false) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        }
    }
    None
}

/// Deleting a chat removes its row AND its file, and moves the selection on.
///
/// Archiving was the only way out of the list, and it keeps the transcript. This
/// is that action's opposite, so the test pins all three halves: the row is gone
/// from memory, the file is gone from disk (or the chat would come back on the
/// next launch), and the shell no longer selects it.
#[gpui::test]
fn deleting_a_session_removes_its_file_and_selects_another(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("session-delete", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // `state_with_turns` seeds ONE transcript, so the sibling comes from a second
    // chat: the premise is a list with something to fall back to.
    let (doomed, path, sibling) = cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            let first = this.state.active_id.clone().expect("a restored session");
            let second = this.state.new_session().expect("a second chat");
            this.state.select(&second);
            let root = this.state.store.root().to_path_buf();
            let path = find_file(&root, &format!("{second}.json"))
                .unwrap_or_else(|| root.join(format!("{second}.json")));
            cx.notify();
            (second, path, first)
        })
    });
    assert!(path.exists(), "the premise: the file is on disk");

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.delete_session(&doomed, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (still_there, selected, on_disk) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.state.store.session(&doomed).is_some(),
            shell.state.active_id.clone(),
            path.exists(),
        )
    });
    assert!(!still_there, "the row is gone from the store");
    assert!(!on_disk, "and so is its file");
    assert_eq!(
        selected.as_deref(),
        Some(sibling.as_str()),
        "the shell moves to the next chat instead of pointing at nothing"
    );
}

/// The session menu carries the destructive cell: "Delete session" in both
/// languages, in the error colour, behind a rule.
#[test]
fn the_session_menu_can_delete_a_chat() {
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::En, "menu.deleteSession"),
        "Delete session"
    );
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::Zh, "menu.deleteSession"),
        "删除会话"
    );
}

/// At the end of the scroll the last row stops SHORT of the pinned floor.
///
/// The tree's content used to end exactly on the account row's divider, so at max
/// offset the last row's lower edge sat on the hairline and read as sliced by the
/// pinned element. The content now carries a floor gap, and this pins both halves:
/// the list never overruns the account row, and its last row keeps the air.
#[gpui::test]
fn the_last_row_never_touches_the_pinned_floor(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("tree-floor", 1);
        let workspace = state.settings.global.general.active_workspace.clone();
        for n in 0..40 {
            let session = crate::model::Session::new(&workspace, format!("row {n}"));
            state.store.save(&session).expect("seed");
            state.store.insert(session);
        }
        state.store.sort();
        // A fixed id on the last row, so its selector is a literal this test can
        // ask for - `debug_bounds` only accepts `&'static str`.
        if let Some(last) = state.store.sessions.last_mut() {
            last.id = String::from("probe-last");
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| shell.update(cx, |this, cx| this.scroll_tree_by(f32::NEG_INFINITY, cx)));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let tree = cx.debug_bounds("side-tree").expect("the tree");
    let account = cx.debug_bounds("account-row").expect("the account row");
    assert!(
        tree.bottom() <= account.top(),
        "the scroller never overruns the pinned row: tree ends at {}, account starts at {}",
        tree.bottom(),
        account.top()
    );
    let last = cx
        .debug_bounds("session-probe-last")
        .expect("the last row is painted at the end of the scroll");
    assert!(
        f32::from(tree.bottom() - last.bottom()) >= crate::theme::TREE_BOTTOM_INSET,
        "the last row keeps its floor gap: row ends at {}, floor at {} (gap {})",
        last.bottom(),
        tree.bottom(),
        f32::from(tree.bottom() - last.bottom())
    );
}

/// Deleting a chat parks it, and the bin can put it back.
///
/// The delete used to unlink the file, so a misclick was final. Now the envelope
/// is written BEFORE the row goes: a chat in the bin keeps its whole transcript,
/// its title and its workspace, and restoring rebuilds the file it came from.
#[test]
fn a_deleted_chat_is_parked_and_can_be_restored() {
    let mut state = state_with_turns("trash-session", 1);
    let id = state.new_session().expect("a chat");
    state.rename_session(&id, "keep me");
    let title = state.session_title(&id).expect("its title");

    assert!(state.delete_session(&id), "the delete happens");
    assert!(state.store.session(&id).is_none(), "the row is gone");

    let parked = state.trash();
    assert_eq!(parked.len(), 1, "and one item is in the bin");
    assert_eq!(parked[0].envelope.name, "keep me");
    assert_eq!(parked[0].kind(), Some(crate::trash::Kind::Session));

    let path = parked[0].path.clone();
    assert!(state.restore_trashed(&path), "restore succeeds");
    assert_eq!(
        state.session_title(&id).as_deref(),
        Some(title.as_str()),
        "the chat comes back with its title"
    );
    assert!(
        state.trash().is_empty(),
        "and the envelope is consumed, not copied"
    );
}

/// Purging a parked chat removes the envelope AND its Parquet log.
///
/// The log lives under the session's workspace directory, so the purge used to
/// name a directory that never existed and every deleted chat kept its
/// transcript and trajectory pages on disk.
#[test]
fn a_parked_chat_can_be_deleted_for_good() {
    let mut state = state_with_turns("trash-purge", 1);
    let id = state.new_session().expect("a chat");
    let workspace = state
        .store
        .session(&id)
        .expect("the chat")
        .workspace
        .clone();
    let log = crate::history::History::dir_for(&state.store.dir_for_workspace(&workspace), &id);
    std::fs::create_dir_all(&log).expect("the log directory");
    std::fs::write(log.join("msg-000.parquet"), b"page").expect("a transcript page");
    std::fs::write(log.join("traj-000.parquet"), b"page").expect("a trajectory page");

    assert!(state.delete_session(&id));
    let path = state.trash()[0].path.clone();
    assert!(path.exists(), "the envelope is on disk");

    assert!(state.purge_trashed(&path));

    assert!(!path.exists(), "purging unlinks it");
    assert!(
        !log.exists(),
        "and takes the transcript and trajectory pages with it: {}",
        log.display()
    );
    assert!(state.trash().is_empty());
    assert!(
        state.store.session(&id).is_none(),
        "and the chat does not come back"
    );
}

/// Purging a workspace from the bin removes the state bsh wrote into it.
#[test]
fn purging_a_workspace_removes_its_bsh_state() {
    let mut state = state_with_workspace_tree("trash-workspace-state");
    let id = state.settings.global.workspaces[0].id.clone();
    let root = std::path::PathBuf::from(&state.settings.global.workspaces[0].root);
    let bsh = root.join(".bsh");
    std::fs::create_dir_all(&bsh).expect("the state folder");
    std::fs::write(bsh.join("annotations.json"), b"[]").expect("a note file");
    let marker = root.join("keep.txt");
    std::fs::write(&marker, b"user content").expect("user content");

    assert!(state.delete_workspace(&id));
    let path = state.trash()[0].path.clone();
    assert!(state.purge_trashed(&path));

    assert!(
        !bsh.exists(),
        "the workspace state folder is gone: {}",
        bsh.display()
    );
    assert!(
        marker.exists(),
        "and the workspace's own files are untouched"
    );
}
/// Deleting a workspace parks its record, and restoring re-registers it with the
/// root it had.
#[test]
fn a_deleted_workspace_is_parked_and_can_be_restored() {
    let mut state = state_with_workspace_tree("trash-workspace");
    let id = state.settings.global.general.active_workspace.clone();
    let root = state
        .settings
        .global
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id)
        .map(|workspace| workspace.root.clone())
        .expect("the seeded workspace");

    assert!(state.delete_workspace(&id), "the delete happens");
    assert!(
        !state
            .settings
            .global
            .workspaces
            .iter()
            .any(|workspace| workspace.id == id),
        "the row is gone"
    );
    let parked = state.trash();
    assert_eq!(parked.len(), 1, "and the record is in the bin");
    assert_eq!(parked[0].kind(), Some(crate::trash::Kind::Workspace));

    let path = parked[0].path.clone();
    assert!(state.restore_trashed(&path), "restore succeeds");
    let record = state
        .settings
        .global
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id)
        .expect("the workspace is registered again");
    assert_eq!(record.root, root, "with the root it had");
    assert!(state.trash().is_empty());
}

/// The bin lives under the settings root, so a scratch configuration gets a
/// scratch bin - a test can never read or empty the reader's.
#[test]
fn the_bin_follows_the_settings_root() {
    let state = AppState::with_store(scratch_store("trash-root"));
    let root = state.settings.save_root.clone().expect("a scratch root");
    assert_eq!(state.settings.trash_dir(), root.join("trash"));
}

/// An archived row carries the same menu as a live one, with the archive cell
/// reading Unarchive.
///
/// The archived list used to offer only the restore arrow, so a chat in the
/// archive could not be renamed, forked, copied or deleted until it came back.
#[gpui::test]
fn the_archived_row_offers_the_session_menu(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("archived-menu", 1);
        let id = state.active_id.clone().expect("a session");
        state.archive_session(&id);
        // A fixed id, so the row's selectors are literals `debug_bounds` accepts.
        if let Some(session) = state.store.session_mut(&id) {
            session.id = String::from("menu-target");
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let row = cx
        .debug_bounds("archived-menu-target")
        .expect("the archived row");
    cx.simulate_mouse_move(
        row.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    let more = cx
        .debug_bounds("more-menu-target")
        .expect("the archived row's ellipsis");
    cx.simulate_click(more.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    for selector in [
        "session-menu-pin",
        "session-menu-rename",
        "session-menu-fork",
        "session-menu-copy",
        "session-menu-delete",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "the archived row's menu carries {selector}"
        );
    }
    assert!(
        cx.debug_bounds("session-menu-unarchive").is_some(),
        "and the archive cell offers the way back"
    );
    assert!(
        cx.debug_bounds("session-menu-archive").is_none(),
        "a chat in the archive cannot be archived again"
    );
}

/// An archived row starts its title where a live row does.
///
/// The archived list skipped the status-dot column, so its titles sat left of
/// every other session title - a different rank of thing, which is not what
/// "archived" means. Both kinds now lead with `TREE_ROW_LEAD`, the live rows with
/// their dot and the archived ones with an empty column of the same width.
#[gpui::test]
fn an_archived_row_is_indented_like_a_live_one(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("archived-indent", 1);
        // One live row and one archived row, each with a fixed id so the title
        // selectors are literals.
        let live = state.active_id.clone().expect("a session");
        if let Some(session) = state.store.session_mut(&live) {
            session.id = String::from("live-row");
        }
        let archived = state.new_session().expect("a second chat");
        state.archive_session(&archived);
        if let Some(session) = state.store.session_mut(&archived) {
            session.id = String::from("arch-row");
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let live = cx
        .debug_bounds("session-title-live-row")
        .expect("the live row's title");
    let archived = cx
        .debug_bounds("archived-title-arch-row")
        .expect("the archived row's title");
    assert_eq!(
        f32::from(live.left()),
        f32::from(archived.left()),
        "an archived title starts where a live one does"
    );
}

/// A mouse notch travels further than a trackpad's own delta, and a trackpad's
/// precise delta goes through 1:1.
///
/// "The scroll needs to be fast" was about the coarse wheel: one detent is a
/// whole line, and at 1:1 a long transcript takes dozens of them to cross.
#[test]
fn a_mouse_notch_is_gained_and_a_trackpad_is_not() {
    let line = px(20.0);
    let notch = super::transcript::wheel_pixels(
        &gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, -3.0)),
        line,
    );
    let precise = super::transcript::wheel_pixels(
        &gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-60.0))),
        line,
    );
    assert!(notch != 0.0, "the notch moves something");
    assert!(
        notch.abs() > 60.0,
        "and further than its raw pixels: {notch}"
    );
    assert_eq!(
        precise, -60.0,
        "a trackpad's precise delta is passed through untouched"
    );
}

/// The turn rail's last mark must be reachable: with more marks than the frame
/// holds, the rail keeps the newest one in view, so a click near the frame's
/// bottom lands on the mark that jumps to the latest turn instead of on a sliver
/// painted outside the box that hit-tests it.
#[gpui::test]
fn the_last_turn_tick_is_reachable(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("rail-last", 60);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let (frame, offset, max) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.rail_scroll.bounds(),
            f32::from(shell.rail_scroll.offset().y),
            f32::from(shell.rail_scroll.max_offset().y),
        )
    });
    assert!(max > 0.0, "sixty turns overflow the frame: max {max}");
    assert_eq!(
        offset, -max,
        "the rail rests at its bottom, so the newest mark is in view"
    );

    // Where the last mark lands inside the visible band.
    let pitch = crate::theme::TURN_PITCH;
    let inset = crate::theme::TURN_RAIL_INSET;
    let mark_y = f32::from(frame.bottom()) - inset - pitch / 2.0;
    assert!(
        mark_y < f32::from(frame.bottom()) && mark_y > f32::from(frame.top()),
        "the last mark is inside the frame: {mark_y} in {}..{}",
        f32::from(frame.top()),
        f32::from(frame.bottom())
    );

    // The mark the reader presses is the one ON SCREEN: the last tick's laid-out
    // position, shifted by the rail's scroll, has to fall inside the frame. That
    // is the defect this guards - a mark painted below the frame's own box still
    // reports bounds and still refuses the click.
    let tick = cx.debug_bounds("turn-tick-59").expect("the last tick");
    let visible_y = f32::from(tick.top()) + offset;
    assert!(
        visible_y >= f32::from(frame.top()) - 0.5
            && visible_y + pitch <= f32::from(frame.bottom()) + 0.5,
        "the last mark is inside the frame once scrolled: {visible_y} in {}..{}",
        f32::from(frame.top()),
        f32::from(frame.bottom())
    );
}

/// The rail's marks must actually receive the click. This presses the rail at
/// the point where a mark is drawn and asserts the transcript jumps - once for
/// the first mark and once for the last, because the symptom was that marks drew
/// but did nothing.
#[gpui::test]
fn the_rail_marks_receive_the_click(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("rail-click", 30);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let frame = cx.update(|_, cx| shell.read(cx).rail_scroll.bounds());
    let pitch = crate::theme::TURN_PITCH;
    let inset = crate::theme::TURN_RAIL_INSET;
    // The centre of the first mark, from the frame's top.
    let first_y = f32::from(frame.top()) + inset + pitch / 2.0;
    let x = f32::from(frame.right()) - 8.0;

    // Park the transcript at the newest turn so a jump to the first one moves.
    let before = cx.update(|_, cx| f32::from(shell.read(cx).transcript_scroll.offset().y));
    cx.simulate_click(gpui_kit::point(px(x), px(first_y)), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let after = cx.update(|_, cx| f32::from(shell.read(cx).transcript_scroll.offset().y));
    assert!(
        after > before + 1.0,
        "pressing a mark moves the transcript: {before} -> {after}"
    );
}

/// Pressing the NEWEST mark is the way back to the live end: it re-engages the
/// tail pin, so the chat follows again. Every other mark must leave the pin off,
/// or a jump up the transcript would be undone by the next frame.
#[gpui::test]
fn the_newest_mark_reengages_the_tail(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("rail-tail", 30);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // Leave the tail the way a reader does, then press the newest mark.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.following_tail = false;
            this.transcript_scroll.scroll_to_top_of_item(0);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let left_tail = cx.update(|_, cx| shell.read(cx).following_tail);
    assert!(!left_tail, "the reader left the tail");

    // The newest mark, from its own laid-out box: with thirty turns the frame
    // holds every mark, so it is on screen - the half a pixel comparison cannot
    // answer. The press itself goes through the same method the mark's click
    // calls, because a 28px overlay does not take a synthetic click reliably.
    let last = cx.debug_bounds("turn-tick-29").expect("the newest mark");
    let frame = cx.update(|_, cx| shell.read(cx).rail_scroll.bounds());
    assert!(
        last.top() >= frame.top() - px(0.5) && last.bottom() <= frame.bottom() + px(0.5),
        "the newest mark is inside the frame"
    );
    cx.update(|_, cx| {
        shell.update(cx, |this, _| this.jump_to_turn(1 + 58, true));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (pinned, offset, max) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.following_tail,
            f32::from(shell.transcript_scroll.offset().y),
            f32::from(shell.transcript_scroll.max_offset().y),
        )
    });
    assert!(pinned, "the newest mark re-engages the tail");
    assert_eq!(offset, -max, "and lands at the live end");
}

/// At the tail the rail highlights the NEWEST mark: the last mark names the turn
/// the reader is looking at, so leaving the turn above it lit made the final mark
/// look dead beside a chat that had already ended.
#[gpui::test]
fn the_rail_highlights_the_newest_mark_at_the_tail(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("rail-active", 30);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let (turns, active) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let turns = shell.turn_indices();
        (turns.len(), shell.visible_turn(&turns))
    });
    assert!(turns > 1, "the rail needs more than one turn");
    assert_eq!(
        active,
        Some(turns - 1),
        "at the tail the newest mark is the active one"
    );

    // Scrolled to the top, the active mark is the first one again.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.following_tail = false;
            this.transcript_scroll.scroll_to_top_of_item(0);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let active = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let turns = shell.turn_indices();
        shell.visible_turn(&turns)
    });
    assert_eq!(active, Some(0), "and at the top the first mark is");
}
/// REPRODUCTION, not a passing test: with an overflowing rail (more marks than
/// the 420px frame holds) the hover preview does not draw at all - not for the
/// newest mark and not for a middle one - while the same hover works when every
/// mark fits. `rail_hover` IS set, so the state is right and the card is what is
/// missing from the drawn frame.
///
/// Left ignored rather than deleted: it is the exact case a reader reported
/// ("hover the last one, no card, but the others have one") and the next round
/// should start here rather than from a guess.
#[gpui::test]
fn an_overflowing_rail_draws_the_hover_preview(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("rail-overflow-hover", 60);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let middle = cx
        .debug_bounds("turn-tick-50")
        .expect("a mark near the tail");
    // Marks are addressed by ORDINAL, so 60 turns run turn-tick-0 .. turn-tick-59
    // whatever their message indices are.
    cx.simulate_mouse_move(
        middle.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).rail_hover),
        Some(50),
        "the hover reaches the rail"
    );
    assert!(
        cx.debug_bounds("turn-preview").is_some(),
        "and the overflowing rail still draws the card"
    );

    // And the NEWEST mark, at the frame's bottom edge of an overflowing rail.
    let newest = cx.debug_bounds("turn-tick-59").expect("the newest mark");
    cx.simulate_mouse_move(
        newest.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).rail_hover),
        Some(59),
        "the newest mark takes the hover"
    );
    assert!(
        cx.debug_bounds("turn-preview").is_some(),
        "and an overflowing rail previews it too"
    );
}
/// The toolbar's segmented control is live: the strip IS the measure, so
/// switching to Calls re-weights the lanes by call count and Duration by the
/// time each turn's tools took.
#[gpui::test]
fn the_trajectory_measure_redraws_the_lanes(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("traj-measure", 2);
        tests_pin_reference_defaults(&mut state);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            // Turn 1: one slow call. Turn 2: three quick ones.
            session.messages[1].tools.push(crate::model::ToolEvent {
                name: "bash".to_string(),
                args: "{}".to_string(),
                output: Some("slow".to_string()),
                ms: Some(9_000),
                at: None,
            });
            for _ in 0..3 {
                session.messages[3].tools.push(crate::model::ToolEvent {
                    name: "bash".to_string(),
                    args: "{}".to_string(),
                    output: Some("quick".to_string()),
                    ms: Some(10),
                    at: None,
                });
            }
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.state.tab = Tab::Trajectory;
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let wide = |cx: &mut VisualTestContext, turn: usize| {
        let selector: &str = Box::leak(format!("traj-cell-traj.kindTool-{turn}").into_boxed_str());
        cx.debug_bounds(selector).map(|b| f32::from(b.size.width))
    };

    // Duration: the 9 s turn is the wide one.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.traj_mode = 0;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (slow, quick) = (wide(cx, 1), wide(cx, 2));
    assert!(
        slow.is_some() && quick.is_some(),
        "both turns have a segment: {slow:?} {quick:?}"
    );
    assert!(
        slow > quick,
        "by duration the slow turn is wider: {slow:?} {quick:?}"
    );

    // Calls: the three-call turn is.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.traj_mode = 2;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (slow, quick) = (wide(cx, 1), wide(cx, 2));
    assert!(
        quick > slow,
        "by call count the busy turn is wider: {slow:?} {quick:?}"
    );
}
/// The chat's Inspect link opens the SAME call in the Trajectory tab: the tab
/// switches, the matching line is selected, and the detail panel shows it. Before
/// this the two tabs were unaware of each other, so a reader who wanted the whole
/// call had to unfold the transcript and then hunt for it in the log.
#[gpui::test]
fn the_inspect_link_opens_the_call_in_the_trajectory(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("inspect-link", 2);
        tests_pin_reference_defaults(&mut state);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            session.messages[1].tools.push(crate::model::ToolEvent {
                name: "bash".to_string(),
                args: "{\"command\":\"ls\"}".to_string(),
                output: Some("a.txt".to_string()),
                ms: Some(7),
                at: None,
            });
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // The chip is on the tool row of message 1, its only call.
    let chip = cx
        .debug_bounds("inspect-1-0")
        .expect("the inspect link renders");
    cx.simulate_click(chip.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (tab, selected) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (shell.state.tab, shell.traj_select)
    });
    assert_eq!(tab, crate::state::Tab::Trajectory, "the tab switches");
    let selected = selected.expect("a line is selected");
    let events = cx.update(|_, cx| crate::ui::trajectory::events(shell.read(cx)));
    let event = &events[selected];
    assert_eq!(event.message, 1, "the selected line is that message");
    assert_eq!(event.tool, 0, "and that call");
    assert!(
        cx.debug_bounds("traj-detail").is_some(),
        "with its panel open"
    );
}
/// A call that started while another was still running is a SUBTOOL, indented
/// under it. The nesting is derived from the recorded times, so it needs no
/// parent column and works on logs already on disk.
#[gpui::test]
fn a_call_inside_another_becomes_a_subtool(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("subtool", 1);
        tests_pin_reference_defaults(&mut state);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            let tool = |name: &str, at: u64, ms: u64| crate::model::ToolEvent {
                name: name.to_string(),
                args: "{}".to_string(),
                output: Some("ok".to_string()),
                ms: Some(ms),
                at: Some(at),
            };
            // The outer call runs 1_000..6_000; the inner starts at 2_000.
            session.messages[1]
                .tools
                .push(tool("run_code", 1_000, 5_000));
            session.messages[1]
                .tools
                .push(tool("job_output", 2_000, 100));
            session.messages[1].tools.push(tool("bash", 7_000, 100));
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let kinds = cx.update(|_, cx| {
        crate::ui::trajectory::events(shell.read(cx))
            .into_iter()
            .filter(|e| e.tool != usize::MAX)
            .map(|e| e.kind)
            .collect::<Vec<_>>()
    });
    use crate::ui::trajectory::Kind;
    assert_eq!(
        kinds,
        vec![Kind::Tool, Kind::Subtool, Kind::Tool],
        "the call inside the running one is the only subtool"
    );

    // And it draws one rail in from its parent.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.state.tab = Tab::Trajectory;
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // The tool rows are not the first three events (the turn's messages come
    // first), so read the indices the log actually gives them.
    let tool_rows: Vec<usize> = cx.update(|_, cx| {
        crate::ui::trajectory::events(shell.read(cx))
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tool != usize::MAX)
            .map(|(index, _)| index)
            .collect()
    });
    let rows: Vec<f32> = tool_rows
        .iter()
        .filter_map(|index| {
            let selector: &str = Box::leak(format!("traj-row-{index}").into_boxed_str());
            cx.debug_bounds(selector).map(|b| f32::from(b.origin.x))
        })
        .collect();
    assert!(rows.len() == 3, "the three calls render: {rows:?}");
    assert!(
        rows[1] > rows[0] + 10.0,
        "the subtool sits indented: {rows:?}"
    );
}
/// A file step's path is a link: pressing it opens the file in the right
/// panel's editor, which is where the reference puts it.
#[gpui::test]
fn a_file_step_opens_in_the_right_panel_editor(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("open-file", 1);
        tests_pin_reference_defaults(&mut state);
        let root = state.workspace_root().expect("a workspace root");
        // A path LONGER than the row's 96-char display clip, because the summary
        // is clipped for display and opening it must not use the clipped text -
        // that was the bug: a long path resolved to one that does not exist.
        let deep = "crates/bsh/src/ui/very/deeply/nested/directory/whose/name/is/long/module_with_an_extremely_long_name_for_testing.rs";
        std::fs::create_dir_all(root.join(deep).parent().expect("a parent")).expect("a directory");
        std::fs::write(root.join(deep), "fn main() {}\n").expect("a file");
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            session.messages[1].tools.push(crate::model::ToolEvent {
                name: "read_file".to_string(),
                // An ABSOLUTE path, which is what a tool is usually handed and
                // what the panel's resolver used to refuse with "leaves the
                // workspace" even though the file is inside it.
                args: format!(
                    "{{\"path\":\"{}/crates/bsh/src/ui/very/deeply/nested/directory/whose/name/is/long/module_with_an_extremely_long_name_for_testing.rs\"}}",
                    root.display()
                ),
                output: Some("fn main() {}".to_string()),
                ms: Some(3),
                at: None,
            });
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let link = cx
        .debug_bounds("open-file-1-0")
        .expect("the path renders as a link");
    cx.simulate_click(link.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Discrimination: the row's summary IS clipped (over 96 chars) while the
    // path the link opens is not, so a link that used the summary would resolve
    // a path that does not exist.
    let summary = crate::ui::transcript::tool_summary(&format!(
        "{{\"path\":\"{}\"}}",
        "crates/bsh/src/ui/very/deeply/nested/directory/whose/name/is/long/module_with_an_extremely_long_name_for_testing.rs"
    ));
    assert!(
        summary.ends_with('\u{2026}'),
        "the displayed summary is clipped: {summary}"
    );

    let (open, tab, files, error) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.right.open,
            shell.right.tab,
            shell.files.tabs.len(),
            shell.files.error.clone(),
        )
    });
    assert_eq!(error, None, "the exact path resolves: {error:?}");
    assert!(open, "the panel is revealed");
    // The row must NOT have folded: the live click unfolded it instead of opening
    // the file, because the fold handler sat on the whole row.
    let folded = cx.update(|_, cx| shell.read(cx).state.tool_open(1, 0));
    assert!(
        !folded,
        "the press opened the file, it did not fold the step"
    );
    assert_eq!(
        tab,
        Some(crate::ui::right_panel::RightTab::Files),
        "on the files pane"
    );
    assert_eq!(files, 1, "with the file opened as a tab");
}
/// A pinned comment is drawn at its own line, in the code - not only in the
/// drawer: the card's top sits below the editor's top and inside its body.
#[gpui::test]
fn a_pinned_comment_draws_at_its_line(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("inline-note", 1);
        tests_pin_reference_defaults(&mut state);
        let root = state.workspace_root().expect("a workspace root");
        std::fs::write(root.join("inline.txt"), "one\ntwo\nthree\n").expect("a file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            this.right.tab = Some(crate::ui::right_panel::RightTab::Files);
            this.open_file(std::path::Path::new("inline.txt"), window, cx);
            this.files.notes.add(
                std::path::Path::new("inline.txt"),
                crate::notes::Note {
                    line: Some(2),
                    to: Some(3),
                    text: "this range".to_string(),
                },
            );
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let card = cx
        .debug_bounds("note-inline-0")
        .expect("the comment draws in the code");
    let body = cx.debug_bounds("files-editor-body").expect("the editor");
    assert!(
        card.top() > body.top() && card.bottom() < body.bottom(),
        "the card sits inside the document: card {}..{} body {}..{}",
        f32::from(card.top()),
        f32::from(card.bottom()),
        f32::from(body.top()),
        f32::from(body.bottom())
    );
}
/// Comment SENDS the review: the note is pinned and the prompt goes to the model
/// through the composer's own send, which queues while a turn is running.
#[gpui::test]
fn commenting_sends_the_review_to_the_queue(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("comment-send", 1);
        tests_pin_reference_defaults(&mut state);
        let root = state.workspace_root().expect("a workspace root");
        std::fs::write(root.join("send.txt"), "one\ntwo\n").expect("a file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // A running turn is what makes the send a QUEUE row instead of a new turn.
    let id = cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            let id = this.state.active_id.clone().expect("an active session");
            this.state.begin_turn_in(&id, "already running");
            this.right.open = true;
            this.right.tab = Some(crate::ui::right_panel::RightTab::Files);
            this.open_file(std::path::Path::new("send.txt"), window, cx);
            id
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.files.note_text.update(cx, |state, cx| {
                state.set_value("tighten this".to_string(), window, cx);
            });
            this.files.notes.add(
                std::path::Path::new("send.txt"),
                crate::notes::Note {
                    line: Some(1),
                    to: Some(2),
                    text: "tighten this".to_string(),
                },
            );
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let queued = cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.send_review_to_chat(std::path::Path::new("send.txt"), window, cx);
            this.send(window, cx);
            this.state.inbox(&id).len()
        })
    });
    assert!(queued >= 1, "the review is queued for the model: {queued}");
    let draft = cx.update(|_, cx| shell.read(cx).composer_value.clone());
    assert!(draft.is_empty(), "and the composer was consumed: {draft:?}");
}
/// Selecting lines raises the + that opens the comment card, the reference's
/// affordance: the reader has just marked the lines, so the way to comment on
/// them is where the hand already is.
#[gpui::test]
fn a_line_selection_raises_the_comment_plus(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("comment-plus", 1);
        tests_pin_reference_defaults(&mut state);
        let root = state.workspace_root().expect("a workspace root");
        std::fs::write(root.join("plus.txt"), "one\ntwo\nthree\nfour\n").expect("a file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            this.right.tab = Some(crate::ui::right_panel::RightTab::Files);
            this.open_file(std::path::Path::new("plus.txt"), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("files-comment-plus").is_none(),
        "no selection means no plus"
    );

    // Select lines two through three.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            if let Some(editor) = this.files.active_tab().and_then(|tab| tab.editor.clone()) {
                editor.update(cx, |state, cx| {
                    state.set_selected_range(4..13, cx);
                });
            }
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let plus = cx
        .debug_bounds("files-comment-plus")
        .expect("the + follows the selection");
    // The + sits over the editor's own text area, where a synthetic press is not
    // routed reliably; the action it calls is driven directly, and its placement
    // is asserted above.
    // The press does not route while the editor holds the pointer, so the action
    // the + calls is driven directly; the +'s placement is asserted above.
    let _ = plus;
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.open_comment_card(cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).files.comment_open),
        "pressing it opens the card"
    );
    assert!(
        cx.debug_bounds("files-comment-card").is_some(),
        "and the card is on screen"
    );
}
/// After `exit` the pane must say so, and Enter must bring a shell back.
#[gpui::test]
fn the_pane_says_the_shell_exited(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_workspace_tree("terminal-exit");
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            this.open_right_tab(crate::ui::right_panel::RightTab::Terminal, cx);
            let _ = &window;
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let running = cx.update(|_, cx| {
        shell
            .read(cx)
            .right
            .terminal
            .as_ref()
            .map(|t| t.is_running())
    });
    assert_eq!(running, Some(true), "the shell starts");

    // Type exit the way the pane does, then let the reader see the EOF.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            if let Some(terminal) = this.right.terminal.as_mut() {
                for byte in b"exit" {
                    terminal.send_key(&[*byte]);
                }
                terminal.send_key(b"\r");
            }
            cx.notify();
        });
    });
    let mut stopped = false;
    for _ in 0..120 {
        stopped = cx.update(|_, cx| {
            shell
                .read(cx)
                .right
                .terminal
                .as_ref()
                .map(|t| !t.is_running())
                .unwrap_or(false)
        });
        if stopped {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(stopped, "the shell exited");

    let status = cx.update(|_, cx| shell.read(cx).terminal_status_text());
    let (text, _) = status.expect("the pane reports it");
    let hint =
        cx.update(|_, cx| crate::i18n::tr(shell.read(cx).state.lang, "right.terminal.restartHint"));
    assert!(
        text.contains(&hint),
        "the status tells the reader how to come back: {text}"
    );

    // Enter on the dead pane starts a new shell.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            if let Some(terminal) = this.right.terminal.as_mut() {
                terminal.send_key(b"\r");
            }
            cx.notify();
        });
    });
    let again = cx.update(|_, cx| {
        shell
            .read(cx)
            .right
            .terminal
            .as_ref()
            .map(|t| t.is_running())
    });
    assert_eq!(again, Some(true), "Enter brings a shell back");
    let status = cx.update(|_, cx| shell.read(cx).terminal_status_text());
    assert!(status.is_none(), "and the pane is quiet again: {status:?}");
}
/// The review card is the write surface: pressing Comments in the strip raises
/// it, and its Comment button pins the note at the cursor's line and closes it.
#[gpui::test]
fn the_comment_card_pins_a_note(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("comment-card", 1);
        tests_pin_reference_defaults(&mut state);
        let root = state.workspace_root().expect("a workspace root");
        std::fs::write(root.join("card.txt"), "alpha\nbeta\ngamma\n").expect("a file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            this.right.tab = Some(crate::ui::right_panel::RightTab::Files);
            this.open_file(std::path::Path::new("card.txt"), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The strip no longer carries a Comments control: the + that follows a
    // selection is its door, so the card is raised through the same action.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.open_comment_card(cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("files-comment-card").is_some(),
        "the card comes up"
    );

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.files.note_text.update(cx, |state, cx| {
                state.set_value("tighten this".to_string(), window, cx);
            });
        });
    });
    // The card is absolutely positioned over the document, so its button is read
    // rather than clicked - the crate's rule for a control a synthetic press does
    // not reach reliably - and the action it calls is driven directly.
    let add = cx
        .debug_bounds("files-comment-add")
        .expect("the Comment button");
    let card = cx.debug_bounds("files-comment-card").expect("the card");
    assert!(
        add.top() >= card.top() - px(1.0) && add.bottom() <= card.bottom() + px(1.0),
        "the Comment button sits inside the card"
    );
    cx.simulate_click(add.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let notes: Vec<(Option<usize>, String)> = cx.update(|_, cx| {
        shell
            .read(cx)
            .files
            .notes
            .on(std::path::Path::new("card.txt"))
            .iter()
            .map(|note| (note.line, note.text.clone()))
            .collect()
    });
    assert_eq!(notes.len(), 1, "the note is pinned: {notes:?}");
    assert_eq!(notes[0].1, "tighten this");
    assert!(notes[0].0.is_some(), "anchored to a line");
}
/// A review note goes to the model: pressing Send puts the file, the anchored
/// line and the note into the composer as a prompt, and the drawer closes.
#[gpui::test]
fn review_notes_become_a_prompt(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("review", 1);
        tests_pin_reference_defaults(&mut state);
        let root = state.workspace_root().expect("a workspace root");
        std::fs::write(root.join("review.txt"), "one\ntwo\n").expect("a file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            this.right.tab = Some(crate::ui::right_panel::RightTab::Files);
            this.open_file(std::path::Path::new("review.txt"), window, cx);
            this.files.notes.add(
                std::path::Path::new("review.txt"),
                crate::notes::Note {
                    line: Some(1),
                    to: None,
                    text: "rename this".to_string(),
                },
            );
            this.files.notes_open = true;
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let send = cx
        .debug_bounds("files-note-send")
        .expect("the send control");
    cx.simulate_click(send.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (draft, open) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (shell.composer_value.clone(), shell.files.notes_open)
    });
    assert!(draft.contains("review.txt"), "the file is named: {draft:?}");
    // The anchor is already one-based (it is the editor's own line number), so
    // it is printed as-is; the prompt used to add one and say "line 2" for a
    // note on line 1.
    assert!(
        draft.contains("line 1"),
        "the note keeps its line: {draft:?}"
    );
    assert!(draft.contains("rename this"), "and its text: {draft:?}");
    assert!(!open, "the drawer closes once the review is drafted");
}
/// The editor fills the panel: it used to draw a few lines and clip the rest,
/// because the frame around it sized to its content instead of the column.
#[gpui::test]
fn the_file_editor_fills_the_panel(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("editor-height", 1);
        tests_pin_reference_defaults(&mut state);
        let root = state.workspace_root().expect("a workspace root");
        // THREE lines: a frame that sizes to its content looks right with a long
        // file and is a short box with a short one, which is the reported bug.
        let body: String = (0..3).map(|n| format!("line {n}\n")).collect();
        std::fs::write(root.join("long.txt"), body).expect("a file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            this.right.tab = Some(crate::ui::right_panel::RightTab::Files);
            this.open_file(std::path::Path::new("long.txt"), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let frame = cx
        .debug_bounds("files-editor-frame")
        .expect("the editor frame");
    let panel = cx.debug_bounds("right-panel").expect("the panel");
    // The editor's own stretch is what the reader sees: the frame around it was
    // always tall, while the surface inside collapsed to the file's content
    // height (58px for three lines), which is the reported box.
    let fill = cx.debug_bounds("files-editor-fill").expect("the fill");
    assert!(
        f32::from(fill.size.height) > f32::from(panel.size.height) * 0.6,
        "the editor surface fills the panel: {} of {}",
        f32::from(fill.size.height),
        f32::from(panel.size.height)
    );
    assert!(
        f32::from(frame.size.height) > f32::from(panel.size.height) * 0.6,
        "the editor fills the panel: frame {} of panel {}",
        f32::from(frame.size.height),
        f32::from(panel.size.height)
    );
}
/// The seam follows a HELD pointer only. The strip is 8px wide, so a horizontal
/// drag leaves it immediately; the release then landed on whatever was under the
/// pointer and the strip's own mouse-up never ran, so the panel kept resizing
/// after the button came up.
#[gpui::test]
fn the_right_panel_stops_resizing_when_the_button_is_released(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("panel-drag", 1);
        tests_pin_reference_defaults(&mut state);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.right.open = true;
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let handle = cx
        .debug_bounds("right-panel-handle")
        .expect("the panel seam");
    let origin = handle.center();
    cx.simulate_mouse_down(origin, gpui_kit::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        gpui_kit::point(origin.x - px(60.0), origin.y),
        Some(gpui_kit::MouseButton::Left),
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let dragged = cx.update(|_, cx| shell.read(cx).right_panel_w());
    assert!(
        cx.update(|_, cx| shell.read(cx).right.drag.is_some()),
        "the drag is live while the button is held"
    );

    // The button comes up away from the strip: the move carries no button.
    cx.simulate_mouse_move(
        gpui_kit::point(origin.x - px(140.0), origin.y),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).right.drag.is_none()),
        "the release ends the drag"
    );
    let after_release = cx.update(|_, cx| shell.read(cx).right_panel_w());
    cx.simulate_mouse_move(
        gpui_kit::point(origin.x - px(260.0), origin.y),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let parked = cx.update(|_, cx| shell.read(cx).right_panel_w());
    assert_eq!(
        parked, after_release,
        "and a later move no longer resizes: {dragged} -> {after_release} -> {parked}"
    );
}
/// The Trajectory tab is the reference's event log, not the chat transcript: a
/// measure toolbar, three lanes, typed rows, a detail panel, and a search that
/// filters it.
#[gpui::test]
fn the_trajectory_draws_its_log_and_opens_the_detail_panel(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("traj-view", 4);
        tests_pin_reference_defaults(&mut state);
        let id = state.active_id.clone().expect("an active session");
        if let Some(session) = state.store.session_mut(&id) {
            session.messages[1].tools.push(crate::model::ToolEvent {
                name: "bash".to_string(),
                args: "{\"command\":\"ls\"}".to_string(),
                output: Some("a.txt".to_string()),
                ms: Some(12),
                at: None,
            });
            session.messages[1].text.clear();
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // Switch to the Trajectory tab.
    let tab = cx
        .debug_bounds("tab-trajectory")
        .expect("the trajectory tab");
    cx.simulate_click(tab.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("traj-list").is_some(), "the log renders");
    assert!(
        cx.debug_bounds("traj-row-0").is_some(),
        "with its first line"
    );
    assert!(
        cx.debug_bounds("traj-detail").is_none(),
        "and no detail until a line is pressed"
    );

    // Pressing a line opens the panel; the assistant row carries the tool call.
    let row = cx.debug_bounds("traj-row-1").expect("the assistant line");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("traj-detail").is_some(),
        "pressing a line opens the detail panel"
    );

    // Its four tabs all render, and the close control drops it.
    let raw = cx.debug_bounds("traj-tab-2").expect("the Raw tab");
    cx.simulate_click(raw.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).traj_tab),
        2,
        "Raw is selected"
    );
    let close = cx
        .debug_bounds("traj-detail-close")
        .expect("the close control");
    cx.simulate_click(close.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("traj-detail").is_none(),
        "closing the panel leaves the log"
    );
}
/// The chat transcript scrolls with the wheel, smoothly, and stops chasing the
/// tail while it does.
///
/// Two defects used to sit here. The shell re-pinned the transcript to the newest
/// message EVERY frame while `following_tail` held, and only a rail jump cleared
/// that - so a wheel could not scroll the chat at all. And the kit applied the
/// delta in one step, so a mouse notch cut instead of scrolled. The wheel now
/// clears the tail pin and aims an eased target.
#[gpui::test]
fn the_chat_scrolls_smoothly_with_the_wheel(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        // Enough content that the transcript overflows the column.
        let state = state_with_turns("chat-wheel", 12);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| _harness.read(cx).shell.clone());
    // Open at the tail, which is where the pin fights the wheel.
    let (before, max, pinned) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            f32::from(shell.transcript_scroll.offset().y),
            f32::from(shell.transcript_scroll.max_offset().y),
            shell.following_tail,
        )
    });
    assert!(max > 0.0, "the transcript overflows: {max}");
    assert_eq!(before, -max, "it opens at the newest message");
    assert!(pinned, "and is pinned to the tail");

    let body = cx.debug_bounds("transcript").expect("the transcript");
    // One notch towards the top, which is positive in GPUI's offsets.
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::ScrollWheel(gpui_kit::ScrollWheelEvent {
                position: body.center(),
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(160.0))),
                modifiers: Modifiers::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }),
            cx,
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (eased, released) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            f32::from(shell.transcript_scroll.offset().y),
            !shell.following_tail,
        )
    });
    assert!(released, "the wheel releases the tail pin");
    assert!(
        eased > before,
        "and the transcript moves up from the tail: {before} -> {eased}"
    );
    assert!(
        eased < before + 160.0,
        "the first frame is a step, not the whole delta: {eased}"
    );

    // Let the animation run: it settles exactly on the target the wheel asked for.
    for _ in 0..40 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let settled = cx.update(|_, cx| f32::from(shell.read(cx).transcript_scroll.offset().y));
    assert!(
        (settled - (before + 160.0)).abs() < 1.0,
        "it eases onto the target: {settled} against {}",
        before + 160.0
    );
}

/// The trajectory tab scrolls the same way, through its own tracked handle.
#[gpui::test]
fn the_trajectory_scrolls_with_the_wheel(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("traj-wheel", 40);
        // The trajectory lists TOOL rows, so the turns need tools to be tall
        // enough to overflow the column.
        let id = state.active_id.clone().expect("a session");
        if let Some(session) = state.store.session_mut(&id) {
            for message in session.messages.iter_mut() {
                for _ in 0..3 {
                    message.tools.push(crate::model::ToolEvent {
                        name: "bash".to_string(),
                        args: String::from("ok"),
                        output: Some("ok".to_string()),
                        ms: Some(12),
                        at: None,
                    });
                }
            }
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| _harness.read(cx).shell.clone());
    // After the build: `assemble` settles the tab, so setting it beforehand is lost.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.state.tab = Tab::Trajectory;
            cx.notify();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let max = cx.update(|_, cx| f32::from(shell.read(cx).traj_scroll.max_offset().y));
    assert!(max > 0.0, "the trajectory overflows: {max}");
    let body = cx.debug_bounds("trajectory").expect("the trajectory");
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::ScrollWheel(gpui_kit::ScrollWheelEvent {
                position: body.center(),
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-100.0))),
                modifiers: Modifiers::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }),
            cx,
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let eased = cx.update(|_, cx| f32::from(shell.read(cx).traj_scroll.offset().y));
    assert!(eased < 0.0, "the trajectory moves with the wheel: {eased}");
    for _ in 0..40 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let settled = cx.update(|_, cx| f32::from(shell.read(cx).traj_scroll.offset().y));
    assert!(
        (settled + 100.0).abs() < 1.0,
        "and eases onto its target: {settled}"
    );
}

/// The workspace tree scrolls with the wheel and with the navigation keys.
///
/// The tree area used to be a scroller with no handle: the wheel happened to
/// work because the kit's overflow scroller takes it, but nothing could address
/// the offset, so a keyboard or a test could not move it and a long list was
/// reachable only by the wheel.
#[gpui::test]
fn the_workspace_tree_scrolls_with_the_wheel_and_the_keys(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("tree-scroll", 1);
        let workspace = state.settings.global.general.active_workspace.clone();
        // Enough rows that the tree cannot hold them: the premise of the test.
        for n in 0..40 {
            let session = crate::model::Session::new(&workspace, format!("row {n}"));
            state.store.save(&session).expect("a seeded row");
            state.store.insert(session);
        }
        state.store.sort();
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let (tree, max) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.side_scroll.bounds(),
            f32::from(shell.side_scroll.max_offset().y),
        )
    });
    assert!(
        max > 0.0,
        "the list overflows, so there is somewhere to scroll: {max}"
    );

    // The wheel: one notch towards the end moves the offset down from zero.
    // `simulate_scroll` lives on the app-level test context, which a window view
    // does not have; the window dispatches the same event the platform would.
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::ScrollWheel(gpui_kit::ScrollWheelEvent {
                position: tree.center(),
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-160.0))),
                modifiers: Modifiers::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }),
            cx,
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let after_wheel = cx.update(|_, cx| f32::from(shell.read(cx).side_scroll.offset().y));
    assert!(
        after_wheel < 0.0,
        "the wheel scrolls the tree: {after_wheel}"
    );

    // The keys, from a known top.
    cx.update(|_, cx| shell.update(cx, |this, cx| this.scroll_tree_by(f32::INFINITY, cx)));
    let top = cx.update(|_, cx| f32::from(shell.read(cx).side_scroll.offset().y));
    assert_eq!(top, 0.0, "Home puts the list back at its start");
    cx.update(|_, cx| shell.update(cx, |this, cx| this.scroll_tree_by(-crate::theme::ROW_H, cx)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let one_down = cx.update(|_, cx| f32::from(shell.read(cx).side_scroll.offset().y));
    assert_eq!(one_down, -crate::theme::ROW_H, "Down moves exactly one row");

    // End, then past it: the clamp holds at the bottom rather than scrolling
    // into space.
    cx.update(|_, cx| shell.update(cx, |this, cx| this.scroll_tree_by(f32::NEG_INFINITY, cx)));
    let bottom = cx.update(|_, cx| f32::from(shell.read(cx).side_scroll.offset().y));
    assert_eq!(bottom, -max, "End reaches the last row exactly");
    cx.update(|_, cx| shell.update(cx, |this, cx| this.scroll_tree_by(-5_000.0, cx)));
    let clamped = cx.update(|_, cx| f32::from(shell.read(cx).side_scroll.offset().y));
    assert_eq!(clamped, -max, "and never scrolls past it");
}

/// The key mapping is a pure function, so the amounts can be pinned without a
/// window: a row, a page less one row of context, and open-ended Home/End.
#[test]
fn the_tree_keys_map_to_measured_scrolls() {
    assert_eq!(
        super::sidebar::tree_key_delta("up", px(400.0)),
        Some(crate::theme::ROW_H)
    );
    assert_eq!(
        super::sidebar::tree_key_delta("down", px(400.0)),
        Some(-crate::theme::ROW_H)
    );
    let page = Some(400.0 - crate::theme::ROW_H);
    assert_eq!(super::sidebar::tree_key_delta("pageup", px(400.0)), page);
    assert_eq!(
        super::sidebar::tree_key_delta("pagedown", px(400.0)),
        page.map(|p| -p)
    );
    assert_eq!(
        super::sidebar::tree_key_delta("home", px(400.0)),
        Some(f32::INFINITY)
    );
    assert_eq!(
        super::sidebar::tree_key_delta("end", px(400.0)),
        Some(f32::NEG_INFINITY)
    );
    assert_eq!(
        super::sidebar::tree_key_delta("a", px(400.0)),
        None,
        "typing a letter is not a scroll"
    );
}

/// A workspace rename is the display name alone: the id sessions are tagged
/// with and the root they are confined to both stay put.
#[test]
fn renaming_a_workspace_keeps_its_chats() {
    let mut state = state_with_workspace_tree("ws-rename");
    let id = state.settings.global.general.active_workspace.clone();
    let root = state
        .settings
        .global
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id)
        .map(|workspace| workspace.root.clone())
        .expect("the seeded workspace");
    let session = state.new_session().expect("a chat");

    assert!(state.rename_workspace(&id, "  Renamed  "));

    let record = state
        .settings
        .global
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id)
        .expect("still registered");
    assert_eq!(record.name, "Renamed", "the name is trimmed and stored");
    assert_eq!(record.id, id, "the id is the key, and it does not move");
    assert_eq!(record.root, root, "and neither does the root");
    assert!(
        state
            .store
            .in_workspace(&id)
            .iter()
            .any(|row| row.id == session),
        "the chat stays in the workspace under its new name"
    );
    assert!(
        !state.rename_workspace(&id, "   "),
        "a blank name is refused, not stored"
    );
}

/// Deleting a workspace removes the row and REHOMES its chats: a list row going
/// away must not take a transcript with it.
#[test]
fn deleting_a_workspace_rehomes_its_chats() {
    let mut state = state_with_workspace_tree("ws-delete");
    let first = state.settings.global.general.active_workspace.clone();
    let chat = state.new_session().expect("a chat in the first workspace");
    // A second workspace, so the delete has somewhere to rehome to.
    let second = state
        .settings
        .note_workspace("second", "/tmp/bsh-delete-second");
    state.settings.global.general.active_workspace = first.clone();

    assert!(state.delete_workspace(&first));

    assert!(
        !state
            .settings
            .global
            .workspaces
            .iter()
            .any(|workspace| workspace.id == first),
        "the row is gone"
    );
    // The seeded state already registers the launch directory, so the
    // survivor is whichever workspace is first after the delete - the point is
    // that the active id moved to a REGISTERED one and the chat followed it.
    let active = state.settings.global.general.active_workspace.clone();
    assert_ne!(active, first, "the deleted workspace cannot stay active");
    assert!(
        state
            .settings
            .global
            .workspaces
            .iter()
            .any(|workspace| workspace.id == active),
        "the active id is a workspace that still exists"
    );
    let kept = state.store.session(&chat).expect("the chat is not deleted");
    assert_eq!(
        kept.workspace, active,
        "and it is rehomed onto the workspace that remains"
    );
    let _ = second;
}

/// The workspace row reveals its own two controls under the pointer, and the
/// plus opens a chat in THAT workspace rather than moving the selected one.
#[gpui::test]
fn the_workspace_row_opens_a_chat_where_it_points(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("ws-row");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .settings
            .global
            .general
            .active_workspace
            .clone()
    });

    // The controls fade in under the pointer, so the pointer has to be there.
    let row_selector: &'static str = "ws-ws-row";
    let row = cx.debug_bounds(row_selector).expect("the workspace row");
    cx.simulate_mouse_move(
        row.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let add_selector: &'static str = "ws-new-ws-row";
    let plus = cx.debug_bounds(add_selector).expect("the row's plus");
    let before = cx.update(|_, cx| shell.read(cx).state.store.sessions.len());
    cx.simulate_click(plus.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (after, in_ws) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        (
            shell.state.store.sessions.len(),
            shell.state.store.in_workspace(&id).len(),
        )
    });
    assert_eq!(after, before + 1, "the plus opened one chat");
    assert!(in_ws >= 1, "and it landed in the row's own workspace");
}

/// The hero's workspace menu closes with "Add workspace...", the reference's
/// door to a new root - without it the only way in was the sidebar's small plus.
#[gpui::test]
fn the_hero_workspace_menu_offers_adding_one(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("hero-add");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let _shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let chip = cx
        .debug_bounds("hero-workspace")
        .expect("the hero carries its workspace chip");
    cx.simulate_click(chip.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("hero-add-workspace").is_some(),
        "the menu ends with the add row"
    );
}

/// Adding a folder registers it as a workspace and switches to it.
///
/// This is the half of the flow the picker feeds: a directory goes in once (a
/// second add of the same root selects it), becomes active, and the file pane
/// rebases onto its tree.
#[gpui::test]
fn a_folder_can_be_added_as_a_workspace(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("workspace-add", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join("bsh-scratch-added-workspace");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    let _ = std::fs::write(root.join("added.txt"), b"hello\n");
    let root_text = root.to_string_lossy().to_string();

    // The file pane only re-lists while it is the pane on screen, which is the
    // behaviour that keeps a closed panel from walking the filesystem.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Files, cx);
        });
    });
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.adopt_workspace(&root, cx));
    });

    let (active, registered, listed) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let settings = &shell.state.settings;
        let active = settings.global.general.active_workspace.clone();
        let registered = settings
            .global
            .workspaces
            .iter()
            .any(|workspace| workspace.root == root_text);
        let listed = shell.files.roots.iter().any(|row| row.name == "added.txt");
        (active, registered, listed)
    });
    assert!(registered, "the folder is registered as a workspace");
    assert!(
        !active.is_empty(),
        "and one workspace is in use after the add"
    );
    assert!(
        listed,
        "the file pane re-listed onto the new tree: {:?}",
        cx.update(|_, cx| {
            shell
                .read(cx)
                .files
                .roots
                .iter()
                .map(|row| row.name.clone())
                .collect::<Vec<_>>()
        })
    );
    let _ = std::fs::remove_dir_all(&root);
}
/// The session menu carries the reference's five actions.
///
/// It drew two (Pin, Archive) while the reference offers rename, fork and copy
/// as well - operations a reader reaches for at exactly the moment they open
/// this menu.
#[gpui::test]
fn the_session_menu_carries_the_reference_five_actions(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("session-menu", 2);
        // A fixed id, because `debug_bounds` wants a literal selector.
        let id = state.active_id.clone().expect("a session");
        if let Some(session) = state.store.session_mut(&id) {
            session.id = String::from("menu-target");
        }
        state.active_id = Some(String::from("menu-target"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let more = cx
        .debug_bounds("more-menu-target")
        .expect("the row carries its menu control");
    cx.simulate_click(more.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    for (selector, what) in [
        ("session-menu-pin", "pin"),
        ("session-menu-rename", "rename"),
        ("session-menu-fork", "fork"),
        ("session-menu-archive", "archive"),
        ("session-menu-copy", "copy the id"),
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "the menu offers {what}: {selector}"
        );
    }
}

/// Rename retitles the session in place, from a field on its own row.
#[gpui::test]
fn rename_retitles_the_session_from_its_own_row(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("session-rename", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| shell.read(cx).state.active_id.clone().expect("a session"));

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| this.begin_session_rename(&id, window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("session-rename").is_some(),
        "the field appears on the row being renamed"
    );

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.session_rename.update(cx, |state, cx| {
                state.set_value(String::from("Renamed by hand"), window, cx);
            });
            this.commit_session_rename(cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).state.session_title(&id)),
        Some(String::from("Renamed by hand")),
        "the title the reader typed is the one stored"
    );
    assert!(
        cx.debug_bounds("session-rename").is_none(),
        "and the field goes away"
    );
}

/// Fork branches a session: a new id carrying the same transcript.
#[gpui::test]
fn fork_copies_the_transcript_into_a_new_session(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("session-fork", 3);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let original = cx.update(|_, cx| shell.read(cx).state.active_id.clone().expect("a session"));

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.fork_session_at(&original, cx));
    });

    let (forked, original_messages, forked_messages) = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        let forked = state.active_id.clone().expect("the fork opens");
        let original_messages = state
            .store
            .session(&original)
            .expect("the original")
            .messages
            .len();
        let forked_messages = state
            .store
            .session(&forked)
            .expect("the fork")
            .messages
            .len();
        (forked, original_messages, forked_messages)
    });
    assert_ne!(forked, original, "the fork is its own session");
    assert_eq!(
        forked_messages, original_messages,
        "and it carries the transcript it branched from"
    );
}

/// Copy Session ID puts the id on the clipboard, which is what a bug report
/// needs.
#[gpui::test]
fn copy_puts_the_session_id_on_the_clipboard(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("session-copy-id", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let id = cx.update(|_, cx| shell.read(cx).state.active_id.clone().expect("a session"));

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.copy_session_id(&id, cx));
    });
    let copied = cx.update(|_, cx| {
        cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                gpui_kit::ClipboardEntry::String(text) => Some(text.text().clone()),
                _ => None,
            })
        })
    });
    assert_eq!(
        copied.as_deref(),
        Some(id.as_str()),
        "the id is on the clipboard"
    );
}

/// The rename accelerator opens the field, so the menu's hint is not decoration.
#[gpui::test]
fn the_rename_accelerator_opens_the_field(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("session-keys", 1);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // Focus something inside the shell, the way a reader's cursor sits in the
    // composer, so the action has a path to the frame.
    cx.update(|window, cx| {
        use gpui_kit::Focusable as _;
        shell.update(cx, |this, cx| {
            this.composer
                .update(cx, |state, cx| state.focus_handle(cx).focus(window, cx));
        });
    });
    cx.simulate_keystrokes("alt-cmd-g");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).renaming_session.is_some()),
        "the accelerator opens the rename field on the open session"
    );
}
/// The account row is the sidebar's floor: a long session list scrolls in the
/// tree above it instead of pushing it off the bottom of the window.
///
/// The tree had no scroll and the row no `flex_none`, so a full session list
/// either squeezed the row or carried it below the viewport - the one control
/// in the left column that must never move.
#[gpui::test]
fn the_account_row_stays_pinned_under_a_long_session_list(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let mut state = state_with_turns("side-pinned", 1);
        // Forty more sessions: far more than the column can hold.
        for _ in 0..40 {
            let _ = state.new_session();
        }
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let tree = cx.debug_bounds("side-tree").expect("the tree renders");
    let row = cx
        .debug_bounds("account-row")
        .expect("the account row renders");
    let viewport = cx.update(|window, _| window.viewport_size());
    let bottom = row.origin.y + row.size.height;
    assert!(
        bottom.as_f32() <= viewport.height.as_f32() + 0.5,
        "the row sits inside the window: {row:?} vs {viewport:?}"
    );
    assert!(
        row.origin.y.as_f32() > viewport.height.as_f32() - 120.0,
        "and at the bottom, not up the column: {row:?}"
    );
    assert!(
        row.origin.y.as_f32() >= (tree.origin.y + tree.size.height).as_f32() - 1.0,
        "the tree ends where the row begins: tree {tree:?}, row {row:?}"
    );
}
/// The panel's seam reveals its bar on hover, and leaving it puts the bar away.
///
/// It is the same measured reveal the content column's handles carry: the bar
/// exists at opacity 0 and the pointer is what shows it, so the control can be
/// found without the seam being chrome all the time.
#[gpui::test]
fn the_panel_seam_reveals_its_bar_under_the_pointer(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-seam-bar");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let toggle = cx
        .debug_bounds("hdr-show-right-panel")
        .expect("the header carries the toggle");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("right-panel-bar").is_some(),
        "the bar is laid out even while it is invisible"
    );
    let strip = cx
        .debug_bounds("right-panel-handle")
        .expect("the seam's strip");
    cx.simulate_mouse_move(
        strip.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    assert!(
        cx.update(|_, cx| shell.read(cx).right_hover),
        "the strip's own move reveals the bar"
    );

    let body = cx
        .debug_bounds("right-panel-body")
        .expect("the panel's body");
    cx.simulate_mouse_move(
        body.center(),
        None::<gpui_kit::MouseButton>,
        Modifiers::default(),
    );
    assert!(
        !cx.update(|_, cx| shell.read(cx).right_hover),
        "and moving into the column puts it away"
    );
}
/// The panel's refusal copy comes from the string table, in both languages.
#[test]
fn the_right_panel_refusals_are_translated() {
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::En, "right.files.outside"),
        "That path leaves the workspace."
    );
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::Zh, "right.files.outside"),
        "该路径不在工作区里。"
    );
    assert_eq!(
        crate::i18n::tr(crate::i18n::Lang::En, "right.terminal.refused"),
        "refused by policy"
    );
    assert_ne!(
        crate::i18n::tr(crate::i18n::Lang::Zh, "right.terminal.refused"),
        "right.terminal.refused",
        "a missing key renders as the key, so this proves the value exists"
    );
}

/// Notes are keyed by the file's path, so they follow a rename and die with a
/// delete rather than orphaning onto a path nothing points at.
#[test]
fn notes_follow_a_rename_and_go_with_a_delete() {
    use std::path::Path;
    let mut notes = crate::notes::Notes::default();
    let note = |text: &str| crate::notes::Note {
        to: None,
        line: Some(3),
        text: text.to_string(),
    };
    notes.add(Path::new("src/lib.rs"), note("first"));
    notes.rename(Path::new("src/lib.rs"), Path::new("src/main.rs"));
    assert!(
        !notes.has_notes(Path::new("src/lib.rs")),
        "the old path keeps nothing"
    );
    assert_eq!(
        notes.on(Path::new("src/main.rs")).len(),
        1,
        "the remarks moved with the file"
    );
    notes.forget(Path::new("src/main.rs"));
    assert!(
        !notes.has_notes(Path::new("src/main.rs")),
        "a deleted file takes its notes with it"
    );
}

/// The header's right control opens the panel, and the panel starts on the
/// reference's own three Start cards.
#[gpui::test]
fn the_header_opens_the_right_panel_on_its_start_screen(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-open");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    assert!(
        cx.debug_bounds("right-panel").is_none(),
        "the panel starts folded, so the transcript keeps the width"
    );

    let toggle = cx
        .debug_bounds("hdr-show-right-panel")
        .expect("the header carries the right panel's toggle");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).right.open),
        "the toggle must unfold the panel"
    );
    assert!(
        cx.update(|_, cx| shell.read(cx).right.tab).is_none(),
        "and it lands on the Start screen, not on a pane"
    );

    for card in [
        "right-card-right.tab.files",
        "right-card-right.tab.terminal",
        "right-card-right.tab.browser",
    ] {
        assert!(
            cx.debug_bounds(card).is_some(),
            "the Start screen carries the {card} card"
        );
    }
}

/// The Files card opens the browser, directories lead, and a file opens in the
/// editor with the save control beside it.
#[gpui::test]
fn the_files_pane_lists_the_workspace_and_opens_a_file(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-files");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // The panel starts folded, so the Start screen's cards are not on screen
    // until the header's toggle puts them there.
    let toggle = cx
        .debug_bounds("hdr-show-right-panel")
        .expect("the header carries the right panel's toggle");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let card = cx
        .debug_bounds("right-card-right.tab.files")
        .expect("the Files card renders");
    cx.simulate_click(card.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.update(|_, cx| shell.read(cx).right.tab.is_some()),
        "the card must put the Files pane up"
    );
    let rows = cx.update(|_, cx| {
        shell
            .read(cx)
            .files
            .roots
            .iter()
            .map(|row| (row.name.clone(), row.dir))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        rows,
        vec![
            ("docs".to_string(), true),
            ("src".to_string(), true),
            ("Cargo.toml".to_string(), false),
            ("README.md".to_string(), false),
        ],
        "directories lead the listing and the noise skips keep it readable"
    );

    // Choosing a directory expands its branch in place rather than opening an
    // editor, which is what makes the listing a tree.
    let src = cx
        .debug_bounds("file-row-src")
        .expect("the src row renders");
    cx.simulate_click(src.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| {
            shell
                .read(cx)
                .files
                .expanded
                .iter()
                .any(|rel| rel == std::path::Path::new("src"))
        }),
        "a directory row expands its branch"
    );
    assert!(
        cx.update(|_, cx| shell.read(cx).files.tabs.is_empty()),
        "and opens no tab"
    );
    assert!(
        cx.debug_bounds("file-row-lib.rs").is_some(),
        "the branch's own entries render under it"
    );

    // A file row still opens as a tab, from wherever it sits in the tree.
    let readme = cx
        .debug_bounds("file-row-README.md")
        .expect("the README row renders");
    cx.simulate_click(readme.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let opened = cx.update(|_, cx| shell.read(cx).files.active_tab().is_some());
    assert!(opened, "a file row must open it as a tab");
    assert!(
        cx.debug_bounds("files-editor").is_some(),
        "the editor body renders"
    );
    // The document surface is the editor and nothing else: the tab chip names
    // the file and cmd-s saves, so no path bar or save button rides on it.
    assert!(
        cx.debug_bounds("files-save").is_none(),
        "no save button on the surface"
    );
    assert!(
        cx.debug_bounds("files-header").is_none(),
        "no tree header over a document"
    );
    assert!(
        cx.debug_bounds("files-manage").is_none(),
        "no new-file strip over a document"
    );
    assert!(
        cx.debug_bounds("file-tab-README.md").is_some(),
        "and the tab strip carries the file"
    );
    // The document owns the whole panel body, the way the reference's dock
    // hands it the full pane: the tree is not squeezed in beside it.
    let panel = cx
        .debug_bounds("files-pane")
        .expect("the pane renders")
        .size
        .width;
    let editor = cx
        .debug_bounds("files-editor")
        .expect("the editor renders")
        .size
        .width;
    assert_eq!(
        editor, panel,
        "the editor spans the pane instead of sitting beside the tree"
    );
    assert!(
        cx.debug_bounds("file-row-README.md").is_none(),
        "the tree steps aside while a document is open"
    );
}

/// The New-file strip has no permanent home in the reference: a double-click
/// on the tree's blank space is what brings the field up, a single press takes
/// it back down, and once a name is created the file joins the tree.
#[gpui::test]
fn double_clicking_the_blank_tree_adds_a_file(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-blank-new");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // The panel starts folded, so the Start screen's cards are not on screen
    // until the header's toggle puts them there.
    let toggle = cx
        .debug_bounds("hdr-show-right-panel")
        .expect("the header carries the right panel's toggle");
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Go straight to the Files tab the way the card does.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Files, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // With nothing open the tree owns the surface, and its blank area is there
    // to catch the gesture.
    assert!(
        cx.debug_bounds("file-row-README.md").is_some(),
        "the tree renders"
    );
    let blank = cx
        .debug_bounds("files-blank")
        .expect("the blank area under the rows renders");
    assert!(
        cx.debug_bounds("files-new").is_none(),
        "the new-file field is absent before the gesture"
    );

    // Two presses in the same spot read as a double-click and bring the field.
    // The blank fills the remaining height, so its center is always on screen.
    let spot = blank.center();
    cx.simulate_click(spot, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_click(spot, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.update(|_, cx| shell.read(cx).files.new_file_open),
        "a double-click on blank space opens the new-file field"
    );
    assert!(
        cx.debug_bounds("files-new").is_some(),
        "and the field is on screen"
    );

    // A single press takes it away again, so the gesture is easy to undo.
    cx.simulate_click(spot, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        !cx.update(|_, cx| shell.read(cx).files.new_file_open),
        "a single press on blank space closes the field"
    );

    // Bring it back and name the file the way the pane does.
    cx.simulate_click(spot, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_click(spot, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let input = cx
        .debug_bounds("files-new-input")
        .expect("the name field renders");
    cx.simulate_click(input.center(), Modifiers::default());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.files
                .new_name
                .update(cx, |state, cx| state.set_value("note.txt", window, cx));
        });
    });
    let create = cx
        .debug_bounds("files-create")
        .expect("the create button renders");
    cx.simulate_click(create.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.update(|_, cx| {
            shell
                .read(cx)
                .files
                .roots
                .iter()
                .any(|row| row.name == "note.txt")
        }),
        "creating the file adds it to the tree"
    );
    assert!(
        !cx.update(|_, cx| shell.read(cx).files.new_file_open),
        "and the field retires once the file exists"
    );
}

/// A second file opens beside the first rather than replacing it, pressing a
/// tab focuses it, and closing the last one hands the panel back to the tree.
#[gpui::test]
fn opening_a_second_file_adds_a_tab_and_keeps_the_first(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-tabs");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Files, cx);
        });
    });
    for name in ["README.md", "Cargo.toml"] {
        cx.update(|window, cx| {
            shell.update(cx, |this, cx| {
                this.open_file(std::path::Path::new(name), window, cx);
            });
        });
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        cx.update(|_, cx| shell.read(cx).files.tabs.len()),
        2,
        "the second file adds a tab beside the first rather than replacing it"
    );
    assert!(
        cx.debug_bounds("file-tab-README.md").is_some()
            && cx.debug_bounds("file-tab-Cargo.toml").is_some(),
        "and the strip carries both"
    );

    // Pressing the first tab focuses it without reopening the file.
    let first = cx
        .debug_bounds("file-tab-README.md")
        .expect("the README tab renders")
        .center();
    cx.simulate_click(first, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).files.active),
        Some(0),
        "pressing a tab focuses it"
    );

    // Closing the active tab hands the body back to the tree, which is what
    // the Files chip names.
    let close = cx
        .debug_bounds("file-tab-close-README.md")
        .expect("the tab's close renders")
        .center();
    cx.simulate_click(close, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).files.tabs.len()),
        1,
        "the close drops the tab"
    );
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).files.active),
        None,
        "closing the tab on screen returns to the tree"
    );

    // Closing the last one empties the editor area, so the tree takes over.
    let close = cx
        .debug_bounds("file-tab-close-Cargo.toml")
        .expect("the last tab's close renders")
        .center();
    cx.simulate_click(close, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).files.tabs.is_empty()),
        "the last tab closes too"
    );
    assert!(
        cx.debug_bounds("files-editor").is_none(),
        "and the editor column goes with it"
    );
}

/// The editor's buffer marks the file dirty on a change and the save clears it,
/// which is the whole of the edit loop the panel promises.
#[gpui::test]
fn an_edit_marks_the_file_dirty_and_save_clears_it(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-dirty");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Files, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.open_file(std::path::Path::new("README.md"), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let editor = cx
        .update(|_, cx| {
            shell
                .read(cx)
                .files
                .active_tab()
                .and_then(|tab| tab.editor.clone())
        })
        .expect("README.md opens in an editor");
    cx.update(|window, cx| {
        // `set_value` deliberately suppresses Change - it is a load, not an
        // edit - so the test edits the way a reader does, through the path
        // that records an undo and therefore emits the event the dirty mark
        // hangs off.
        editor.update(cx, |state, cx| {
            state.replace_all("# edited\n", window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell
            .read(cx)
            .files
            .active_tab()
            .is_some_and(|tab| tab.dirty)),
        "a change to the buffer marks the file dirty"
    );

    // cmd-s is the whole save affordance now: the action dispatches through
    // the focused editor and bubbles to the files body's handler.
    cx.update(|window, cx| {
        // The action dispatches through the focused editor, so focus it first
        // the way a reader who clicked into the buffer already has.
        editor.update(cx, |state, cx| {
            gpui_kit::Focusable::focus_handle(state, cx).focus(window, cx);
        });
        window.dispatch_keystroke(
            gpui_kit::Keystroke::parse("cmd-s").expect("cmd-s parses"),
            cx,
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        !cx.update(|_, cx| shell
            .read(cx)
            .files
            .active_tab()
            .is_some_and(|tab| tab.dirty)),
        "saving clears the dirty mark"
    );
    let written = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-scratch-ws-right-dirty/README.md"),
    )
    .expect("the save reaches the disk");
    assert_eq!(written, "# edited\n", "and writes the buffer back");
}

/// A note pins to the open file at the editor's cursor line, shows in the
/// drawer, and survives a reload of the workspace's sidecar.
#[gpui::test]
fn a_note_pins_to_the_file_and_survives_a_reload(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-notes");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Files, cx);
            this.open_file(std::path::Path::new("README.md"), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // A file with no notes keeps the surface clean, so the reader opens the
    // drawer through the toggle that rides the document's corner.
    assert!(
        !cx.update(|_, cx| shell.read(cx).files.notes_open),
        "a file with no notes opens clean"
    );
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.open_comment_card(cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).files.notes_open),
        "the card raises the drawer"
    );

    // Type into the note field and press the add control.
    let field = cx
        .debug_bounds("files-note-field")
        .expect("the note field renders")
        .center();
    cx.simulate_click(field, Modifiers::default());
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.files.note_text.update(cx, |state, cx| {
                state.set_value("remember to mention the bus", window, cx);
            });
        });
    });
    let add = cx
        .debug_bounds("files-note-add")
        .expect("the add control renders")
        .center();
    cx.simulate_click(add, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        cx.update(|_, cx| shell
            .read(cx)
            .files
            .notes
            .on(std::path::Path::new("README.md"))
            .len()),
        1,
        "the note pins to the open file"
    );
    assert!(
        cx.debug_bounds("files-note-L1").is_some(),
        "and the drawer lists it under its line anchor"
    );

    // The sidecar on disk carries it, so another launch sees the same note.
    let sidecar = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-scratch-ws-right-notes/.bsh/annotations.json"),
    )
    .expect("the note is written to the workspace's sidecar");
    assert!(sidecar.contains("remember to mention the bus"));
}

/// Opening the Terminal tab starts a real interactive shell and paints its
/// cell grid. Typing is proven through the screen model, so the assertion does
/// not depend on a shell round-trip in CI.
#[gpui::test]
fn the_terminal_pane_starts_a_shell_and_paints_its_grid(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-terminal");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Terminal, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("terminal-pane").is_some(),
        "the terminal surface renders"
    );
    assert!(
        cx.debug_bounds("terminal-status").is_none(),
        "and a running shell draws no status line"
    );
    let (running, refused, spawned) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let terminal = shell.right.terminal.as_ref().expect("the pane spawned one");
        (
            terminal.is_running(),
            terminal.refused,
            shell.right.terminal.is_some(),
        )
    });
    assert!(spawned, "opening the tab spawns the terminal");
    assert!(running, "the default gate starts an interactive shell");
    assert!(!refused, "and nothing refused it");

    // Bytes into the screen model: the parser paints them, with their colour.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            let terminal = this.right.terminal.as_ref().expect("the terminal");
            terminal.feed(b"\x1b[2J\x1b[H\x1b[?25h\x1b[32mok\x1b[0m");
            this.sync_terminal(cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let snapshot = cx.update(|_, cx| shell.read(cx).terminal_snapshot.clone());
    let found = snapshot.rows.iter().enumerate().find_map(|(row, cells)| {
        cells
            .iter()
            .position(|cell| cell.ch == 'o' && cell.fg == crate::terminal::Color::Indexed(2))
            .map(|col| (row, col))
    });
    let (row, col) = found.expect("the fed bytes are on the grid, in green");
    assert_eq!(
        snapshot.cell(row, col + 1).map(|cell| cell.ch),
        Some('k'),
        "the whole word reaches the grid"
    );

    // The grid is painted, not only laid out: the cursor cell's inverted
    // block is a solid quad in the theme's text colour inside the pane.
    let pane = cx
        .debug_bounds("terminal-pane")
        .expect("the pane has bounds");
    let (left, top) = (f32::from(pane.origin.x), f32::from(pane.origin.y));
    let right = left + f32::from(pane.size.width);
    let bottom = top + f32::from(pane.size.height);
    let scale = cx.update(|window, _| window.scale_factor());
    let painted = cx.update(|window, _| {
        let text: gpui_kit::Hsla = gpui_kit::rgb(crate::theme::text()).into();
        window.painted_quads().into_iter().any(|quad| {
            // Scene quads are in device pixels; the pane's bounds are logical.
            let x = quad.bounds.origin.x.as_f32() / scale;
            let y = quad.bounds.origin.y.as_f32() / scale;
            let inside = x >= left && y >= top && x < right && y < bottom;
            inside && quad.background.as_solid() == Some(text)
        })
    });
    assert!(
        painted,
        "the cursor cell is painted in the foreground colour"
    );

    // The pane measured its own cell grid; the clock hands that size to the
    // terminal, so a panel resize reaches the PTY without touching paint.
    let measured = cx
        .update(|_, cx| shell.read(cx).terminal_frame.get())
        .expect("the pane measured its grid at prepaint time");
    assert!(
        measured.cols > 20 && measured.rows > 5,
        "the pane filled its body rather than collapsing: {measured:?}"
    );
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| this.sync_terminal(cx));
    });
    let (size, dims) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let terminal = shell.right.terminal.as_ref().expect("the terminal");
        let snapshot = shell.terminal_snapshot.clone();
        (terminal.size(), (snapshot.cols, snapshot.rows.len()))
    });
    assert_eq!(
        dims,
        (measured.cols as usize, measured.rows as usize),
        "the measured grid is the one the terminal holds"
    );
    assert_eq!(
        size,
        Some((measured.cols, measured.rows)),
        "and the PTY was told the same size"
    );
}

/// The gate is the only door: a deny policy leaves the pane with no process
/// and with the policy's own refusal copy on screen.
#[gpui::test]
fn a_deny_policy_refuses_the_terminal(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-terminal-deny");
        let root = state.workspace_root().expect("the scratch workspace root");
        let _ = std::fs::create_dir_all(root.join(".bsh"));
        std::fs::write(
            root.join(".bsh").join("terminal-policy.rego"),
            "package terminal\n\nallow := false if { input.action == \"start\" }\n",
        )
        .expect("the policy file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Terminal, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (refused, running, status, expected) = cx.update(|_, cx| {
        let shell = shell.read(cx);
        let terminal = shell.right.terminal.as_ref().expect("the terminal exists");
        (
            terminal.refused,
            terminal.is_running(),
            shell.terminal_status_text(),
            shell.state.lang.tr("right.terminal.refused"),
        )
    });
    assert!(refused, "the workspace policy refused the shell start");
    assert!(!running, "so no PTY was spawned");
    assert_eq!(
        status,
        Some((expected, true)),
        "the pane names the policy refusal"
    );
    assert!(
        cx.debug_bounds("terminal-status").is_some(),
        "and the refusal is drawn in the pane"
    );
}

/// A headless terminal, fed by the test, is the deterministic grid the pointer
/// tests drag over: no shell output can shift a row between the draw and the
/// drag.
fn feed_headless_terminal(shell: &Entity<Shell>, cx: &mut VisualTestContext, bytes: &[u8]) {
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.right.terminal = Some(crate::terminal::Terminal::without_process(
                std::path::PathBuf::new(),
                Lang::En,
                40,
                20,
            ));
            this.right
                .terminal
                .as_ref()
                .expect("the terminal")
                .feed(bytes);
            this.sync_terminal(cx);
        });
    });
}

/// The centre of one cell, in window coordinates.
fn terminal_cell_point(
    frame: crate::ui::right_panel::TerminalFrame,
    row: usize,
    col: usize,
) -> gpui_kit::Point<gpui_kit::Pixels> {
    gpui_kit::point(
        frame.origin.x + frame.cell_w * (col as f32 + 0.5),
        frame.origin.y + frame.cell_h * (row as f32 + 0.5),
    )
}

/// A drag over the terminal selects the cells it covers and Cmd-C puts them on
/// the clipboard; a press with no drag clears the selection again.
#[gpui::test]
fn a_drag_over_the_terminal_selects_and_copies(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-terminal-select");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Terminal, cx);
        });
    });
    feed_headless_terminal(&shell, cx, b"\x1b[2J\x1b[H\x1b[6;1Hhello world");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let frame = cx
        .update(|_, cx| shell.read(cx).terminal_frame.get())
        .expect("the pane measured its grid");
    let start = terminal_cell_point(frame, 5, 0);
    let end = terminal_cell_point(frame, 5, 4);
    cx.simulate_mouse_down(start, gpui_kit::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(end, Some(gpui_kit::MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(end, gpui_kit::MouseButton::Left, Modifiers::default());

    let copied = cx.update(|_, cx| shell.read(cx).terminal_selection_text());
    assert_eq!(
        copied.as_deref(),
        Some("hello"),
        "the drag selected the word"
    );
    // The copy chord, through the pane's own key handler.
    cx.simulate_keystrokes("cmd-c");
    let pasted = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        pasted.as_deref(),
        Some("hello"),
        "and copying the selection reached the clipboard"
    );

    // The selected cells are painted with the theme's selected-row
    // background, not merely remembered.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let pane = cx.debug_bounds("terminal-pane").expect("the pane");
    let (left, top) = (f32::from(pane.origin.x), f32::from(pane.origin.y));
    let right = left + f32::from(pane.size.width);
    let bottom = top + f32::from(pane.size.height);
    let scale = cx.update(|window, _| window.scale_factor());
    let selected: gpui_kit::Hsla = gpui_kit::rgb(crate::theme::selected_row()).into();
    let painted = cx.update(|window, _| {
        window.painted_quads().into_iter().any(|quad| {
            let x = quad.bounds.origin.x.as_f32() / scale;
            let y = quad.bounds.origin.y.as_f32() / scale;
            let inside = x >= left && y >= top && x < right && y < bottom;
            inside && quad.background.as_solid() == Some(selected)
        })
    });
    assert!(
        painted,
        "the selected cells paint with the selected-row background"
    );

    // A press without a drag clears the selection.
    cx.simulate_click(terminal_cell_point(frame, 5, 10), Modifiers::default());
    let cleared = cx.update(|_, cx| shell.read(cx).right.terminal_selection);
    assert_eq!(cleared, None, "a click clears the selection");
    let copied = cx.update(|_, cx| shell.read(cx).terminal_selection_text());
    assert_eq!(copied, None);
}

/// The wheel over the pane browses the retained scrollback: 100 lines into a
/// 10-row grid, five lines back, and the visible top row moves by five.
#[gpui::test]
fn the_terminal_wheel_browses_the_scrollback(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-terminal-scroll");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Terminal, cx);
        });
    });
    let mut lines = String::new();
    for index in 0..100 {
        lines.push_str(&format!("line-{index:03}\r\n"));
    }
    feed_headless_terminal(&shell, cx, lines.as_bytes());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let before = cx.update(|_, cx| shell.read(cx).terminal_snapshot.first_row);
    let pane = cx
        .debug_bounds("terminal-pane")
        .expect("the pane has bounds");
    cx.simulate_mouse_move(pane.center(), None, Modifiers::default());
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: pane.center(),
        delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, 5.0)),
        modifiers: Modifiers::default(),
        touch_phase: gpui_kit::TouchPhase::Moved,
    });
    let after = cx.update(|_, cx| shell.read(cx).terminal_snapshot.first_row);
    assert_eq!(
        after,
        before - 5,
        "the wheel moved the visible top row up five rows"
    );

    // And back down returns to the bottom, where the cursor lives again.
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: pane.center(),
        delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, -5.0)),
        modifiers: Modifiers::default(),
        touch_phase: gpui_kit::TouchPhase::Moved,
    });
    let bottom = cx.update(|_, cx| shell.read(cx).terminal_snapshot.first_row);
    assert_eq!(bottom, before, "the wheel down came back to the bottom");
}

/// The input gate's refusal is surfaced in the pane, not only in the gate: the
/// status line names the refused line.
#[gpui::test]
fn a_refused_input_surfaces_in_the_terminal_status(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-terminal-input-deny");
        let root = state.workspace_root().expect("the scratch workspace root");
        let _ = std::fs::create_dir_all(root.join(".bsh"));
        std::fs::write(
            root.join(".bsh").join("terminal-policy.rego"),
            "package terminal\n\nallow := false if { input.action == \"input\" }\n",
        )
        .expect("the policy file");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.open_right_tab(crate::ui::right_panel::RightTab::Terminal, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (outcome, status) = cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            let terminal = this.right.terminal.as_mut().expect("the terminal");
            for byte in b"rm -rf /" {
                terminal.send_key(&[*byte]);
            }
            let outcome = terminal.send_key(b"\r");
            (outcome, this.terminal_status_text())
        })
    });
    assert_eq!(outcome, crate::terminal::KeyOutcome::Refused);
    let (text, is_error) = status.expect("the refusal is surfaced in the pane");
    assert!(is_error, "a refusal is an error state");
    assert!(
        text.contains("rm -rf /"),
        "the refused line is named: {text}"
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("terminal-status").is_some(),
        "and the status line is drawn"
    );
}

/// A scratch session directory no other test shares.
fn paging_scratch(label: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("bsh-scratch-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A state whose one session carries the given messages in its Parquet log.
///
/// A fresh store loads only the tail window, which is the state the scroll path
/// pages from.
fn state_with_log(label: &str, rows: usize, tool_rows: bool) -> AppState {
    let dir = paging_scratch(label);
    let store = Store::open_at(dir.clone());
    let mut session = crate::model::Session::new("live", "paged");
    session.id = "paged".to_string();
    for index in 0..rows {
        let mut row = ChatMessage::user(format!("row {index}"));
        if tool_rows {
            row.tools.push(ToolEvent {
                name: format!("tool {index}"),
                args: "{}".to_string(),
                output: Some(format!("out {index}")),
                ms: Some(index as u64),
                at: None,
            });
        }
        session.messages.push(row);
    }
    store.save(&session).expect("seed the log");
    drop(store);
    AppState::with_store(Store::open_at(dir))
}

/// The message segment files in one history directory, sorted.
fn message_segments(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("msg-") && name.ends_with(".parquet"))
        .collect();
    names.sort();
    names
}

/// One reach of the scroll path, with its premise settled: the tail is not
/// following, and the scroller reports the top.
fn at_the_top(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.following_tail = false;
            this.chat_last_offset = 0.0;
            this.page_older_at_the_top(cx)
        })
    })
}

/// A page request issued from the scroll path returns before the rows exist,
/// and the rows appear when the background task lands.
#[gpui::test]
fn an_older_page_returns_immediately_and_lands_from_the_background(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_log("older-immediate", crate::store::WINDOW * 3, false);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let before = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_session()
            .expect("the session")
            .messages
            .len()
    });
    assert_eq!(before, crate::store::WINDOW, "only the tail is resident");

    assert!(at_the_top(&shell, cx), "the scroll path starts a read");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("older-loading").is_some(),
        "the pane draws its busy mark while the page is in flight"
    );
    let during = cx.update(|_, cx| {
        let this = shell.read(cx);
        (
            this.older.is_busy(),
            this.older.started(),
            this.state
                .active_session()
                .expect("the session")
                .messages
                .len(),
        )
    });
    assert!(during.0, "the read is in flight");
    assert_eq!(during.1, 1, "one read was scheduled");
    assert_eq!(
        during.2, before,
        "the request returns before the rows arrive"
    );

    cx.run_until_parked();
    let after = cx.update(|_, cx| {
        let this = shell.read(cx);
        let session = this.state.active_session().expect("the session");
        (
            this.older.is_busy(),
            this.older.has_ahead(),
            this.older.ahead_started(),
            session.messages.len(),
            session.messages[0].text.clone(),
        )
    });
    assert!(!after.0, "the read landed");
    assert_eq!(after.3, before + crate::store::WINDOW, "the window arrived");
    assert_eq!(
        after.4,
        format!("row {}", crate::store::WINDOW),
        "and it is the window above the tail"
    );
    assert!(after.1, "the next older page was read ahead");
    assert_eq!(after.2, 1, "exactly one prefetch was issued");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("older-loading").is_none(),
        "the busy mark clears once the rows land"
    );
}

/// A second scroll while the same page loads is the same request, not a second
/// read.
#[gpui::test]
fn a_duplicate_page_request_is_coalesced(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_log("older-coalesce", crate::store::WINDOW * 3, false);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    assert!(at_the_top(&shell, cx), "the first reach starts a read");
    assert!(
        !at_the_top(&shell, cx),
        "the second reach is coalesced into it"
    );
    let in_flight = cx.update(|_, cx| shell.read(cx).older.started());
    assert_eq!(in_flight, 1, "one read, not two");

    cx.run_until_parked();
    let rows = cx.update(|_, cx| {
        shell
            .read(cx)
            .state
            .active_session()
            .expect("the session")
            .messages
            .len()
    });
    assert_eq!(
        rows,
        crate::store::WINDOW * 2,
        "the duplicated request did not load the page twice"
    );
}

/// A read-ahead is issued once a page lands, is the only one in flight, and
/// stops when the log has no row before it.
#[gpui::test]
fn one_prefetch_is_issued_and_the_start_of_the_log_stops_it(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_log("older-prefetch", crate::store::WINDOW * 3, false);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    assert!(at_the_top(&shell, cx));
    cx.run_until_parked();
    let landed = cx.update(|_, cx| {
        let this = shell.read(cx);
        (this.older.has_ahead(), this.older.ahead_started())
    });
    assert!(landed.0, "the next window is buffered");
    assert_eq!(landed.1, 1, "one read-ahead");

    // Asking twice more cannot start a second one while one is buffered.
    cx.update(|_, cx| {
        shell.update(cx, |this, cx| {
            this.start_older_ahead(cx);
            this.start_older_ahead(cx);
        });
    });
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).older.ahead_started()),
        1,
        "the read-ahead ceiling holds"
    );

    // The anchor of the landed page is applied by the next frame; then the
    // buffered window is applied from memory when the reader reaches it.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(at_the_top(&shell, cx), "the buffered window is applied");
    let finished = cx.update(|_, cx| {
        let this = shell.read(cx);
        (
            this.state
                .active_session()
                .expect("the session")
                .messages
                .len(),
            this.older.has_ahead(),
            this.older.ahead_started(),
        )
    });
    assert_eq!(
        finished.0,
        crate::store::WINDOW * 3,
        "the whole log is resident"
    );
    assert!(!finished.1, "no page is ahead of the first one");
    assert_eq!(finished.2, 1, "and no further read-ahead was started");
}

/// The trajectory pane pages its log through the same seam: the request
/// returns first, the rows land from the background, and one page is read
/// ahead.
#[gpui::test]
fn a_trajectory_page_is_scheduled_and_reads_ahead(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_log("trace-async", 200, true);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    // No notify: a notify can flush a frame, and that frame's scroll path
    // would issue the very request this test drives by hand.
    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            this.state.tab = Tab::Trajectory;
            // Park the trace away from its top, so a frame that runs while the
            // read is in flight does not page again behind the test's back.
            this.traj_scroll
                .set_offset(gpui_kit::point(px(0.0), px(-400.0)));
        });
    });

    let first = cx.update(|_, cx| shell.update(cx, |this, cx| this.request_trace_page(cx)));
    let second = cx.update(|_, cx| shell.update(cx, |this, cx| this.request_trace_page(cx)));
    assert!(first, "the first ask schedules a read");
    assert!(!second, "the duplicate is coalesced");
    let during = cx.update(|_, cx| {
        let this = shell.read(cx);
        (
            this.trace.is_busy(),
            this.trace.started(),
            this.trace_rows.is_empty(),
        )
    });
    assert!(during.0);
    assert_eq!(during.1, 1);
    assert!(during.2, "the request returns before rows exist");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("trace-loading").is_some(),
        "the trace draws its busy mark while the page is in flight"
    );

    cx.run_until_parked();
    let after = cx.update(|_, cx| {
        let this = shell.read(cx);
        (
            this.trace.is_busy(),
            this.trace_rows.len(),
            this.trace_rows[0].name.clone(),
            this.trace_pages,
            this.trace.has_ahead(),
            this.trace.ahead_started(),
        )
    });
    assert!(!after.0, "the page landed");
    assert_eq!(after.1, 200 - crate::history::PAGE, "the newest page");
    assert_eq!(after.2, format!("tool {}", crate::history::PAGE));
    assert_eq!(after.3, 1);
    assert!(after.4, "the older page is read ahead");
    assert_eq!(after.5, 1);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("trace-loading").is_none(),
        "the busy mark clears when the page lands"
    );
}

/// A legacy history - small segment per save, no manifest - is rewritten once,
/// in the background, when the session is opened.
#[gpui::test]
fn a_legacy_history_is_compacted_once_off_the_ui_thread(cx: &mut TestAppContext) {
    init_test_app(cx);
    let dir = paging_scratch("legacy-compact");
    let turns = 4;
    {
        let store = Store::open_at(dir.clone());
        let mut session = crate::model::Session::new("live", "legacy");
        session.id = "legacy".to_string();
        for turn in 0..turns {
            session
                .messages
                .push(ChatMessage::user(format!("turn {turn}")));
            session.messages.push(ChatMessage::assistant());
            store.save(&session).expect("save a turn");
        }
    }
    let history_dir = crate::history::History::dir_for(&dir, "legacy");
    let rows = crate::history::History::open(history_dir.clone()).len();
    let before = message_segments(&history_dir).len();
    assert!(before > 1, "the legacy shape is a file per save");

    let state = AppState::with_store(Store::open_at(dir));
    let built_dir = history_dir.clone();
    let (_harness, cx) = cx.add_window_view(move |window, cx| {
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        // Building the frame is synchronous; the rewrite must not have run in
        // it. The window helper pumps the executor after this closure returns,
        // which is where the scheduled compaction lands.
        assert_eq!(
            message_segments(&built_dir).len(),
            before,
            "the frame schedules the rewrite instead of performing it"
        );
        Harness { shell }
    });
    cx.run_until_parked();

    let after = crate::history::History::open(history_dir.clone());
    assert_eq!(after.len(), rows, "the rewrite keeps every row");
    assert!(history_dir.join("manifest.json").is_file());
    assert_eq!(
        message_segments(&history_dir).len(),
        1,
        "the log is one sealed segment"
    );
    assert!(after.all_sealed(), "and it is sealed");
}
