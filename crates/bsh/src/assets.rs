//! Product artwork.
//!
//! The files under `assets/` are compiled into the binary, so a built `bsh`
//! carries its own logo and needs nothing on disk. The `.icns` and `.ico` next
//! to them are bundle material: `make-dev-app.sh` gives the macOS build its
//! Dock icon from the former, and Windows packaging reads the latter. The bare
//! binary has no bundle to read, so [`install_dock_icon`] hands the same
//! `.icns` to AppKit at startup.

use std::sync::{Arc, OnceLock};

use gpui_kit::{Image, ImageFormat};

/// The product logo, decoded once and shared by every caller.
pub(crate) fn logo() -> Arc<Image> {
    static LOGO: OnceLock<Arc<Image>> = OnceLock::new();
    LOGO.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            include_bytes!("assets/bos-harness-logo.svg").to_vec(),
        ))
    })
    .clone()
}

/// Give the process its Dock icon.
///
/// `make-dev-app.sh` sets `CFBundleIconFile` for `BSH.app`, but the bare
/// `target/debug/bsh` the capture recipe launches has no bundle, so macOS
/// draws a generic placeholder for it. This decodes the same `.icns` that the
/// bundle would use — from bytes compiled into the binary — and sets it as
/// `NSApplication.applicationIconImage`, so the app carries its logo either
/// way. No-op on platforms whose desktop shell takes the icon from packaging
/// (Windows reads the `.ico` next to this file).
///
/// Timing is part of the contract: call this only **after** the platform has
/// created the shared application. GPUI builds its `NSApplication` singleton
/// through its own `GPUIApplication` subclass (gpui-pre-macos `platform.rs`),
/// and the first `+sharedApplication` message decides which class Cocoa
/// instantiates — calling the plain binding first would create a bare
/// `NSApplication` without the platform's `platform` ivar and panic when the
/// run loop looks it up. Once the subclass instance exists, the plain binding
/// safely returns it, which is why this runs inside the launch callback in
/// `ui::run`.
pub(crate) fn install_dock_icon() {
    #[cfg(target_os = "macos")]
    {
        use objc2::{AnyThread, MainThreadMarker};
        use objc2_app_kit::{NSApplication, NSImage};
        use objc2_foundation::NSData;

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let data = NSData::with_bytes(include_bytes!("assets/bos-harness-icon.icns"));
        let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
            return;
        };
        // SAFETY: `icon` is a freshly decoded NSImage we keep alive for the
        // call, which is the whole contract of the setter.
        unsafe {
            NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&icon));
        }
    }
}
