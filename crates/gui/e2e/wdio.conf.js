/**
 * WebdriverIO configuration for the BOS desktop app.
 *
 * The driver is `tauri-driver`, which speaks WebDriver to the webview the app
 * embeds: WebKitWebDriver on Linux and msedgedriver on Windows. **It has no
 * macOS backend** — a WKWebView app cannot be driven this way on a Mac — so
 * this suite runs on Linux (CI uses xvfb) or Windows. The same feature points
 * are covered on any machine by `crates/gui/tests/page_selftest.sh`, which
 * drives the shipped page with a recording bridge double.
 *
 *   cargo build -p bsh                     # produce target/debug/bsh
 *   cd crates/gui/e2e && npm install
 *   xvfb-run --auto-servernum npm run e2e  # Linux
 *
 * Set BOS_APP_BINARY to test a different build, or TAURI_DRIVER_URL when the
 * driver is already running somewhere (then nothing is spawned here).
 */
const path = require("node:path");
const net = require("node:net");
const { spawn } = require("node:child_process");

const appBinary =
  process.env.BOS_APP_BINARY || path.resolve(__dirname, "../../../target/debug/bsh");
const externalDriver = process.env.TAURI_DRIVER_URL || "";
const port = Number(process.env.TAURI_DRIVER_PORT || 4444);
const nativeDriver =
  process.env.TAURI_NATIVE_DRIVER || (process.platform === "win32" ? "" : "/usr/bin/WebKitWebDriver");

let driverProcess = null;

const waitForPort = (host, targetPort, timeoutMs, spawnFailure) =>
  new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs;
    const attempt = () => {
      const socket = net.connect({ host, port: targetPort });
      socket.once("connect", () => {
        socket.destroy();
        resolve();
      });
      socket.once("error", () => {
        socket.destroy();
        // A failed spawn is terminal and worth reporting as itself: retrying a
        // missing binary for thirty seconds only buries the real message.
        if (spawnFailure && spawnFailure()) {
          reject(missingDriver(spawnFailure()));
          return;
        }
        if (Date.now() > deadline) {
          reject(new Error(`tauri-driver never listened on ${host}:${targetPort}`));
        } else {
          setTimeout(attempt, 250);
        }
      });
    };
    attempt();
  });

const missingDriver = (err) =>
  new Error(
    `could not start tauri-driver (${err.message}). Install it with ` +
      "`cargo install tauri-driver`, and note that tauri-driver has no macOS " +
      "backend: run this suite on Linux or Windows (see e2e/README.md).",
  );

exports.config = {
  runner: "local",
  hostname: "127.0.0.1",
  port,
  path: "/",
  specs: ["./specs/**/*.spec.js"],
  exclude: [],
  maxInstances: 1,
  logLevel: process.env.WDIO_LOG_LEVEL || "warn",
  waitforTimeout: 20000,
  connectionRetryTimeout: 180000,
  connectionRetryCount: 2,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    // A cold GUI start plus a real backend round trip is slower than a browser.
    timeout: 120000,
  },
  capabilities: [
    {
      maxInstances: 1,
      "tauri:options": { application: appBinary },
    },
  ],

  onPrepare: async () => {
    if (externalDriver) return;
    const args = ["--port", String(port)];
    if (nativeDriver) args.push("--native-driver", nativeDriver);
    let spawnFailure = null;
    driverProcess = spawn("tauri-driver", args, { stdio: "inherit" });
    driverProcess.once("error", (err) => {
      spawnFailure = err;
    });
    await waitForPort("127.0.0.1", port, 30000, () => spawnFailure);
  },

  onComplete: () => {
    if (driverProcess && !driverProcess.killed) driverProcess.kill();
  },
};
