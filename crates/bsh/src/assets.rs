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

/// The headline mark: the mantis-shrimp logo the product is branded with.
///
/// Cut from the 2048px source sheet - its near-white background flood-filled to
/// transparency, trimmed to the subject, squared, scaled to 128 - because a 3 MB
/// sheet is not something a binary should carry for a 34px draw.
pub(crate) fn mark() -> Arc<Image> {
    static MARK: OnceLock<Arc<Image>> = OnceLock::new();
    MARK.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("assets/bsh-mark.png").to_vec(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The headline mark is the mantis-shrimp emblem: an embedded PNG of the
    /// measured draw size, with an alpha channel so the sheet's white paper is
    /// gone rather than drawn as a light square on a dark theme.
    #[test]
    fn the_headline_mark_is_an_embedded_transparent_png() {
        let image = mark();
        assert!(matches!(image.format, ImageFormat::Png));
        let bytes = &image.bytes;
        assert_eq!(&bytes[1..4], b"PNG", "a real PNG signature");
        assert_eq!(&bytes[12..16], b"IHDR", "the header comes first");
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        assert_eq!((width, height), (128, 128), "the cut-out square");
        // Colour type 6 is RGBA; without it the white sheet would render as a
        // pale tile behind the emblem on every dark theme.
        assert_eq!(bytes[25], 6, "RGBA");
        // A 3 MB source sheet must never ride in the binary for a 34px draw.
        assert!(
            bytes.len() < 64 * 1024,
            "the embedded mark stays small: {} bytes",
            bytes.len()
        );
    }
}
