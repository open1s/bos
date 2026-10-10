#!/usr/bin/env node
/**
 * Fail fast, and legibly, before WebdriverIO starts.
 *
 * The suite needs four things, and when one is missing the runner's own error
 * ("Unable to connect to http://127.0.0.1:4444") names none of them. This
 * checks them in the order that produces the most useful message and exits
 * non-zero with the fix.
 */
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const problems = [];
const appBinary =
  process.env.BOS_APP_BINARY || path.resolve(__dirname, "../../../target/debug/bsh");
const externalDriver = process.env.TAURI_DRIVER_URL || "";
const supported = process.platform === "linux" || process.platform === "win32";

if (!fs.existsSync(appBinary)) {
  problems.push(
    `the app binary is missing at ${appBinary}\n` +
      "    build it first:  cargo build -p bsh\n" +
      "    (or point BOS_APP_BINARY at another build)",
  );
}

if (!supported && !externalDriver) {
  problems.push(
    `tauri-driver has no ${process.platform} backend, so it cannot drive this app here.\n` +
      "    tauri-driver speaks WebDriver to the embedded webview: WebKitWebDriver on\n" +
      "    Linux and msedgedriver on Windows only.\n" +
      "    Run this suite on Linux (CI uses xvfb-run) or Windows, or attach to an\n" +
      "    already-running driver with TAURI_DRIVER_URL.\n" +
      "    For the same feature points on this machine, run instead:\n" +
      "      bash crates/gui/tests/page_selftest.sh",
  );
}

const onPath = (name) => {
  try {
    execFileSync(process.platform === "win32" ? "where" : "which", [name], {
      stdio: "ignore",
    });
    return true;
  } catch {
    return false;
  }
};

if (supported && !externalDriver && !onPath("tauri-driver")) {
  problems.push(
    "tauri-driver is not on PATH\n    install it:  cargo install tauri-driver --locked",
  );
}

if (supported && process.platform === "linux" && !onPath("WebKitWebDriver")) {
  problems.push(
    "WebKitWebDriver is not on PATH (apt: webkit2gtk-driver)\n" +
      "    and a display is needed: on a headless box use `xvfb-run --auto-servernum`",
  );
}

if (problems.length) {
  console.error("\nGUI end-to-end suite cannot start:\n");
  for (const problem of problems) console.error(`  ✖ ${problem}\n`);
  process.exit(1);
}

console.log(`preflight ok: ${appBinary}${externalDriver ? ` via ${externalDriver}` : " with tauri-driver"}`);
