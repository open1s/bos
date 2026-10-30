//! The GPUI Kit shell.
//!
//! One view (Shell) owns the state and renders five regions plus overlays;
//! each region lives in its own module as an impl block on Shell.

pub(crate) mod audit;
mod command_menu;
pub(crate) mod composer;
pub(crate) mod diff_pane;
pub(crate) mod frame_trace;
pub(crate) mod header;
pub(crate) mod hero;
pub(crate) mod lane;
pub(crate) mod meter;
pub(crate) mod paging;
pub(crate) mod permission;
mod session_menu;
mod slash_palette;
pub(crate) mod smooth;
pub(crate) mod trash;
pub(crate) mod workspace_menu;

pub(crate) mod queue_dock;
pub(crate) mod reference_chip;
pub(crate) mod right_panel;
pub(crate) mod settings;
pub(crate) mod shell;
#[cfg(test)]
mod shell_tests;
pub(crate) mod sidebar;
pub(crate) mod statusbar;
pub(crate) mod terminal_view;
pub(crate) mod todo_panel;
pub(crate) mod trajectory;
pub(crate) mod transcript;

use gpui_kit::*;

/// Conditional builders, missing from this GPUI revision.
///
/// GPUI 0.3.x removed Div::when; the shell uses it on nearly every row, so it
/// is reintroduced here for both the plain and the stateful builder.
pub(crate) trait DivExt: Sized {
    /// Apply the builder only when the condition holds.
    fn when(self, condition: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            f(self)
        } else {
            self
        }
    }

    /// Apply the builder with a value that may be absent.
    fn when_some<T>(self, value: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => f(self, value),
            None => self,
        }
    }
}

impl DivExt for Div {}
impl<E> DivExt for Stateful<E> {}

/// A styled text leaf.
pub(crate) fn txt(text: impl Into<SharedString>, size: f32, color: u32) -> Div {
    div()
        .text_size(px(size))
        .text_color(rgb(color))
        .child(text.into())
}

/// A styled glyph leaf, for the few marks that are text rather than icons.
pub(crate) fn glyph(text: &'static str, size: f32, color: u32) -> Div {
    div().text_size(px(size)).text_color(rgb(color)).child(text)
}

/// The bundled Lucide icon names.
///
/// This is the asset crate's full catalog; the component crate's enum of the
/// same name is only the subset its own widgets use.
pub(crate) use gpui_kit::assets::IconName;

/// A line icon sized and coloured like the text around it.
///
/// The renderer takes its size and colour from the inherited text style, so
/// the returned row only has to set them.
pub(crate) fn icon(name: IconName, size: f32, color: u32) -> Div {
    div().text_size(px(size)).text_color(rgb(color)).child(name)
}

/// A raw SVG glyph, sized and coloured like a line icon.
///
/// The reference's workspace folder and disclosure triangle are not in the
/// kit's Lucide catalog, and both carry a 16px viewBox of their own, so they
/// travel as bundled SVG bytes and render through GPUI's data path.
/// `quarter_turns` turns the drawing without moving its box: the reference
/// rotates its caret 90 degrees when a workspace opens (`rotate(90deg)` on
/// `.arrow`).
pub(crate) fn asset_icon(bytes: &'static [u8], size: f32, color: u32, quarter_turns: f32) -> Div {
    let mut glyph = svg()
        .data(bytes)
        .flex_shrink_0()
        .size(px(size))
        .text_color(rgb(color));
    if quarter_turns != 0.0 {
        glyph = glyph.with_transformation(Transformation::rotate(radians(
            std::f32::consts::FRAC_PI_2 * quarter_turns,
        )));
    }
    div()
        .text_size(px(size))
        .text_color(rgb(color))
        .child(glyph)
}

/// The reference draws its hairlines at `.5px`, which the kit's whole-pixel
/// border macros cannot express, so the measured width goes straight into the
/// refinement. The kit's border macros are whole-pixel only.
pub(crate) fn hairline_bottom<E: Styled>(mut element: E) -> E {
    element.style().border_widths.bottom = Some(px(0.5).into());
    element
}

/// A `.5px` border on every side; see [`hairline_bottom`].
pub(crate) fn hairline_all<E: Styled>(mut element: E) -> E {
    let width = Some(px(0.5).into());
    let style = element.style();
    style.border_widths.top = width;
    style.border_widths.right = width;
    style.border_widths.bottom = width;
    style.border_widths.left = width;
    element
}

/// The measured reveal bar a width handle shows while the pointer is on it.
///
/// One definition for every seam in the shell: a 2px column, 72 tall, whose
/// solid 16px band sits between two 28px fades of the same colour - the
/// reference's `linear-gradient(transparent 0, scrollbar-bg-l1 8px,
/// scrollbar-bg-l1 8px, transparent)` written as three stacked layers, because
/// gpui-pre's `Background` carries exactly two stops.
pub(crate) fn handle_bar(debug: &'static str, opacity: f32) -> Div {
    let colour = crate::theme::scrollbar_bar();
    let faded = colour & 0xFFFFFF00;
    let fade = |from: u32, to: u32| {
        linear_gradient(
            180.0,
            linear_color_stop(rgba(from), 0.0),
            linear_color_stop(rgba(to), 1.0),
        )
    };
    div()
        .debug_selector(move || debug.to_string())
        .w(px(crate::theme::HANDLE_BAR_W))
        .h(px(crate::theme::HANDLE_BAR_H))
        .flex()
        .flex_col()
        .rounded(px(crate::theme::HANDLE_BAR_W))
        .opacity(opacity)
        .child(
            div()
                .w_full()
                .h(px(crate::theme::HANDLE_BAR_FADE))
                .bg(fade(faded, colour)),
        )
        .child(
            div()
                .w_full()
                .h(px(crate::theme::HANDLE_BAR_BAND))
                .bg(rgba(colour)),
        )
        .child(
            div()
                .w_full()
                .h(px(crate::theme::HANDLE_BAR_FADE))
                .bg(fade(colour, faded)),
        )
}
/// A `.5px` border on the left only; see [`hairline_bottom`].
pub(crate) fn hairline_left<E: Styled>(mut element: E) -> E {
    element.style().border_widths.left = Some(px(0.5).into());
    element
}

/// A `.5px` border on the top only; see [`hairline_bottom`].
///
/// The queue dock rules its rows apart with an inset top shadow, which GPUI has
/// no equivalent for, so the measured `.5px` goes on the row's own top edge.
pub(crate) fn hairline_top<E: Styled>(mut element: E) -> E {
    element.style().border_widths.top = Some(px(0.5).into());
    element
}

/// A `.5px` border on every side but the bottom; see [`hairline_bottom`].
///
/// The queue dock's ring is `border-bottom: none`: its bottom edge tucks under
/// the composer card, which is what keeps the panel's bottom corners square.
pub(crate) fn hairline_no_bottom<E: Styled>(mut element: E) -> E {
    let width = Some(px(0.5).into());
    let style = element.style();
    style.border_widths.top = width;
    style.border_widths.right = width;
    style.border_widths.left = width;
    element
}

/// What the command line asked the window to do.
#[derive(Clone, Default)]
pub(crate) struct Startup {
    /// Open the settings dialog as the window appears, on this rail index.
    pub(crate) settings: Option<usize>,
    /// Open the Browser pane on this URL. A deep link, so a reader (or a
    /// capture) can land on a page without typing into the address bar.
    pub(crate) browser: Option<String>,
}

impl Startup {
    /// Read the flags. Unknown flags are ignored here; `--help` is answered
    /// by the binary before the window is built.
    pub(crate) fn from_args() -> Self {
        let mut startup = Self::default();
        for arg in std::env::args().skip(1) {
            if let Some(value) = arg.strip_prefix("--settings") {
                let id = value.strip_prefix('=').unwrap_or("account");
                startup.settings = Some(settings::section_index(id).unwrap_or(0));
            } else if let Some(value) = arg.strip_prefix("--browser=") {
                let url = value.trim();
                if !url.is_empty() {
                    startup.browser = Some(url.to_string());
                }
            }
        }
        startup
    }
}

/// Open the window and run the application.
/// The app-level key bindings bsh adds on top of the kit's own.
///
/// The files editor has no save button on its surface, so cmd-s is the whole
/// save affordance; the binding is global because the action bubbles from the
/// focused editor through the files body. Tests bind it too, since they never
/// walk [`run`].
pub(crate) fn bind_global_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-s", right_panel::SaveFile, None),
        // The Browser pane's Start card prints this chord, so it has to open
        // the pane - a hint that presses nothing is a lying control.
        KeyBinding::new("cmd-t", right_panel::BrowserTab, None),
        // Tab belongs to the shell while the terminal has focus. Without a
        // binding the macOS input handler falls through to AppKit's key-view
        // loop, so focus leaves the pane and the shell never sees the byte.
        KeyBinding::new("tab", right_panel::TerminalTab, Some("terminal")),
        // The session menu's accelerators. The menu shows these keys, so they
        // have to do the same thing the row does - a hint that presses nothing
        // is a lying control.
        KeyBinding::new("alt-cmd-g", session_menu::RenameSession, None),
        KeyBinding::new("alt-cmd-f", session_menu::ForkSession, None),
        KeyBinding::new("shift-cmd-a", session_menu::ArchiveSession, None),
    ]);
}

pub(crate) fn run(startup: Startup) {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        // QuitMode::Default resolves to Explicit on macOS (gpui-pre app.rs:
        // the window-close path only quits when `cfg!(not(target_os =
        // "macos"))`), so closing the last window left the process running -
        // no window, but a live menu bar and Dock tile. bsh is a
        // single-window app: closing the window must exit it.
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            // Before init: the platform has created the shared application by
            // now (see assets::install_dock_icon for the ordering), and the
            // Dock tile should be ours from the first frame.
            crate::assets::install_dock_icon();
            // Launched from a shell the process is not the frontmost app, so its
            // window can sit fully occluded behind another one; asking to activate is
            // what a bundled launch does for us, and a photograph of the window needs
            // it on screen (CG reports only unoccluded windows).
            cx.activate(true);
            gpui_kit::init(cx);
            bind_global_actions(cx);
            gpui_kit::open_window(
                WindowOptions {
                    // The reference is a frameless window: its content starts at y=0 and
                    // the traffic lights sit inside the sidebar own top strip.
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(160.0), px(160.0)),
                        size(px(1280.0), px(820.0)),
                    ))),
                    // The reference shows its traffic lights inside the sidebar own top
                    // strip, so the title bar is transparent and the lights are placed
                    // where the reference measures them.
                    titlebar: Some(TitlebarOptions {
                        title: None,
                        appears_transparent: true,
                        traffic_light_position: Some(point(
                            px(crate::theme::TRAFFIC_LIGHTS_ORIGIN.0),
                            px(crate::theme::TRAFFIC_LIGHTS_ORIGIN.1),
                        )),
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let browser = startup.browser.clone();
                    let shell = cx.new(|cx| shell::Shell::new(window, startup.settings, cx));
                    // Closing the window while a turn runs used to take that
                    // turn with it: the process exits and nothing had saved the
                    // stream. Settle every running turn and persist every
                    // session before the close is allowed.
                    window.on_window_should_close(cx, {
                        let shell = shell.clone();
                        move |_window, cx| {
                            shell.update(cx, |shell, _| shell.state.shutdown());
                            true
                        }
                    });
                    if let Some(url) = browser {
                        // The deep link lands on the pane it names, with the
                        // same navigation a press of Go performs.
                        shell.update(cx, |shell, cx| {
                            shell.open_right_tab(right_panel::RightTab::Browser, window, cx);
                            shell.browser_go(url, cx);
                        });
                    }
                    shell
                },
            )
            .expect("bsh opens its window");
        });
}
