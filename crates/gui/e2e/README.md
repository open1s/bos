# GUI end-to-end tests (tauri-driver + WebdriverIO)

These specs drive the **real application window** — the shipped `ui/` against the
real Rust backend — through WebDriver, so they assert what a user would see:
elements, labels, and the effect of a click.

```bash
cargo build -p bsh                       # produce target/debug/bsh
cd crates/gui/e2e
npm install
xvfb-run --auto-servernum npm run e2e    # Linux
npm run e2e                              # Windows (msedgedriver on PATH)
```

`BOS_APP_BINARY` selects another build, `TAURI_DRIVER_PORT` another port, and
`TAURI_DRIVER_URL` attaches to a driver you started yourself (then nothing is
spawned by the config).

## Platform reality

`tauri-driver` speaks WebDriver to the webview the app embeds: **WebKitWebDriver
on Linux and msedgedriver on Windows. It has no macOS backend** — a WKWebView
app cannot be driven this way on a Mac, so this suite is not runnable on
macOS and does not pretend to be.

That is why the same feature points are also covered by
`../tests/page_selftest.sh`, which loads the shipped page in Chrome with a
recording bridge double and drives every panel through its real handler. That
one runs anywhere (`bash crates/gui/tests/page_selftest.sh`) and is the fast
feedback loop; this suite is the integration check that also proves the Rust
commands behind the elements answer for real.

## What is covered

| Spec | Feature point | Proves |
|------|---------------|--------|
| `shell.spec.js` | chrome | sidebar/composer, Settings open+close, skip link, theme cycle repaints `<html>`, EN↔中文 round trip, `?`/Escape reference, live status line |
| `jobs.spec.js` | M7 jobs | the Automation nav reaches the panel, the list renders states or its empty note, refresh works |
| `context.spec.js` | M6 context | the meter reconciles estimate vs provider, compaction budget rows, `compact_now` reports in words, `restore_compacted` counts, spilled output lists |
| `rules.spec.js` | M4 rules | empty tool refused with a reason, add → appears with the stored predicate → revoke removes it, audit ring renders |
| `inspect.spec.js` | M11/M10/M14 | inventory has a non-zero tool count, evaluation shows a grade, replay shows metrics, `export_bundle` names the `.zip` it wrote |

## Notes

* The rules spec adds a **global** rule (`read_file` → allow) and revokes it
  again, which touches the real rules file for the duration of the test. A
  failed run could leave that one rule behind; revoke it from the Rules panel.
* The suite starts a fresh chat through the active workspace row's `+`
  (`#session-list .ws-head.active .ws-new`) for session-scoped panels, so it
  never depends on the state of an existing conversation.
* Run it under a virtual display on a headless Linux box: `xvfb-run`.
