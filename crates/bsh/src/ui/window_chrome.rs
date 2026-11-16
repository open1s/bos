//! The window chrome the host does not draw for us.
//!
//! BSH opens a frameless window: the reference's content starts at y=0 and its
//! three window buttons sit inside the sidebar's own top strip, which macOS
//! does for it through TitlebarOptions::traffic_light_position. The other
//! hosts have no such service, so the same strip, which every column that can
//! own the window's top-left corner already reserves
//! ([crate::theme::traffic_light_inset]), carries two things here:
//!
//! - the three buttons, drawn at the measured macOS positions and wired to the
//!   window operations the host understands, and
//! - the drag surface. On Linux the compositor takes the move when
//!   [Window::start_window_move] is called on the first motion of a press that
//!   landed on the strip; on Windows the strip's empty stretches answer
//!   HTCAPTION through [WindowControlArea::Drag]; on macOS AppKit owns both
//!   already.
//!
//! The Linux buttons are drawn only when the window has no window manager
//! frame: a server-decorated window already carries its own buttons, and two
//! close buttons in one corner is worse than none.

use std::cell::Cell;

use gpui_kit::*;

use super::shell::Shell;
use crate::theme;

/// The three fills, measured from the reference's own window.
const CLOSE_FILL: u32 = 0xFF5F57;
const MINIMIZE_FILL: u32 = 0xFEBC2E;
const ZOOM_FILL: u32 = 0x28C840;

/// The square a light is centred in: the 12px circle is the mark, not the
/// target, and three 20px cells are 60 of the 66px the buttons span.
const LIGHT_CELL: f32 = 20.0;

/// Which host is running, in the form the platform matrix is tested with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    /// AppKit draws the buttons and moves the window from its titlebar.
    MacOs,
    /// A transparent titlebar removes the caption buttons; nothing draws them.
    Windows,
    /// Client-side decorations are ours, server-side ones are the window
    /// manager's.
    Linux,
}

/// The host this build targets.
pub(crate) fn host() -> Host {
    if cfg!(target_os = "macos") {
        Host::MacOs
    } else if cfg!(target_os = "windows") {
        Host::Windows
    } else {
        Host::Linux
    }
}

/// Whether BSH has to draw the window buttons itself.
///
/// The platform's own answer is part of the decision: a server-decorated Linux
/// window already carries the window manager's buttons, and drawing a second
/// set would stack two close buttons in the same corner.
pub(crate) fn draws_buttons_for(host: Host, decorations: Decorations) -> bool {
    match host {
        Host::MacOs => false,
        Host::Windows => true,
        Host::Linux => matches!(decorations, Decorations::Client { .. }),
    }
}

/// Whether this frame draws the window buttons itself.
///
/// The decoration answer is read on every host and only the Linux arm of the
/// matrix looks at it: asking the window is cheap, and one path here keeps the
/// platform question in one place.
pub(crate) fn draws_buttons(window: &Window) -> bool {
    draws_buttons_for(host(), window.window_decorations())
}

/// One window button, in the order macOS draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Light {
    Close,
    Minimize,
    Zoom,
}

impl Light {
    fn fill(self) -> u32 {
        match self {
            Light::Close => CLOSE_FILL,
            Light::Minimize => MINIMIZE_FILL,
            Light::Zoom => ZOOM_FILL,
        }
    }

    /// The platform area a host that hit-tests the frame needs to name.
    #[cfg(target_os = "windows")]
    fn area(self) -> WindowControlArea {
        match self {
            Light::Close => WindowControlArea::Close,
            Light::Minimize => WindowControlArea::Min,
            Light::Zoom => WindowControlArea::Max,
        }
    }
}

/// The three window buttons, or None where the host draws its own.
pub(crate) fn buttons(window: &Window, cx: &mut Context<Shell>) -> Option<AnyElement> {
    if !draws_buttons(window) {
        return None;
    }
    let origin = lights_origin();
    Some(
        div()
            .debug_selector(|| "window-buttons".to_string())
            .absolute()
            .left(px(origin.0))
            .top(px(origin.1))
            .flex()
            .flex_row()
            .child(button("window-close", Light::Close, cx))
            .child(button("window-minimize", Light::Minimize, cx))
            .child(button("window-zoom", Light::Zoom, cx))
            .into_any_element(),
    )
}

/// The top-left corner of the first button's cell.
fn lights_origin() -> (f32, f32) {
    let origin = theme::TRAFFIC_LIGHTS_ORIGIN;
    let inset = (LIGHT_CELL - theme::TRAFFIC_LIGHT_DIAMETER) / 2.0;
    (origin.0 - inset, origin.1 - inset)
}

/// One button: a 20px cell around the measured 12px circle.
fn button(id: &'static str, light: Light, cx: &mut Context<Shell>) -> Stateful<Div> {
    let cell = div()
        .id(id)
        .debug_selector(move || id.to_string())
        .flex()
        .w(px(LIGHT_CELL))
        .h(px(LIGHT_CELL))
        .items_center()
        .justify_center();
    wire_button(cell, light, cx).child(
        div()
            .w(px(theme::TRAFFIC_LIGHT_DIAMETER))
            .h(px(theme::TRAFFIC_LIGHT_DIAMETER))
            .rounded_full()
            .bg(rgb(light.fill())),
    )
}

/// Give one button the behaviour its host understands.
fn wire_button(cell: Stateful<Div>, light: Light, cx: &mut Context<Shell>) -> Stateful<Div> {
    // Windows turns a hit test on HTCLOSE, HTMINBUTTON and HTMAXBUTTON back
    // into the window operation itself - including the maximize/restore toggle
    // and the system menu - so doing the work here as well would run it twice.
    #[cfg(target_os = "windows")]
    {
        let _ = cx;
        cell.window_control_area(light.area())
    }
    // Linux has no non-client frame to hit-test, so the button runs the same
    // operations a window manager button would.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        cell.cursor_pointer().on_click(cx.listener(
            move |shell, _event: &ClickEvent, window, _cx| {
                match light {
                    Light::Minimize => window.minimize_window(),
                    Light::Zoom => window.zoom_window(),
                    Light::Close => {
                        // The platform's own close runs on_window_should_close,
                        // which settles every streaming turn and persists the
                        // sessions before the process exits. A drawn button
                        // must take the same path; remove_window alone would
                        // drop a running turn's stream.
                        shell.state.shutdown();
                        window.remove_window();
                    }
                }
            },
        ))
    }
    // macOS never draws this overlay; the cell stays inert there.
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
    {
        let _ = (light, cx);
        cell
    }
}

thread_local! {
    /// Whether a press that landed on the title strip is waiting for its first
    /// move. Per thread for the same reason the palette is: GPUI drives one
    /// window per thread, and the gesture belongs to that window. It exists so
    /// the compositor is asked to move the window exactly once per gesture,
    /// and so a press elsewhere can never turn into a window move.
    static ARMED: Cell<bool> = const { Cell::new(false) };
    /// Where that press landed.
    static PRESSED_AT: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
}

/// How far the pointer has to travel before an armed press starts a move.
///
/// A click is a press and a release in place, so a hand that trembles while
/// pressing the fold control or a header chip must still get its click: 3px is
/// under the reach of a drag and over the jitter of a click.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub(crate) const DRAG_THRESHOLD: f32 = 3.0;

/// The strip a press has to land in to be a window move.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub(crate) fn strip_height() -> f32 {
    theme::traffic_light_inset().bottom
}

/// Whether a press at this window height belongs to the title strip.
///
/// Pure, so the rule is testable on every host even though only the hosts that
/// do not move a frameless window for us act on it.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub(crate) fn is_strip(y: f32) -> bool {
    y < strip_height()
}

/// Whether an armed press has travelled far enough to be a drag.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub(crate) fn is_drag(dx: f32, dy: f32) -> bool {
    dx * dx + dy * dy >= DRAG_THRESHOLD * DRAG_THRESHOLD
}

/// Whether a point falls on one of the three buttons.
///
/// The buttons are the strip's one exception to dragging: a press on close,
/// minimize or zoom belongs to that button, and a hand that trembles while
/// aiming at close must not turn into a window drag instead.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub(crate) fn is_over_buttons(x: f32, y: f32) -> bool {
    let (left, top) = lights_origin();
    x >= left && x < left + LIGHT_CELL * 3.0 && y >= top && y < top + LIGHT_CELL
}

/// Handle a left press on the window's title strip.
///
/// The move starts on the first motion, not on the press: a strip also carries
/// real controls (the fold button, the tabs, the header's chips), and taking
/// the pointer at press time would turn every one of their clicks into a
/// window drag. A double click on the strip zooms the window, the way a
/// double click on a titlebar does; the buttons are carved out of both, so a
/// press aiming at close stays a press on close.
pub(crate) fn strip_pressed(event: &MouseDownEvent, window: &mut Window) {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        let x = f32::from(event.position.x);
        let y = f32::from(event.position.y);
        if !is_strip(y) || is_over_buttons(x, y) {
            ARMED.set(false);
            return;
        }
        PRESSED_AT.set((x, y));
        if event.click_count >= 2 {
            ARMED.set(false);
            window.zoom_window();
        } else {
            ARMED.set(true);
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = (event, window);
    }
}

/// Start the platform move on the first motion of an armed press.
pub(crate) fn drag_moved(event: &MouseMoveEvent, window: &mut Window) {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        if event.pressed_button != Some(MouseButton::Left) || !ARMED.get() {
            return;
        }
        let (x, y) = PRESSED_AT.get();
        if is_drag(event.position.x.as_f32() - x, event.position.y.as_f32() - y) {
            ARMED.set(false);
            window.start_window_move();
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = (event, window);
    }
}

/// Drop the arm: the press ended, or moved somewhere the move was refused.
pub(crate) fn disarm_drag() {
    ARMED.set(false);
}

/// Turn an empty stretch of the strip into a Windows caption region.
///
/// Windows has no start_window_move: the window moves when the region under the
/// pointer answers HTCAPTION, which the drag hitbox arranges. The element must
/// be empty - a caption region swallows the clicks of anything drawn inside it.
pub(crate) fn drag_grip(grip: Stateful<Div>) -> Stateful<Div> {
    #[cfg(target_os = "windows")]
    {
        grip.window_control_area(WindowControlArea::Drag)
    }
    #[cfg(not(target_os = "windows"))]
    {
        grip
    }
}

#[cfg(test)]
mod tests {
    use super::{
        draws_buttons_for, is_over_buttons, is_strip, lights_origin, strip_height, Host, LIGHT_CELL,
    };
    use crate::theme;
    use gpui_kit::Decorations;

    fn client() -> Decorations {
        Decorations::Client {
            tiling: Default::default(),
        }
    }

    /// macOS draws the buttons itself, so BSH must never draw a second set.
    #[test]
    fn macos_never_draws_the_window_buttons() {
        assert!(!draws_buttons_for(Host::MacOs, client()));
        assert!(!draws_buttons_for(Host::MacOs, Decorations::Server));
    }

    /// Windows removes the caption for a transparent titlebar and does not put
    /// the buttons back, whatever decoration state it reports (the platform
    /// default answers Server even though it draws no frame).
    #[test]
    fn windows_always_draws_them() {
        assert!(draws_buttons_for(Host::Windows, client()));
        assert!(draws_buttons_for(Host::Windows, Decorations::Server));
    }

    /// On Linux the display server decides: a client-decorated window has no
    /// other buttons, a server-decorated one already carries the window
    /// manager's.
    #[test]
    fn linux_draws_them_only_under_client_decorations() {
        assert!(draws_buttons_for(Host::Linux, client()));
        assert!(!draws_buttons_for(Host::Linux, Decorations::Server));
    }

    /// The buttons land on the measured lights: the same origin, the same 12px
    /// mark, and the last cell inside the reserved zone.
    #[test]
    fn the_three_buttons_fit_the_reserved_zone() {
        let (left, top) = lights_origin();
        let diameter = theme::TRAFFIC_LIGHT_DIAMETER;
        assert_eq!(
            left,
            theme::TRAFFIC_LIGHTS_ORIGIN.0 - (LIGHT_CELL - diameter) / 2.0
        );
        assert_eq!(
            top,
            theme::TRAFFIC_LIGHTS_ORIGIN.1 - (LIGHT_CELL - diameter) / 2.0
        );
        let inset = theme::traffic_light_inset();
        assert!(
            left + LIGHT_CELL * 3.0 <= inset.right,
            "three 20px cells fit the reserved width"
        );
        let center = top + LIGHT_CELL / 2.0;
        assert_eq!(
            center,
            theme::traffic_light_center_y(),
            "the cells centre on the measured light line"
        );
        assert!(center + diameter / 2.0 <= inset.bottom);
    }

    /// The drag strip and the button zone are the same 52px band: a press that
    /// can move the window is exactly a press that landed beside the buttons.
    #[test]
    fn the_drag_strip_is_the_reserved_band() {
        let inset = theme::traffic_light_inset();
        assert_eq!(strip_height(), inset.bottom);
        assert!(strip_height() > theme::TRAFFIC_LIGHTS_ORIGIN.1 + theme::TRAFFIC_LIGHT_DIAMETER);
        assert!(is_strip(0.0), "the window's top edge is the strip");
        assert!(
            is_strip(theme::traffic_light_center_y()),
            "the buttons' centre line is the strip"
        );
        assert!(
            !is_strip(inset.bottom),
            "the strip ends where the brand row and the header's body begin"
        );
    }

    /// A click has to survive the press: travel under the threshold stays a
    /// click on the control under the pointer, and only a real drag moves the
    /// window.
    #[test]
    fn a_trembling_press_is_still_a_click() {
        assert!(!super::is_drag(0.0, 0.0), "no movement is a click");
        assert!(
            !super::is_drag(2.0, 2.0),
            "jitter under the threshold is a click"
        );
        assert!(
            !super::is_drag(super::DRAG_THRESHOLD - 0.5, 0.0),
            "the threshold is exclusive"
        );
        assert!(
            super::is_drag(super::DRAG_THRESHOLD, 0.0),
            "a real drag moves"
        );
        assert!(super::is_drag(0.0, -40.0), "any direction is a drag");
    }

    /// The three buttons are carved out of the drag: a press aiming at close
    /// must reach the button, not move the window.
    #[test]
    fn the_buttons_are_carved_out_of_the_drag_strip() {
        let (left, top) = lights_origin();
        for cell in 0..3 {
            let x = left + LIGHT_CELL * (cell as f32 + 0.5);
            let y = top + LIGHT_CELL / 2.0;
            assert!(is_strip(y), "the buttons sit inside the strip");
            assert!(is_over_buttons(x, y), "cell {cell} is a button");
        }
        assert!(
            !is_over_buttons(left - 1.0, top + 1.0),
            "the gap before the first button drags"
        );
        assert!(
            !is_over_buttons(left + LIGHT_CELL * 3.0, top + 1.0),
            "the gap after the last button drags"
        );
        assert!(
            !is_over_buttons(left + 1.0, top + LIGHT_CELL),
            "the strip below the buttons drags"
        );
    }
}
