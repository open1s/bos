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
    let plugins = cx
        .debug_bounds("nav-plugins")
        .expect("the plugins row renders");
    let automation = cx
        .debug_bounds("nav-automation")
        .expect("the automation row renders");
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
    assert_eq!(plugins.size.height, px(36.0), "panelRow is 36 min-height");
    assert_eq!(plugins.origin.x, sidebar.origin.x + px(14.0));
    assert_eq!(
        automation.origin.y - plugins.origin.y,
        px(40.0),
        "the measured pitch is 36 over a 4px gap"
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
    assert_eq!(last_bar.size.width, px(12.0));
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

/// Each sidebar panel row names a panel, so it must open the section it names
/// instead of the first one in the rail.
#[gpui::test]
fn nav_row_click_opens_the_section_it_names(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("nav", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    let mcp = cx.debug_bounds("nav-mcp").expect("the MCP row");
    cx.simulate_click(mcp.center(), Modifiers::default());
    let (overlay, section) = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        (
            matches!(state.overlay, Overlay::Settings),
            state.settings_section,
        )
    });
    assert!(overlay, "the MCP row must open the settings dialog");
    assert_eq!(
        section,
        section_index("mcp").expect("the mcp section"),
        "the MCP row must land on the MCP section"
    );
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
            .extentions
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

/// The Plugins row names the reference Built-in plugins panel, and it must open
/// that pane rather than the tool inventory next to it - and that pane own rows
/// must come from what the agent was built from.
#[gpui::test]
fn plugins_row_opens_the_builtin_plugins_pane(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_turns("plugins", 2);
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let nav = cx.debug_bounds("nav-plugins").expect("the Plugins row");
    cx.simulate_click(nav.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let section = cx.update(|_, cx| shell.read(cx).state.settings_section);
    assert_eq!(
        section,
        section_index("plugins").expect("the Built-in plugins section"),
        "the Plugins row must land on the Built-in plugins pane"
    );
    assert!(
        cx.debug_bounds("builtin-plugin-0").is_some(),
        "the pane must list the plugin the agent was built from"
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
            global.add_provider("work", "", "https://work.example/v1", "k", &[]);
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

/// Seed one provider into the global tree the panes edit, with one model leaf
/// so a workspace can name it.
fn seed_provider(global: &mut crate::settings::global::GlobalSettings, id: &str, base_url: &str) {
    global.add_provider(
        id,
        "",
        base_url,
        "",
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
        cx.debug_bounds("field-editor-api-key").is_some(),
        "the API key is the editor's one primary field"
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
        state.settings.workspace.general.active_session =
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

/// The reference's chat content width, straight from its sheet:
/// `clamp(680px, calc(var(--dsh-conversation-column-width, 0px) * .64), 920px)`.
#[test]
fn the_chat_column_takes_the_measured_clamp() {
    use crate::theme::{chat_width_at, CHAT_MAX_W, CHAT_W};
    assert_eq!(
        chat_width_at(0.0),
        CHAT_W,
        "a column with no measurement yet lands on the 680 floor"
    );
    assert_eq!(
        chat_width_at(1000.0),
        CHAT_W,
        "640 clamps up to the same floor"
    );
    assert!(
        (chat_width_at(1320.0) - 844.8).abs() < 0.01,
        "a mid column takes .64 of it: {}",
        chat_width_at(1320.0)
    );
    assert_eq!(
        chat_width_at(1720.0),
        CHAT_MAX_W,
        "1100.8 clamps down to the 920 cap"
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
        920.0,
        "with no preference the column is the clamp itself"
    );
    assert_eq!(
        resolve_content_width(1640.0, Some(600.0)),
        640.0,
        "a preference below the floor clamps up to 640, not to the clamp's 680"
    );
    assert_eq!(
        resolve_content_width(1640.0, Some(2000.0)),
        1464.0,
        "and above the cap to column - 176"
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
    let body_right = body.origin.x + body.size.width;
    assert_eq!(
        (body_right - (left.origin.x + left.size.width)).as_f32(),
        offset,
        "the left handle hangs `right: calc(50% + content / 2 + 24px)` off the body"
    );
    assert_eq!(
        (right.origin.x - body.origin.x).as_f32(),
        offset,
        "and the right one mirrors it"
    );
    let content_left = body.origin.x + px(column / 2.0 - content / 2.0);
    let content_right = body.origin.x + px(column / 2.0 + content / 2.0);
    // GPUI snaps the painted content column to a device pixel, so the
    // clearance is the measured 24 within half a pixel.
    let left_clear = (content_left - (left.origin.x + left.size.width)).as_f32();
    let right_clear = (right.origin.x - content_right).as_f32();
    assert!(
        (left_clear - crate::theme::HANDLE_OFFSET).abs() <= 0.5,
        "24px clear of the content column's own edge, measured {left_clear}"
    );
    assert!(
        (right_clear - crate::theme::HANDLE_OFFSET).abs() <= 0.5,
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
        state.settings.workspace.general.active_session =
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
        state.settings.workspace.general.active_session =
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
        state.settings.workspace.general.active_session = running.clone();
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
        let mut state = state_with_turns("catalog", 1);
        state.settings.workspace.general.provider = "work".to_string();
        state.settings.workspace.general.model = "gpt-4o".to_string();
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

    // The catalog is the nicknames one provider carries, and the active model is
    // the workspace reference.
    let draft = |cx: &mut VisualTestContext| -> (Vec<String>, String) {
        cx.update(|_, cx| {
            let state = &harness.read(cx).shell.read(cx).state;
            (
                state.draft().catalog("work").to_vec(),
                state.draft().workspace.general.model.clone(),
            )
        })
    };

    let (models, model) = draft(cx);
    assert_eq!(
        models,
        ["work-model".to_string()],
        "the seeded provider carries its one leaf"
    );
    assert_eq!(model, "gpt-4o", "the seeded model is the active one");

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
    let (models, _) = draft(cx);
    assert_eq!(
        models,
        ["o3".to_string(), "work-model".to_string()],
        "adds keep order and a repeat merges rather than duplicating"
    );

    // A row selects itself as the active model, which is what the model menu and
    // the composer chip read.
    cx.update(|_, cx| shell.update(cx, |this, _| this.state.select_model("o3")));
    let (_, model) = draft(cx);
    assert_eq!(model, "o3", "selecting a row makes it the model");

    // Delete drops one id and leaves the active model where it was.
    cx.update(|_, cx| {
        shell.update(cx, |this, _| {
            this.state
                .draft_mut()
                .global
                .remove_model("work", "work-model");
        })
    });
    let (models, model) = draft(cx);
    assert_eq!(models, ["o3".to_string()], "delete removes exactly one id");
    assert_eq!(model, "o3", "deleting does not move the active model");
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

/// Picking a provider picks the pair, so the document stays committable.
///
/// The workspace file refuses a selection that names a provider and no
/// model. Writing the provider alone made creating a fresh provider - the
/// one flow that has no model yet - leave a document that could not be
/// saved, and the refusal named a model the reader had never been asked for.
#[test]
fn selecting_a_provider_never_half_makes_the_selection() {
    let mut state = AppState::with_store(scratch_store("select-pair"));
    // A provider with NO models, which is what the catalogue route adds, and
    // one that carries a model.
    state
        .draft_mut()
        .global
        .add_provider("bare", "", "https://bare.example", "k", &[]);
    seed_provider(
        &mut state.draft_mut().global,
        "full",
        "https://full.example",
    );

    // A provider with no models cannot be selected: the pair must be both
    // or neither, and there is no model to point at. The previous selection
    // is left alone, so creating a provider does not drop the working model.
    state.select_provider("full");
    let chosen = state.draft().workspace.general.model.clone();
    state.select_provider("bare");
    let draft = state.draft();
    assert_eq!(
        draft.workspace.general.provider, "full",
        "a model-less provider does not become the selection"
    );
    assert_eq!(
        draft.workspace.general.model, chosen,
        "and it does not clear the model that was working"
    );
    assert!(
        draft.workspace.validate().is_ok(),
        "the document stays committable after creating a bare provider"
    );

    // A provider that has models moves both halves, to its first model.
    state.select_provider("full");
    let draft = state.draft();
    assert_eq!(draft.workspace.general.provider, "full");
    assert!(
        !draft.workspace.general.model.is_empty(),
        "the model half follows the provider half"
    );
    assert!(draft.workspace.validate().is_ok());
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
        crate::caps::build_agent(shell.read(cx).state.draft())
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
        let mut state = state_with_turns("model-add-row", 1);
        state.settings.workspace.general.provider = "work".to_string();
        state.settings.workspace.general.model = "work-model".to_string();
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
        let mut state = state_with_turns("picker", 1);
        state.settings.workspace.general.provider = "work".to_string();
        state.settings.workspace.general.model = "gpt-4o".to_string();
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

/// The Built-in plugins pane is the reference own section, and its rows are real:
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

    let status = |cx: &mut VisualTestContext, name: &str| -> &'static str {
        cx.update(|_, cx| harness.read(cx).shell.read(cx).plugin_status_key(name))
    };
    assert_eq!(
        status(cx, "shell"),
        "settings.pluginEnabled",
        "an installed plugin reads as enabled"
    );

    cx.update(|_, cx| shell.update(cx, |this, _| this.state.toggle_plugin_off("shell")));
    assert_eq!(
        status(cx, "shell"),
        "settings.pluginDisabled",
        "switching a plugin off in the draft flips this pane status"
    );
    assert_eq!(
        status(cx, "files"),
        "settings.pluginEnabled",
        "another plugin is unaffected"
    );
}

/// Every label the plugin panes draw resolves in both locales; a typo would
/// otherwise render the key itself.
#[test]
fn the_plugin_keys_resolve_in_both_locales() {
    for key in [
        "settings.plugins",
        "settings.builtinPluginsHint",
        "settings.pluginViews",
        "settings.pluginEnabled",
        "settings.pluginDisabled",
        "settings.plugin-inventory",
        "settings.pluginListHint",
    ] {
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Zh] {
            assert_ne!(crate::i18n::tr(lang, key), key, "missing locale value");
        }
    }
}

/// Automation tasks have no panel here: the reference schedules them and bsh
/// composes no task scheduler. The row used to land on Tools, which only looked
/// related, so this guards both halves - it explains itself, and it leaves the
/// pane where it found it.
#[gpui::test]
/// The Automation tasks row opens the page now. Until round 89 it reported that
/// this deployment composes no task scheduler; the click must land on the
/// automation section and raise no notice.
#[gpui::test]
fn the_automation_row_opens_the_automation_section(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = AppState::with_store(scratch_store("nav-automation"));
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());
    let before = cx.update(|_, cx| shell.read(cx).state.settings_section);
    let bounds = cx
        .debug_bounds("nav-automation")
        .expect("the Automation row");
    cx.simulate_click(bounds.center(), Modifiers::default());
    let (notice, section) = cx.update(|_, cx| {
        let state = &shell.read(cx).state;
        (state.notice.clone(), state.settings_section)
    });
    assert_eq!(
        notice, None,
        "the row opens the page instead of explaining itself"
    );
    assert_eq!(
        section,
        section_index("automation").expect("the rail index"),
        "the click must land on the automation section"
    );
    assert_ne!(before, section, "and it must move the pane, not leave it");
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
        let mut state = state_with_turns("input-types", 1);
        state.settings.workspace.general.provider = "p".to_string();
        state.settings.workspace.general.model = "o3".to_string();
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
                "",
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

/// The rail is the reference's five cells. Everything else the registry holds
/// is hosted where the reference hosts it - the view list inside Built-in
/// plugins - and the two lists together cover the registry, so no pane is left
/// unreachable and no control only looks wired.
#[test]
fn the_settings_rail_is_the_five_measured_cells() {
    use super::settings::{section_index, NAV, SECTIONS, VIEWS};
    assert_eq!(
        NAV,
        ["account", "general", "models", "plugins", "agent-presets"],
        "the measured settings.section registrations, in their order"
    );
    assert_eq!(
        VIEWS.len() + NAV.len(),
        SECTIONS.len(),
        "no pane is left out of both lists"
    );
    let mut seen: Vec<usize> = NAV
        .iter()
        .chain(VIEWS.iter())
        .map(|id| section_index(id).expect("every listed id names a pane"))
        .collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..SECTIONS.len()).collect::<Vec<usize>>(),
        "the rail and the view list partition the panes"
    );
}

/// The Built-in plugins pane is the reference's host for the panes the rail does
/// not carry: one view row per hosted pane, and pressing a row really moves the
/// dialog. The About block is General's own last row group, not a cell.
#[gpui::test]
fn the_builtin_plugins_pane_hosts_the_reference_views(cx: &mut TestAppContext) {
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
    ] {
        assert!(cx.debug_bounds(cell).is_some(), "{cell} draws");
    }
    assert!(
        cx.debug_bounds("set-nav-5").is_none(),
        "the rail is five cells, as the reference's five registrations are"
    );
    let row = cx
        .debug_bounds("builtin-view-shell")
        .expect("the Shell view row draws");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let section = cx.update(|_, cx| harness.read(cx).shell.read(cx).state.settings_section);
    assert_eq!(
        section,
        section_index("shell").expect("the Shell pane"),
        "the row opens the pane it names"
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
    state.settings.active_workspace = label.to_string();
    state.settings.workspaces.push(crate::settings::Workspace {
        id: label.to_string(),
        name: label.to_string(),
        root: root.to_string_lossy().to_string(),
        ..Default::default()
    });
    state
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
    let toggle = cx
        .debug_bounds("files-notes-toggle")
        .expect("the notes toggle renders")
        .center();
    cx.simulate_click(toggle, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.update(|_, cx| shell.read(cx).files.notes_open),
        "the toggle opens the drawer"
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

/// The terminal pane opens with a screen and a command line, and a command
/// the gate allows is echoed to the screen the way a shell would show it.
#[gpui::test]
fn the_terminal_pane_echoes_a_gated_command(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-terminal");
        let shell = cx.new(|cx| Shell::build_for_test(window, None, state, cx));
        Harness { shell }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shell = cx.update(|_, cx| harness.read(cx).shell.clone());

    // Opening the Terminal tab spawns the terminal and shows its surface.
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
        cx.debug_bounds("terminal-input").is_some(),
        "and the command line renders"
    );
    assert!(
        cx.update(|_, cx| shell.read(cx).right.terminal.is_some()),
        "opening the tab spawns the terminal"
    );

    // Type a command and press Enter. The default gate allows everything, so
    // the command is echoed to the screen as `cwd $ command`.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.terminal_input.update(cx, |input, cx| {
                input.set_value("ls -la", window, cx);
            });
            this.run_terminal_command(window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let lines = cx.update(|_, cx| {
        shell
            .read(cx)
            .right
            .terminal
            .as_ref()
            .map(|terminal| terminal.screen.lines.clone())
            .unwrap_or_default()
    });
    assert!(
        lines.iter().any(|line| line.ends_with("$ ls -la")),
        "the allowed command is echoed to the screen: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("refused")),
        "and the gate does not refuse it"
    );
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).terminal_input.read(cx).value().to_string()),
        "",
        "and the command line clears after Enter"
    );
}

/// A command the gate refuses is shown as refused, not echoed as run.
#[gpui::test]
fn the_terminal_pane_shows_a_refused_command(cx: &mut TestAppContext) {
    init_test_app(cx);
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let state = state_with_workspace_tree("right-terminal-deny");
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

    // A denied input flips the gate, which is how a policy says no. The
    // engine holds the policy; the test drives the input the policy reads.
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.terminal_input.update(cx, |input, cx| {
                input.set_value("rm -rf /", window, cx);
            });
        });
    });
    cx.update(|_, cx| {
        shell.update(cx, |this, _cx| {
            let Some(terminal) = this.right.terminal.as_mut() else {
                return;
            };
            let policy = r#"
                package terminal

                allow := false if {
                    input.command != "ls"
                }
            "#;
            terminal
                .gate
                .load_policy("deny.rego", policy)
                .expect("the deny policy is valid Rego");
        });
    });
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.run_terminal_command(window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let lines = cx.update(|_, cx| {
        shell
            .read(cx)
            .right
            .terminal
            .as_ref()
            .map(|terminal| terminal.screen.lines.clone())
            .unwrap_or_default()
    });
    assert!(
        lines.iter().any(|line| line.ends_with("$ rm -rf /")),
        "the refused command is still echoed: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("refused by policy")),
        "and the refusal is on the screen"
    );
}
