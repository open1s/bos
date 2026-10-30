//! The terminal pane's native backend: one libghostty surface per pane.
//!
//! Ghostty draws this itself - a Metal layer inside the window's AppKit view -
//! so this module is the seam between bsh and that surface: it creates the
//! surface at the pane's measured bounds, forwards keys, pointer and scroll
//! (Ghostty owns selection, scrollback and mouse reporting), applies bsh's
//! palette as the terminal theme, and pumps the wakeups Ghostty needs to
//! present frames.
//!
//! bsh does NOT use gpui_libghostty::bind_gpui!. The generated adapter owns its
//! own key handler, and the terminal policy has to see a submitted line before
//! the shell does, so the pane keeps the key path (see 'terminal_key' in
//! 'right_panel.rs') and this entity is a surface plus a pump, built from the
//! same boundary the macro expands to. Everything ghostty-specific stays here:
//! the pane hands it gpui types only.

use std::ffi::CString;
use std::sync::Arc;

use gpui_kit::*;
use gpui_libghostty::__private::{
    spawn_surface, KeyAction, Modifiers, MouseButton as GhosttyButton,
    NativeSurface, TerminalThemeState,
};
use gpui_libghostty::{
    ClipboardApproval, ClipboardOperation, TerminalColor, TerminalConfiguration, TerminalOptions,
    TerminalTheme,
};

use crate::terminal::Emission;

/// One pane's libghostty surface.
pub(crate) struct TerminalView {
    surface: NativeSurface,
    theme: TerminalThemeState,
    /// The theme last applied, so a same-palette tick costs one compare.
    applied: TerminalTheme,
    /// The pane's last measured bounds, in window coordinates: pointer events
    /// arrive in those coordinates and Ghostty wants them pane-relative.
    frame: Bounds<Pixels>,
    /// The wakeup loop that drives Ghostty's frame pump.
    pump: Option<Task<()>>,
    visible: bool,
    focused: bool,
}

impl TerminalView {
    /// Create the surface for one pane and its shell.
    ///
    /// The start gate has already been asked by the model; this either returns
    /// a live surface or the sentence Ghostty failed with, which the pane shows
    /// in its status line.
    pub(crate) fn spawn<T: 'static>(
        mut options: TerminalOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Result<Entity<Self>, String> {
        let theme = terminal_theme();
        options.configuration = TerminalConfiguration::Custom(theme);
        // Ghostty's own confirmations still apply; bsh answers the ones it
        // asks. Writing the clipboard (a copy) and an unsafe paste are the
        // reader's own gestures, so they are approved; a protected READ is a
        // program asking for the clipboard, which this pane refuses.
        options.clipboard_approval = Some(Arc::new(|request: ClipboardApproval<'_>| {
            request.operation != ClipboardOperation::Read
        }));
        // macOS prints a login banner before the shell; the pane has never
        // shown one, and its first output is the shell's prompt.
        options.quiet_login = true;
        // SAFETY: GPUI owns the parent view; this entity services and drops its
        // native child on the window's UI thread, as the generated adapter does.
        let (surface, theme_state) = unsafe {
            spawn_surface(options, window, f64::from(window.scale_factor()))?
        };
        Ok(cx.new(|_| Self {
            surface,
            theme: theme_state,
            applied: theme,
            frame: Bounds::default(),
            pump: None,
            visible: true,
            focused: false,
        }))
    }

    /// Whether the shell behind the surface is alive.
    pub(crate) fn is_alive(&self) -> bool {
        self.surface.is_alive()
    }

    /// Show or hide the native child without changing GPUI's focus.
    ///
    /// A hidden surface is not just invisible: it is a layer of the window's
    /// view, so another pane's tab has to hide it or the terminal keeps drawing
    /// over the tab that replaced it.
    pub(crate) fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.surface.set_visible(visible);
        self.surface.set_focus(visible && self.focused);
    }

    /// Mirror GPUI's focus so Ghostty colours its cursor and reports focus.
    pub(crate) fn set_focused(&mut self, focused: bool) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        self.surface.set_focus(self.visible && focused);
    }

    /// Place the surface at the pane's measured bounds.
    ///
    /// Called from the paint pass with window coordinates, which is exactly
    /// what the shim converts into the parent view's own frame.
    pub(crate) fn update_frame(&mut self, bounds: Bounds<Pixels>, scale_factor: f64) {
        self.frame = bounds;
        self.surface.set_frame(
            f64::from(f32::from(bounds.origin.x)),
            f64::from(f32::from(bounds.origin.y)),
            f64::from(f32::from(bounds.size.width)),
            f64::from(f32::from(bounds.size.height)),
            scale_factor,
        );
        // The shim reveals the view on its first frame; keep an explicit hide
        // (a background tab) across layout changes.
        self.surface.set_visible(self.visible);
    }

    /// Forward one keystroke down.
    pub(crate) fn key_down(&mut self, keystroke: &Keystroke, is_held: bool) {
        let action = if is_held {
            KeyAction::Repeat
        } else {
            KeyAction::Press
        };
        self.surface.send_key(
            action,
            &keystroke.key,
            keystroke.key_char.as_deref(),
            ghostty_modifiers(keystroke.modifiers),
        );
    }

    /// Forward one keystroke up (the kitty protocol reports releases).
    pub(crate) fn key_up(&mut self, keystroke: &Keystroke) {
        self.surface.send_key(
            KeyAction::Release,
            &keystroke.key,
            keystroke.key_char.as_deref(),
            ghostty_modifiers(keystroke.modifiers),
        );
    }

    /// Send Tab, for the pane's key binding.
    ///
    /// Tab is bound in the pane's context (AppKit would otherwise hand it to
    /// the key-view loop and move focus out of the pane), so it never reaches
    /// the ordinary key path; the shell still has to see the byte.
    pub(crate) fn tab(&mut self) {
        self.surface
            .send_key(KeyAction::Press, "tab", None, Modifiers::empty());
    }

    /// Write what the input gate decided: a carriage return, or the shell's
    /// kill-whole-line.
    pub(crate) fn emission(&mut self, emission: Emission) {
        match emission {
            Emission::Enter => self
                .surface
                .send_key(KeyAction::Press, "enter", None, Modifiers::empty()),
            Emission::KillLine => self
                .surface
                .send_key(KeyAction::Press, "u", None, Modifiers::CONTROL),
        }
    }

    /// Write a committed composition (an IME result) to the shell.
    pub(crate) fn commit_text(&mut self, text: &str) {
        // Ghostty's text path is NUL-terminated; a NUL has no meaning in a
        // commit and would truncate it.
        let text = text.replace('\0', "");
        if let Ok(text) = CString::new(text) {
            self.surface.text(&text);
        }
    }

    /// Tell Ghostty where the pointer is, in window coordinates.
    pub(crate) fn mouse_move(&mut self, position: Point<Pixels>, modifiers: gpui::Modifiers) {
        let x = f64::from(f32::from(position.x - self.frame.origin.x));
        let y = f64::from(f32::from(position.y - self.frame.origin.y));
        self.surface.mouse_position(x, y, ghostty_modifiers(modifiers));
    }

    /// A pointer button press.
    pub(crate) fn mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: gpui::MouseButton,
        modifiers: gpui::Modifiers,
    ) {
        self.mouse_move(position, modifiers);
        self.surface.mouse_button(
            gpui_libghostty::__private::MouseState::Press,
            ghostty_button(button),
            ghostty_modifiers(modifiers),
        );
    }

    /// A pointer button release.
    pub(crate) fn mouse_up(
        &mut self,
        position: Point<Pixels>,
        button: gpui::MouseButton,
        modifiers: gpui::Modifiers,
    ) {
        self.mouse_move(position, modifiers);
        self.surface.mouse_button(
            gpui_libghostty::__private::MouseState::Release,
            ghostty_button(button),
            ghostty_modifiers(modifiers),
        );
    }

    /// Forward one wheel event; Ghostty scrolls its own scrollback.
    pub(crate) fn scroll(
        &mut self,
        position: Point<Pixels>,
        delta: ScrollDelta,
        modifiers: gpui::Modifiers,
    ) {
        self.mouse_move(position, modifiers);
        let (x, y, precision) = match delta {
            ScrollDelta::Pixels(delta) => (
                f64::from(f32::from(delta.x)),
                f64::from(f32::from(delta.y)),
                true,
            ),
            ScrollDelta::Lines(delta) => (f64::from(delta.x), f64::from(delta.y), false),
        };
        self.surface.mouse_scroll(x, y, precision);
    }

    /// Re-apply the palette when the window's theme changed.
    ///
    /// Returns whether anything was applied, so the clock only notifies a
    /// redraw for a real change.
    pub(crate) fn sync_theme(&mut self, theme: TerminalTheme) -> bool {
        if theme == self.applied {
            return false;
        }
        match self.theme.apply(&mut self.surface, &theme) {
            Ok(()) => {
                self.applied = theme;
                true
            }
            // A failed theme write leaves the previous colours; the next tick
            // or the next theme switch tries again.
            Err(_) => false,
        }
    }

    /// Start the wakeup pump that lets Ghostty present its frames.
    fn ensure_pump(&mut self, cx: &mut Context<Self>) {
        if self.pump.is_some() {
            return;
        }
        self.surface.tick();
        let wakeup = self.surface.wakeup();
        let view = cx.entity().downgrade();
        self.pump = Some(cx.spawn(async move |_, cx| loop {
            wakeup.wait().await;
            // Ghostty draws its native child during the tick; GPUI has no
            // terminal pixels to repaint for this wakeup.
            if view
                .update(cx, |view, _| view.surface.tick())
                .is_err()
            {
                break;
            }
        }));
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_pump(cx);
        let view = cx.entity().downgrade();
        div().size_full().min_h(px(0.0)).child(
            canvas(
                move |bounds, window, cx| {
                    let scale_factor = f64::from(window.scale_factor());
                    let _ = view.update(cx, |view, _| view.update_frame(bounds, scale_factor));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }
}

/// The pane's terminal theme: bsh's surface colours as a Ghostty palette.
///
/// Ghostty derives everything else (cursor, selection, bold ink) from these, so
/// the pane follows the window's light or dark mode without a second theme
/// file of its own.
pub(crate) fn terminal_theme() -> TerminalTheme {
    let palette = crate::theme::terminal_palette().map(color);
    TerminalTheme::new(
        color(crate::theme::page()),
        color(crate::theme::text()),
        palette,
    )
}

fn color(rgb: u32) -> TerminalColor {
    TerminalColor::new((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn ghostty_modifiers(value: gpui::Modifiers) -> Modifiers {
    let mut result = Modifiers::empty();
    if value.shift {
        result.insert(Modifiers::SHIFT);
    }
    if value.control {
        result.insert(Modifiers::CONTROL);
    }
    if value.alt {
        result.insert(Modifiers::ALT);
    }
    if value.platform {
        result.insert(Modifiers::SUPER);
    }
    result
}

fn ghostty_button(button: gpui::MouseButton) -> GhosttyButton {
    match button {
        gpui::MouseButton::Left => GhosttyButton::Left,
        gpui::MouseButton::Right => GhosttyButton::Right,
        gpui::MouseButton::Middle => GhosttyButton::Middle,
        _ => GhosttyButton::Unknown,
    }
}
