//! Per-client presentation preferences for the terminal interface.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::term::Key;

/// A saved theme choice. `Auto` follows terminal capability detection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Theme {
    /// Respect NO_COLOR, TERM and COLORTERM.
    #[default]
    Auto,
    /// Use the sixteen terminal palette slots.
    Ansi,
    /// Preserve shape and contrast without hue.
    Mono,
    /// Use the design's full RGB palette.
    Truecolor,
}

/// The command-palette key, restricted to non-printing chords.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PaletteKey {
    /// Control plus a letter.
    #[default]
    CtrlP,
    /// Control plus another letter.
    Ctrl(char),
    /// Alt plus another letter.
    Alt(char),
}

impl PaletteKey {
    /// Parse a safe chord spelling such as `ctrl+p` or `alt+g`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let (modifier, key) = text.split_once('+')?;
        if key.len() != 1 {
            return None;
        }
        let key = key.chars().next()?;
        if !key.is_ascii_alphabetic() {
            return None;
        }
        match (modifier, key) {
            ("ctrl", 'p') => Some(Self::CtrlP),
            ("ctrl", key) if matches!(key, 'g' | 'o' | 'q') => Some(Self::Ctrl(key)),
            ("alt", key) if matches!(key, 'g' | 'o' | 'q') => Some(Self::Alt(key)),
            _ => None,
        }
    }

    /// Whether a key event opens the palette.
    pub(crate) fn matches(self, key: &Key) -> bool {
        matches!((self, key), (Self::CtrlP, Key::Ctrl('p')))
            || matches!((self, key), (Self::Ctrl(a), Key::Ctrl(b)) | (Self::Alt(a), Key::Alt(b)) if a == *b)
    }

    /// Human-readable binding.
    pub(crate) fn label(self) -> String {
        match self {
            Self::CtrlP => "Ctrl+P".to_string(),
            Self::Ctrl(key) => format!("Ctrl+{}", key.to_ascii_uppercase()),
            Self::Alt(key) => format!("Alt+{}", key.to_ascii_uppercase()),
        }
    }
}

/// Preferences that belong to this terminal client, not to a Host session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
    /// Color selection.
    pub(crate) theme: Theme,
    /// Command-palette chord.
    pub(crate) palette: PaletteKey,
}

impl Preferences {
    /// Load the client's private settings; malformed or absent files use defaults.
    pub(crate) fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join("tui-preferences.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Persist settings with a same-directory replace.
    pub(crate) fn save(self, data_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
        let target = data_dir.join("tui-preferences.json");
        let temporary = data_dir.join(format!("tui-preferences.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(&self).map_err(|error| error.to_string())?;
        std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        #[cfg(windows)]
        if target.exists() {
            std::fs::remove_file(&target).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, &target).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            error.to_string()
        })
    }
}
