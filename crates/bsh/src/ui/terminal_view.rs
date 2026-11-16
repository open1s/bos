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
    spawn_surface, KeyAction, Modifiers, MouseButton as GhosttyButton, NativeSurface,
    TerminalThemeState,
};
use gpui_libghostty::{
    ClipboardApproval, ClipboardOperation, TerminalColor, TerminalConfiguration, TerminalOptions,
    TerminalTheme,
};

use crate::terminal::Emission;

/// One pane's terminal backend.
///
/// macOS/Linux draw Ghostty's native surface; Windows draws a ConPTY-backed
/// software screen (issue #296), because Ghostty ships no Windows render
/// surface. The view drives whichever backend through the same pane contract.
pub(crate) enum Surface {
    /// The Ghostty native surface (macOS, Linux/Wayland).
    Ghostty(NativeSurface),
    /// The Windows ConPTY software terminal (issue #296).
    #[cfg(windows)]
    ConPty(crate::terminal_windows::ConPtyTerminal),
}

impl Surface {
    /// The Ghostty surface, if this backend is Ghostty (None for the Windows
    /// ConPTY backend). An Option so the call sites stay refutable on every
    /// platform, where the enum has a different number of variants.
    fn ghostty_mut(&mut self) -> Option<&mut NativeSurface> {
        match self {
            Surface::Ghostty(surface) => Some(surface),
            #[cfg(windows)]
            Surface::ConPty(_) => None,
        }
    }
}

/// One pane's libghostty surface.
pub(crate) struct TerminalView {
    surface: Surface,
    /// Ghostty's theme-file state; None on the Windows ConPTY backend, which
    /// re-renders its grid from the pane palette directly.
    theme: Option<TerminalThemeState>,
    /// The theme last applied, so a same-palette tick costs one compare.
    applied: TerminalTheme,
    /// The text size last applied, compared the same way.
    applied_font: f32,
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
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Result<Entity<Self>, String> {
        // Detect an unsupported platform BEFORE creating the surface, so the
        // pane shows an actionable platform message (issues #296, #297).
        crate::terminal::surface_support()?;
        let theme = terminal_theme();
        options.configuration = TerminalConfiguration::Custom(theme);
        options.font_size = Some(font_size);
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
        // Windows has no Ghostty render surface: spawn a ConPTY software
        // terminal instead (issue #296). The pane reports it as unconfined.
        #[cfg(windows)]
        {
            let _ = (&options, window, &theme);
            let (rows, cols) = initial_grid(font_size);
            let terminal = crate::terminal_windows::ConPtyTerminal::spawn(
                &crate::terminal_windows::default_shell(),
                rows,
                cols,
            )?;
            return Ok(cx.new(|_| Self {
                surface: Surface::ConPty(terminal),
                theme: None,
                applied: theme,
                applied_font: font_size,
                frame: Bounds::default(),
                pump: None,
                visible: true,
                focused: false,
            }));
        }
        // SAFETY: GPUI owns the parent view; this entity services and drops its
        // native child on the window's UI thread, as the generated adapter does.
        #[cfg(not(windows))]
        {
            let (surface, theme_state) =
                unsafe { spawn_surface(options, window, f64::from(window.scale_factor()))? };
            Ok(cx.new(|_| Self {
                surface: Surface::Ghostty(surface),
                theme: Some(theme_state),
                applied: theme,
                applied_font: font_size,
                frame: Bounds::default(),
                pump: None,
                visible: true,
                focused: false,
            }))
        }
    }

    /// Whether the shell behind the surface is alive.
    pub(crate) fn is_alive(&self) -> bool {
        match &self.surface {
            Surface::Ghostty(surface) => surface.is_alive(),
            #[cfg(windows)]
            Surface::ConPty(terminal) => terminal.is_alive(),
        }
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
        if let Some(surface) = self.surface.ghostty_mut() {
            surface.set_visible(visible);
            surface.set_focus(visible && self.focused);
        }
    }

    /// Mirror GPUI's focus so Ghostty colours its cursor and reports focus.
    pub(crate) fn set_focused(&mut self, focused: bool) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        if let Some(surface) = self.surface.ghostty_mut() {
            surface.set_focus(self.visible && focused);
        }
    }

    /// Place the surface at the pane's measured bounds.
    ///
    /// Called from the pane's measure pass with window coordinates, which is
    /// exactly what the shim converts into the parent view's own frame.
    pub(crate) fn update_frame(&mut self, bounds: Bounds<Pixels>, scale_factor: f64) {
        self.frame = bounds;
        match &mut self.surface {
            Surface::Ghostty(surface) => {
                surface.set_frame(
                    f64::from(f32::from(bounds.origin.x)),
                    f64::from(f32::from(bounds.origin.y)),
                    f64::from(f32::from(bounds.size.width)),
                    f64::from(f32::from(bounds.size.height)),
                    scale_factor,
                );
                // The shim reveals the view on its first frame; keep an explicit
                // hide (a background tab) across layout changes.
                surface.set_visible(self.visible);
            }
            #[cfg(windows)]
            Surface::ConPty(terminal) => {
                // Resize the pseudoconsole to the pane's measured cell grid.
                let (rows, cols) = grid_for_bounds(bounds, self.applied_font);
                terminal.resize(rows, cols);
            }
        }
    }

    /// Forward one keystroke down.
    pub(crate) fn key_down(&mut self, keystroke: &Keystroke, is_held: bool) {
        match &mut self.surface {
            Surface::Ghostty(surface) => {
                let action = if is_held {
                    KeyAction::Repeat
                } else {
                    KeyAction::Press
                };
                surface.send_key(
                    action,
                    &keystroke.key,
                    keystroke.key_char.as_deref(),
                    ghostty_modifiers(keystroke.modifiers),
                );
            }
            #[cfg(windows)]
            Surface::ConPty(terminal) => {
                let bytes = encode_key(
                    &keystroke.key,
                    keystroke.key_char.as_deref(),
                    keystroke.modifiers,
                );
                let _ = terminal.write_input(&bytes);
            }
        }
    }

    /// Forward one keystroke up (the kitty protocol reports releases).
    pub(crate) fn key_up(&mut self, keystroke: &Keystroke) {
        if let Some(surface) = self.surface.ghostty_mut() {
            surface.send_key(
                KeyAction::Release,
                &keystroke.key,
                keystroke.key_char.as_deref(),
                ghostty_modifiers(keystroke.modifiers),
            );
        }
        // A ConPTY shell does not consume key releases.
    }

    /// Send Tab, for the pane's key binding.
    ///
    /// Tab is bound in the pane's context (AppKit would otherwise hand it to
    /// the key-view loop and move focus out of the pane), so it never reaches
    /// the ordinary key path; the shell still has to see the byte.
    pub(crate) fn tab(&mut self) {
        match &mut self.surface {
            Surface::Ghostty(surface) => {
                surface.send_key(KeyAction::Press, "tab", None, Modifiers::empty());
            }
            #[cfg(windows)]
            Surface::ConPty(terminal) => {
                let _ = terminal.write_input(b"	");
            }
        }
    }

    /// Write what the input gate decided: a carriage return, or the shell's
    /// kill-whole-line.
    pub(crate) fn emission(&mut self, emission: Emission) {
        match &mut self.surface {
            Surface::Ghostty(surface) => match emission {
                Emission::Enter => {
                    surface.send_key(KeyAction::Press, "enter", None, Modifiers::empty())
                }
                Emission::KillLine => {
                    surface.send_key(KeyAction::Press, "u", None, Modifiers::CONTROL)
                }
            },
            #[cfg(windows)]
            Surface::ConPty(terminal) => {
                let bytes: &[u8] = match emission {
                    Emission::Enter => {
                        b"
"
                    }
                    Emission::KillLine => b"", // Ctrl-U
                };
                let _ = terminal.write_input(bytes);
            }
        }
    }

    /// Write a committed composition (an IME result) to the shell.
    pub(crate) fn commit_text(&mut self, text: &str) {
        // Ghostty's text path is NUL-terminated; a NUL has no meaning in a
        // commit and would truncate it.
        let text = text.replace('\0', "");
        match &mut self.surface {
            Surface::Ghostty(surface) => {
                if let Ok(text) = CString::new(text) {
                    surface.text(&text);
                }
            }
            #[cfg(windows)]
            Surface::ConPty(terminal) => {
                let _ = terminal.write_input(text.as_bytes());
            }
        }
    }

    /// Whether this pane draws a software screen (the Windows ConPTY backend)
    /// rather than a Ghostty native surface (issue #296).
    pub(crate) fn is_software_terminal(&self) -> bool {
        #[cfg(windows)]
        {
            matches!(self.surface, Surface::ConPty(_))
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// Copy the software screen's text to the clipboard (issue #296). Ghostty
    /// owns its own selection/clipboard, so this is a no-op for that backend.
    pub(crate) fn copy_screen(&mut self, _cx: &mut Context<Self>) {
        #[cfg(windows)]
        if let Surface::ConPty(terminal) = &self.surface {
            let text: String = terminal
                .cells()
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|c| c.text.clone())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(
                    "
",
                );
            _cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Paste the clipboard into the software terminal (issue #296). Ghostty
    /// handles its own paste, so this is a no-op for that backend.
    pub(crate) fn paste_clipboard(&mut self, _cx: &mut Context<Self>) {
        #[cfg(windows)]
        if let Surface::ConPty(terminal) = &mut self.surface {
            let text = _cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            if !text.is_empty() {
                let _ = terminal.write_input(text.as_bytes());
            }
        }
    }

    /// Tell Ghostty where the pointer is, in window coordinates.
    pub(crate) fn mouse_move(&mut self, position: Point<Pixels>, modifiers: gpui::Modifiers) {
        let x = f64::from(f32::from(position.x - self.frame.origin.x));
        let y = f64::from(f32::from(position.y - self.frame.origin.y));
        if let Some(surface) = self.surface.ghostty_mut() {
            surface.mouse_position(x, y, ghostty_modifiers(modifiers));
        }
    }

    /// A pointer button press.
    pub(crate) fn mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: gpui::MouseButton,
        modifiers: gpui::Modifiers,
    ) {
        self.mouse_move(position, modifiers);
        if let Some(surface) = self.surface.ghostty_mut() {
            surface.mouse_button(
                gpui_libghostty::__private::MouseState::Press,
                ghostty_button(button),
                ghostty_modifiers(modifiers),
            );
        }
    }

    /// A pointer button release.
    pub(crate) fn mouse_up(
        &mut self,
        position: Point<Pixels>,
        button: gpui::MouseButton,
        modifiers: gpui::Modifiers,
    ) {
        self.mouse_move(position, modifiers);
        if let Some(surface) = self.surface.ghostty_mut() {
            surface.mouse_button(
                gpui_libghostty::__private::MouseState::Release,
                ghostty_button(button),
                ghostty_modifiers(modifiers),
            );
        }
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
        if let Some(surface) = self.surface.ghostty_mut() {
            surface.mouse_scroll(x, y, precision);
        }
    }

    /// Re-apply the palette when the window's theme changed.
    ///
    /// Returns whether anything was applied, so the clock only notifies a
    /// redraw for a real change.
    pub(crate) fn sync_theme(&mut self, theme: TerminalTheme, font_size: f32) -> bool {
        if theme == self.applied && font_size == self.applied_font {
            return false;
        }
        match (&mut self.theme, &mut self.surface) {
            (Some(state), Surface::Ghostty(surface)) => {
                match state.apply(surface, &theme, Some(font_size)) {
                    Ok(()) => {
                        self.applied = theme;
                        self.applied_font = font_size;
                        true
                    }
                    // A failed theme write leaves the previous colours; the
                    // next tick or the next theme switch tries again.
                    Err(_) => false,
                }
            }
            // The ConPTY grid re-renders from the pane palette on the next
            // frame; there is no Ghostty theme file to rewrite.
            #[cfg(windows)]
            (_, Surface::ConPty(_)) => {
                self.applied = theme;
                self.applied_font = font_size;
                true
            }
            _ => false,
        }
    }

    /// Start the wakeup pump that lets Ghostty present its frames.
    fn ensure_pump(&mut self, cx: &mut Context<Self>) {
        if self.pump.is_some() {
            return;
        }
        match &mut self.surface {
            Surface::Ghostty(surface) => {
                surface.tick();
                let wakeup = surface.wakeup();
                let view = cx.entity().downgrade();
                self.pump = Some(cx.spawn(async move |_, cx| loop {
                    wakeup.wait().await;
                    // Ghostty draws its native child during the tick; GPUI has
                    // no terminal pixels to repaint for this wakeup.
                    let alive = view.update(cx, |view, _| {
                        if let Some(surface) = view.surface.ghostty_mut() {
                            surface.tick();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }));
            }
            // The ConPTY reader thread feeds the screen; repaint on a short
            // cadence so new output appears (issue #296).
            #[cfg(windows)]
            Surface::ConPty(_) => {
                let view = cx.entity().downgrade();
                self.pump = Some(cx.spawn(async move |_, cx| loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(50))
                        .await;
                    if view.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }));
            }
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_pump(cx);
        let view = cx.entity().downgrade();
        // The Windows ConPTY backend draws its software screen itself: Ghostty
        // has no Windows render surface (issue #296).
        #[cfg(windows)]
        if let Surface::ConPty(terminal) = &self.surface {
            let cells = terminal.cells();
            let font_size = self.applied_font;
            let fg = rgba(crate::theme::text());
            let bg = rgba(crate::theme::page());
            let measure_view = view.clone();
            return div()
                .size_full()
                .min_h(px(0.0))
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            let scale_factor = f64::from(window.scale_factor());
                            let _ = measure_view
                                .update(cx, |view, _| view.update_frame(bounds, scale_factor));
                        },
                        move |bounds, _state, window, cx| {
                            let line_height = px(font_size * 1.4);
                            let mut y = bounds.origin.y;
                            for row in &cells {
                                let text: String = row.iter().map(|c| c.text.clone()).collect();
                                let text = text.trim_end();
                                if !text.is_empty() {
                                    let mut style = window.text_style();
                                    style.color = fg.into();
                                    let run = style.to_run(text.len());
                                    let line = window.text_system().shape_line(
                                        text.to_string().into(),
                                        px(font_size),
                                        &[run],
                                        None,
                                    );
                                    let _ = line.paint(
                                        point(bounds.origin.x, y),
                                        line_height,
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                }
                                y += line_height;
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                .bg(bg);
        }
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

/// A 0xRRGGBB colour as a GPUI RGBA (the Windows grid renderer).
#[cfg(windows)]
fn rgba(rgb: u32) -> gpui::Rgba {
    gpui::Rgba {
        r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
        g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
        b: (rgb & 0xFF) as f32 / 255.0,
        a: 1.0,
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

/// The initial cell grid for a new Windows terminal before the pane is measured.
#[cfg(windows)]
fn initial_grid(_font_size: f32) -> (u16, u16) {
    (24, 80)
}

/// The cell grid that fits the pane's measured bounds at the current font size.
#[cfg(windows)]
fn grid_for_bounds(bounds: Bounds<Pixels>, font_size: f32) -> (u16, u16) {
    let cell_w = (font_size * 0.6).max(1.0);
    let cell_h = (font_size * 1.4).max(1.0);
    let cols = (f32::from(bounds.size.width) / cell_w).floor() as u16;
    let rows = (f32::from(bounds.size.height) / cell_h).floor() as u16;
    (rows.max(1), cols.max(1))
}

/// Encode a keystroke as the byte sequence a ConPTY shell expects (issue #296).
#[cfg(windows)]
fn encode_key(key: &str, key_char: Option<&str>, modifiers: gpui::Modifiers) -> Vec<u8> {
    // Control chords map to their C0 control byte.
    if modifiers.control {
        if let Some(ch) = key_char.and_then(|c| c.chars().next()) {
            let upper = ch.to_ascii_uppercase();
            if upper.is_ascii_uppercase() {
                return vec![(upper as u8) - b'A' + 1];
            }
        }
    }
    match key {
        "enter" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        _ => key_char.map(|c| c.as_bytes().to_vec()).unwrap_or_default(),
    }
}
