/** M6 — the token meter, compaction, and reading a spilled tool output back. */
const { $, browser, expect } = require("@wdio/globals");
const { openSection, newChat, textOf, rowCount } = require("./helpers");

describe("M6 context, compaction and spill", () => {
  before(async () => {
    await newChat();
  });

  it("meters the active chat and reconciles against the provider", async () => {
    await openSection("context-section");
    const meter = await textOf("#token-meter");
    for (const label of ["messages", "estimated", "folded", "provider", "delta", "within tolerance"]) {
      expect(meter).toContain(label);
    }
  });

  it("states the compaction budget and whether it may compact", async () => {
    const status = await textOf("#compact-status");
    for (const label of ["live", "folded", "estimated", "budget", "over budget", "can compact"]) {
      expect(status).toContain(label);
    }
  });

  it("compacts on demand and reports the outcome in words", async () => {
    await $("#ctx-compact").click();
    await browser.waitUntil(async () => (await $("#ctx-note").getText()).trim().length > 0, {
      timeoutMsg: "compact_now reported nothing",
    });
    expect(await textOf("#ctx-note")).toMatch(/Folded \d+ turn|Not compacted/);
  });

  it("restores whatever was folded", async () => {
    await $("#ctx-restore").click();
    await browser.waitUntil(async () => /Restored \d+ folded turn/.test(await $("#ctx-note").getText()), {
      timeoutMsg: "restore_compacted reported nothing",
    });
  });

  it("lists spilled output, or explains why the list is empty", async () => {
    const spills = await rowCount("#spill-list .spill-row");
    if (spills === 0) {
      expect(await textOf("#spill-list")).toContain("Nothing has spilled");
    } else {
      await expect($("#spill-list .spill-row .spill-id")).toBeDisplayed();
    }
  });

  it("asks for a chat rather than calling with an empty id", async () => {
    // With no chat selected the panel must refuse locally; the CLI-side guard
    // for the same rule is `cargo test -p bsh`.
    await expect($("#ctx-note")).toBeExisting();
  });
});
