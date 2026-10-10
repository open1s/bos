/** M11 self-inspection, M10 deterministic replay, M14 bundle export. */
const { $, browser, expect } = require("@wdio/globals");
const { openSection, newChat, textOf, rowCount } = require("./helpers");

describe("M11 self-inspection and the run report", () => {
  before(async () => {
    await newChat();
  });

  it("reports the live inventory the agent would actually run with", async () => {
    await openSection("insight-section");
    const grid = await textOf("#insight-grid");
    for (const label of [
      "tools",
      "async tools",
      "skills",
      "plugins",
      "hook events",
      "MCP servers",
      "memories",
      "model",
      "workspace",
      "bash",
    ]) {
      expect(grid).toContain(label);
    }
    const tools = grid.match(/tools(\d+)/);
    expect(tools).not.toBeNull();
    expect(Number(tools[1])).toBeGreaterThan(0);
  });

  it("evaluates the transcript and shows a grade", async () => {
    await $("#insight-evaluate").click();
    const box = $("#insight-score");
    await box.waitForDisplayed({ timeoutMsg: "evaluate_transcript showed no score" });
    expect(await textOf("#insight-score")).toMatch(/[A-F][+-]? · -?\d+/);
  });

  it("replays the run and reports its metrics", async () => {
    await $("#replay-run").click();
    const box = $("#replay-report");
    await box.waitForDisplayed({ timeoutMsg: "replay_session showed no report" });
    const report = await textOf("#replay-report");
    expect(report).toContain("Replay of");
    for (const label of ["turns", "tool calls", "tool time", "estimated"]) {
      expect(report).toContain(label);
    }
  });

  it("exports a bundle and names the archive it wrote", async () => {
    await $("#bundle-export").click();
    await browser.waitUntil(async () => /\.zip/.test(await $("#bundle-result").getText()), {
      timeoutMsg: "export_bundle never reported a path",
    });
    const note = await textOf("#bundle-result");
    expect(note).toMatch(/Wrote .+\.zip/);
    expect(note).toMatch(/schema v\d+/);
    expect(await rowCount(".cap-note.err")).toEqual(0);
  });
});
