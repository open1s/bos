/** M7 — background jobs: what is alive, its output tail, and how to stop it. */
const { $, expect } = require("@wdio/globals");
const { textOf, rowCount } = require("./helpers");

describe("M7 background jobs", () => {
  it("opens the jobs panel from the Automation nav item", async () => {
    await $("#nav-automation").click();
    await $("#jobs-list").waitForDisplayed({ timeoutMsg: "the Automation item did not reach the jobs panel" });
    await expect($("#jobs-refresh")).toBeDisplayed();
  });

  it("lists the jobs it knows about, or says plainly that there are none", async () => {
    const jobs = await rowCount("#jobs-list .job-row");
    if (jobs === 0) {
      expect(await textOf("#jobs-list")).toContain("No background jobs");
    } else {
      expect(jobs).toBeGreaterThan(0);
      await expect($("#jobs-list .job-row .job-state")).toBeDisplayed();
      await expect($("#jobs-list .job-row .job-label")).toBeDisplayed();
    }
  });

  it("reloads on demand without losing the panel", async () => {
    await $("#jobs-refresh").click();
    await $("#jobs-list").waitForDisplayed();
  });
});
