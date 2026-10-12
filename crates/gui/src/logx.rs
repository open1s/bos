//! One append-only log for the GUI: the webview's own lines (through
//! [`ui_log`]) and the backend's decisions land side by side in
//! `~/.bos/gui/log/gui.log` — the rotating file the [`logging`] crate
//! produces for this process (`run` initializes it under the GUI's own
//! subtree) — so a report that "the menu does nothing" can be answered from
//! one ordered log instead of from a guess.
//!
//! A line is `<target>: <message>` under the record's level; writes never
//! fail into the caller: a logging mistake must not become the bug being
//! investigated.

/// Append one line to the GUI log under the `ui` target, which the process's
/// log filter keeps at info so the trail survives an error-level default.
/// Cheap, infallible, best-effort: without an initialized logger the `log`
/// facade simply drops the record.
pub(crate) fn write_line(level: &str, target: &str, message: &str) {
    match level {
        "ERROR" => log::error!(target: "ui", "{target}: {message}"),
        "WARN" => log::warn!(target: "ui", "{target}: {message}"),
        _ => log::info!(target: "ui", "{target}: {message}"),
    }
}

/// A log line from the webview, through the same file as the backend's own.
///
/// The frontend funnels every user-visible decision and every command outcome
/// through this — which action a row-menu pick ran, what `window.confirm`
/// answered, what a command returned — so the two sides of an IPC seam can be
/// read in one place, in order.
#[tauri::command]
pub(crate) fn ui_log(level: String, target: String, message: String) {
    let level = match level.as_str() {
        "error" => "ERROR",
        "warn" => "WARN",
        _ => "INFO",
    };
    write_line(level, &format!("ui/{target}"), &message);
}

#[cfg(test)]
mod tests {
    /// The log line's shape is the interface a grep depends on; pin it.
    #[test]
    fn write_line_never_panics_without_home() {
        // HOME is present on every dev machine, but the point is only that the
        // function is infallible: it must not panic whatever the environment.
        super::write_line("INFO", "test", "a line from a test");
    }
}
