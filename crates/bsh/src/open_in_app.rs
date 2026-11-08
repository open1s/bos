//! Opening the active workspace in an installed application.
//!
//! The reference's Session header carries ONE split button, not a button beside
//! a menu: the main half opens the workspace in the remembered application and
//! the chevron lists every catalog application the machine actually has, with
//! the one the control would use marked "({app} default)" in its own wording.
//! The catalog and its order are the reference's own
//! (`dsh-host-open-in-app`'s `OPEN_IN_APP_CATALOG`): file managers, editors and
//! IDEs, Git GUIs, then terminals - so the menu order is a property of the
//! product rather than of this shell, and a new application is a row here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::i18n::Lang;
use crate::settings::OPENER;

/// An application the active workspace can be opened in.
#[derive(Clone, Debug)]
pub(crate) struct App {
    /// The catalog id the remembered choice stores, and the icon cache key.
    pub(crate) id: &'static str,
    /// How the row is named: a product name printed verbatim in every locale,
    /// or the dictionary key the applications that ship with the OS use.
    label: Label,
    /// The bundle to launch, or none when the launch is the shell's own open
    /// verb - the file manager's reveal.
    bundle: Option<PathBuf>,
}

/// How one row is named.
#[derive(Clone, Copy, Debug)]
enum Label {
    /// A product name, identical in every language (the reference's
    /// `PRODUCT_NAMES`). Only the macOS catalog names products today: the other
    /// platforms resolve the file manager alone.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Product(&'static str),
    /// A dictionary key, for the applications that ship with the OS.
    Key(&'static str),
}

impl App {
    /// The row's wording in the active language.
    pub(crate) fn label(&self, lang: Lang) -> String {
        match self.label {
            Label::Product(name) => name.to_string(),
            Label::Key(key) => lang.tr(key),
        }
    }

    /// The application's icon, as the warm-up cache holds it.
    ///
    /// The reference serves the icon from the host's catalog; bsh asks AppKit
    /// for the same bundle icon and hands GPUI PNG bytes, because GPUI's
    /// decoder reads PNG and not `.icns`. None means GPUI draws the reference's
    /// own fallback glyph instead of an application image.
    ///
    /// This is a pure read by design. Decoding asks AppKit for the bundle image
    /// and re-encodes it, which measured 0.2-3 s per application - Finder alone
    /// took 3.0 s while the icon service cold-started - so a caller on the
    /// render or click path must never pay it. [`App::warm`] fills the cache
    /// from a background task instead.
    pub(crate) fn cached_icon(&self) -> Option<Arc<gpui_kit::Image>> {
        icons().lock().ok()?.get(self.id).cloned().flatten()
    }

    /// Decode this application's icon into the process cache.
    ///
    /// Idempotent, and the only place an icon is decoded: the first menu open
    /// used to run this for every installed application on the UI thread, so a
    /// six-application catalog stalled the click for 4.4 seconds.
    pub(crate) fn warm(&self) -> Option<Arc<gpui_kit::Image>> {
        #[cfg(target_os = "macos")]
        {
            if let Some(hit) = self.cached_icon() {
                return Some(hit);
            }
            let decoded = self.icon_bundle().and_then(|bundle| load_icon(&bundle));
            if let Ok(mut icons) = icons().lock() {
                icons.insert(self.id, decoded.clone());
            }
            decoded
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    /// The bundle whose icon stands for this application.
    ///
    /// The file manager's launch is the shell's open verb, so it carries no
    /// bundle of its own; the OS still draws its icon from Finder.app.
    fn icon_bundle(&self) -> Option<PathBuf> {
        match &self.bundle {
            Some(bundle) => Some(bundle.clone()),
            #[cfg(target_os = "macos")]
            None => Some(PathBuf::from(FINDER_BUNDLE)),
            #[cfg(not(target_os = "macos"))]
            None => None,
        }
    }
}

/// Decoded icons by catalog id, filled by the launch warm-up and read by the views.
///
/// One process-wide map rather than a field on each `App`, so a decoded PNG is
/// never held twice. The lock only guards a lookup or an insert and is released
/// around the decode itself, so a cold read never waits behind AppKit.
type IconCache = Mutex<HashMap<&'static str, Option<Arc<gpui_kit::Image>>>>;
static ICONS: OnceLock<IconCache> = OnceLock::new();

fn icons() -> &'static IconCache {
    ICONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The edge an application icon is decoded at, four times the largest drawn
/// size (14px in a menu row) so a scaled-down draw stays crisp.
#[cfg(target_os = "macos")]
const ICON_PX: u32 = 64;

/// Where the OS keeps the bundle of the application that reveals a folder.
#[cfg(target_os = "macos")]
const FINDER_BUNDLE: &str = "/System/Library/CoreServices/Finder.app";
/// Where the OS keeps its own terminal, which needs no user install.
#[cfg(target_os = "macos")]
const TERMINAL_BUNDLE: &str = "/System/Applications/Utilities/Terminal.app";

/// One catalog row.
struct Entry {
    /// The id the remembered choice stores.
    id: &'static str,
    /// The row's wording.
    label: Label,
    /// How the application is found.
    source: Source,
}

/// How a catalog row resolves to an installed application.
enum Source {
    /// The OS file manager: the launch is the shell's own open verb, which
    /// reveals the folder, and nothing has to be installed for it to exist.
    Shell,
    /// A bundle the OS ships at a fixed path; absent means the row is hidden.
    Fixed(&'static str),
    /// Bundle names probed under the application roots, in order.
    Bundles(&'static [&'static str]),
}

/// The reference's launch catalog in menu order: file managers, editors and
/// IDEs, Git GUIs, terminals.
#[cfg(target_os = "macos")]
const CATALOG: [Entry; 28] = [
    Entry {
        id: "finder",
        label: Label::Key("app.finder"),
        source: Source::Shell,
    },
    Entry {
        id: "cursor",
        label: Label::Product("Cursor"),
        source: Source::Bundles(&["Cursor.app"]),
    },
    Entry {
        id: "vscode",
        label: Label::Product("VS Code"),
        source: Source::Bundles(&["Visual Studio Code.app"]),
    },
    Entry {
        id: "vscodeinsiders",
        label: Label::Product("VS Code Insiders"),
        source: Source::Bundles(&["Visual Studio Code - Insiders.app"]),
    },
    Entry {
        id: "windsurf",
        label: Label::Product("Windsurf"),
        source: Source::Bundles(&["Windsurf.app"]),
    },
    Entry {
        id: "zed",
        label: Label::Product("Zed"),
        source: Source::Bundles(&["Zed.app", "Zed Preview.app"]),
    },
    Entry {
        id: "sublimetext",
        label: Label::Product("Sublime Text"),
        source: Source::Bundles(&["Sublime Text.app"]),
    },
    Entry {
        id: "xcode",
        label: Label::Product("Xcode"),
        source: Source::Bundles(&["Xcode.app"]),
    },
    Entry {
        id: "androidstudio",
        label: Label::Product("Android Studio"),
        source: Source::Bundles(&["Android Studio.app"]),
    },
    Entry {
        id: "intellij",
        label: Label::Product("IntelliJ IDEA"),
        source: Source::Bundles(&[
            "IntelliJ IDEA.app",
            "IntelliJ IDEA Ultimate.app",
            "IntelliJ IDEA CE.app",
        ]),
    },
    Entry {
        id: "pycharm",
        label: Label::Product("PyCharm"),
        source: Source::Bundles(&[
            "PyCharm.app",
            "PyCharm Professional.app",
            "PyCharm CE.app",
            "PyCharm Community.app",
        ]),
    },
    Entry {
        id: "webstorm",
        label: Label::Product("WebStorm"),
        source: Source::Bundles(&["WebStorm.app"]),
    },
    Entry {
        id: "phpstorm",
        label: Label::Product("PhpStorm"),
        source: Source::Bundles(&["PhpStorm.app"]),
    },
    Entry {
        id: "goland",
        label: Label::Product("GoLand"),
        source: Source::Bundles(&["GoLand.app"]),
    },
    Entry {
        id: "rider",
        label: Label::Product("Rider"),
        source: Source::Bundles(&["Rider.app", "JetBrains Rider.app"]),
    },
    Entry {
        id: "rustrover",
        label: Label::Product("RustRover"),
        source: Source::Bundles(&["RustRover.app"]),
    },
    Entry {
        id: "fork",
        label: Label::Product("Fork"),
        source: Source::Bundles(&["Fork.app"]),
    },
    Entry {
        id: "sourcetree",
        label: Label::Product("Sourcetree"),
        source: Source::Bundles(&["Sourcetree.app"]),
    },
    Entry {
        id: "github",
        label: Label::Product("GitHub Desktop"),
        source: Source::Bundles(&["GitHub Desktop.app"]),
    },
    Entry {
        id: "tower",
        label: Label::Product("Tower"),
        source: Source::Bundles(&["Tower.app"]),
    },
    Entry {
        id: "gitkraken",
        label: Label::Product("GitKraken"),
        source: Source::Bundles(&["GitKraken.app"]),
    },
    Entry {
        id: "smartgit",
        label: Label::Product("SmartGit"),
        source: Source::Bundles(&["SmartGit.app"]),
    },
    Entry {
        id: "sublimemerge",
        label: Label::Product("Sublime Merge"),
        source: Source::Bundles(&["Sublime Merge.app"]),
    },
    Entry {
        id: "ghostty",
        label: Label::Product("Ghostty"),
        source: Source::Bundles(&["Ghostty.app"]),
    },
    Entry {
        id: "warp",
        label: Label::Product("Warp"),
        source: Source::Bundles(&["Warp.app"]),
    },
    Entry {
        id: "iterm",
        label: Label::Product("iTerm2"),
        source: Source::Bundles(&["iTerm.app"]),
    },
    Entry {
        id: "kitty",
        label: Label::Product("kitty"),
        source: Source::Bundles(&["kitty.app"]),
    },
    Entry {
        id: "terminal",
        label: Label::Key("app.terminal"),
        source: Source::Fixed(TERMINAL_BUNDLE),
    },
];

/// The catalog the shell can resolve on this platform.
///
/// The reference declares a per-platform catalog; bsh resolves bundles under
/// the macOS application roots, so the only entry a non-macOS build can launch
/// is the file manager the shell's own open verb drives.
#[cfg(not(target_os = "macos"))]
const CATALOG: [Entry; 1] = [Entry {
    id: "filemanager",
    label: Label::Key("app.filemanager"),
    source: Source::Shell,
}];

/// The directories macOS installs applications into, the system one first.
///
/// The reference probes `/Applications` and then `~/Applications`, which is
/// also the precedence a bundle name installed in both takes.
fn application_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        roots
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// The installed applications, in menu order.
///
/// Probed once per process: a bundle directory does not appear or disappear
/// mid-run often enough to justify a disk read on the render path, and the
/// reference reads its own catalog once per page for the same reason.
pub(crate) fn installed() -> &'static [App] {
    static INSTALLED: OnceLock<Vec<App>> = OnceLock::new();
    INSTALLED.get_or_init(|| installed_in(&application_roots()))
}

/// The catalog's installed rows, in menu order.
fn installed_in(roots: &[PathBuf]) -> Vec<App> {
    CATALOG
        .iter()
        .filter_map(|entry| {
            let bundle = match entry.source {
                Source::Shell => None,
                Source::Fixed(path) => {
                    let path = PathBuf::from(path);
                    if !path.is_dir() {
                        return None;
                    }
                    Some(path)
                }
                Source::Bundles(names) => Some(
                    roots
                        .iter()
                        .flat_map(|root| names.iter().map(move |name| root.join(name)))
                        .find(|bundle| bundle.is_dir())?,
                ),
            };
            Some(App {
                id: entry.id,
                label: entry.label,
                bundle,
            })
        })
        .collect()
}

/// The row the control launches: the remembered application while it is still
/// installed, else the first offered one.
pub(crate) fn preferred_index(apps: &[App], remembered: &str) -> Option<usize> {
    if apps.is_empty() {
        return None;
    }
    Some(
        apps.iter()
            .position(|app| app.id == remembered)
            .unwrap_or(0),
    )
}

/// The command that opens a folder in the application.
pub(crate) fn open_command(app: &App, path: &str) -> std::process::Command {
    let mut command = std::process::Command::new(OPENER);
    match &app.bundle {
        #[cfg(target_os = "macos")]
        Some(bundle) => {
            command.arg("-a").arg(bundle);
        }
        _ => {}
    }
    command.arg(path);
    command
}

/// Open a folder in the application.
pub(crate) fn open(app: &App, path: &str) -> std::io::Result<()> {
    open_command(app, path).spawn().map(|_| ())
}

/// How many bundle icons have been decoded in this process.
///
/// Test-only, and the proof that the view path reads the cache instead of
/// paying AppKit on a click: a menu open must not move this counter.
#[cfg(test)]
static ICON_DECODES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The number of bundle icons decoded so far, for the cache-miss test.
#[cfg(test)]
pub(crate) fn icon_decodes() -> usize {
    ICON_DECODES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Decode an application bundle's icon into the PNG bytes GPUI draws.
///
/// AppKit is asked for the bundle icon the Finder and the Dock show, which is
/// the same artwork the reference's host serves from its catalog's
/// `app-bundle` icon claim; the returned NSImage is then rasterised and
/// re-encoded, because GPUI's decoder reads PNG and not `.icns`.
#[cfg(target_os = "macos")]
fn load_icon(bundle: &std::path::Path) -> Option<Arc<gpui_kit::Image>> {
    use gpui_kit::{Image, ImageFormat};
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSString};

    #[cfg(test)]
    ICON_DECODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let path = NSString::from_str(bundle.to_str()?);
    let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
    icon.setSize(objc2_foundation::NSSize::new(64.0, 64.0));
    let tiff = icon.TIFFRepresentation()?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
    // SAFETY: the dictionary is empty, so it carries no property the encoder
    // would have to interpret, and its type is the one the method declares.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    // The bundle's largest representation is 1024px while the control draws 13
    // or 14, so the icon is downscaled once here: six 4 MB textures behind a
    // 24px menu would waste exactly what the cache exists to hold.
    // `thumbnail` averages the source down rather than point-sampling it,
    // which is what keeps an application icon recognisable at 13px.
    let decoded = image::load_from_memory(&png.to_vec()).ok()?;
    let mut bytes = Vec::new();
    decoded
        .thumbnail(ICON_PX, ICON_PX)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .ok()?;
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
}

#[cfg(test)]
mod tests {
    use super::{installed_in, open_command, preferred_index, Entry, Label};
    use std::path::{Path, PathBuf};

    /// A scratch directory tree that lives only for one test.
    fn scratch(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("bsh-open-in-app-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    /// A bundle directory, which is all a probe reads.
    fn bundle(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join(name)).expect("bundle");
    }

    /// Only installed applications are offered, in catalog order: the file
    /// manager first, then editors and IDEs as the catalog lists them rather
    /// than as the filesystem happens to answer.
    #[test]
    fn only_installed_applications_are_offered_in_catalog_order() {
        let system = scratch("system");
        let user = scratch("user");
        bundle(&system, "Zed.app");
        bundle(&system, "Visual Studio Code.app");
        bundle(&user, "PyCharm.app");
        bundle(&system, "Safari.app");

        let installed = installed_in(&[system.clone(), user.clone()]);
        let ids: Vec<&str> = installed.iter().map(|app| app.id).collect();
        assert_eq!(ids.first(), Some(&"finder"), "the file manager leads");
        let at = |id: &str| {
            ids.iter()
                .position(|candidate| *candidate == id)
                .unwrap_or_else(|| panic!("{id} is installed: {ids:?}"))
        };
        assert!(
            at("vscode") < at("zed") && at("zed") < at("pycharm"),
            "editors keep the catalog's order, not the probe's: {ids:?}"
        );
        assert!(
            !ids.contains(&"safari"),
            "an application the catalog does not name is not an editor"
        );
        assert_eq!(
            installed
                .iter()
                .find(|app| app.id == "zed")
                .map(|app| app.bundle.clone()),
            Some(Some(system.join("Zed.app"))),
            "the launch names the bundle that was found"
        );

        let _ = std::fs::remove_dir_all(&system);
        let _ = std::fs::remove_dir_all(&user);
    }

    /// The remembered application is the preferred row while it is installed,
    /// and a stale or empty choice falls back to the first one offered.
    #[test]
    fn the_remembered_application_is_the_preferred_row() {
        let root = scratch("preferred");
        bundle(&root, "Zed.app");
        bundle(&root, "Ghostty.app");
        let installed = installed_in(std::slice::from_ref(&root));
        let ghostty = installed
            .iter()
            .position(|app| app.id == "ghostty")
            .expect("ghostty is installed");
        assert_eq!(preferred_index(&installed, "ghostty"), Some(ghostty));
        assert_eq!(
            preferred_index(&installed, "emacs"),
            Some(0),
            "a remembered application that is gone cannot be launched"
        );
        assert_eq!(preferred_index(&[], "zed"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The launch names the bundle, and the reveal case passes the folder to
    /// the shell's own open verb with no application.
    #[test]
    fn the_open_command_names_the_bundle_and_the_folder() {
        let zed = super::App {
            id: "zed",
            label: Label::Product("Zed"),
            bundle: Some(PathBuf::from("/Applications/Zed.app")),
        };
        let command = open_command(&zed, "/tmp/ws");
        assert_eq!(command.get_program(), crate::settings::OPENER);
        #[cfg(target_os = "macos")]
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["-a", "/Applications/Zed.app", "/tmp/ws"]
        );

        let finder = super::App {
            id: "finder",
            label: Label::Key("app.finder"),
            bundle: None,
        };
        let command = open_command(&finder, "/tmp/ws");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["/tmp/ws"],
            "the file manager is the shell's own open verb"
        );
    }

    /// The catalog's ids are what the settings file stores, so they have to be
    /// unique: a duplicate would make the remembered choice ambiguous.
    #[test]
    fn every_catalog_id_is_unique() {
        for (index, entry) in super::CATALOG.iter().enumerate() {
            let before = &super::CATALOG[..index];
            assert!(
                !before.iter().any(|other| other.id == entry.id),
                "{} appears twice",
                entry.id
            );
        }
    }

    /// An entry exists for every id the reference's own catalog names, in the
    /// same order, so the menu cannot silently lose a product.
    #[test]
    fn the_catalog_covers_the_reference_rows() {
        let ids: Vec<&str> = super::CATALOG.iter().map(|Entry { id, .. }| *id).collect();
        #[cfg(target_os = "macos")]
        assert_eq!(
            ids,
            vec![
                "finder",
                "cursor",
                "vscode",
                "vscodeinsiders",
                "windsurf",
                "zed",
                "sublimetext",
                "xcode",
                "androidstudio",
                "intellij",
                "pycharm",
                "webstorm",
                "phpstorm",
                "goland",
                "rider",
                "rustrover",
                "fork",
                "sourcetree",
                "github",
                "tower",
                "gitkraken",
                "smartgit",
                "sublimemerge",
                "ghostty",
                "warp",
                "iterm",
                "kitty",
                "terminal",
            ]
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(ids, vec!["filemanager"]);
    }

    /// The view path is a pure cache read: a cold entry draws the fallback
    /// glyph instead of paying AppKit on a click.
    ///
    /// This is the regression guard for the first-click stall: the menu rows and
    /// the header's main half call `cached_icon`, and only the launch warm-up
    /// may decode. Measured before the split: 4.4 s for the six installed
    /// applications, Finder alone 3.0 s.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_cold_read_never_decodes_an_icon() {
        let app = super::App {
            id: "probe-not-installed",
            label: Label::Product("Probe"),
            bundle: Some(PathBuf::from("/Applications/Probe.app")),
        };
        let before = super::icon_decodes();
        assert!(
            app.cached_icon().is_none(),
            "an application the warm-up never saw has no icon"
        );
        assert_eq!(
            super::icon_decodes(),
            before,
            "reading an icon must never reach AppKit"
        );
    }
}
