/** M4 — scoped approval rules: the book, its warnings, and the audit ring. */
const { $, $$, browser, expect } = require("@wdio/globals");
const { openSection, textOf, nestedText, rowCount } = require("./helpers");

describe("M4 scoped approval rules", () => {
  it("shows the rule book with its add form", async () => {
    await openSection("rules-section");
    await expect($("#rules-list")).toBeDisplayed();
    await expect($("#rule-tool")).toBeDisplayed();
    await expect($("#rule-add")).toBeDisplayed();
  });

  it("refuses to add a rule with no tool, and says why", async () => {
    const before = await rowCount("#rules-list .rule-row");
    await $("#rule-tool").setValue("");
    await $("#rule-add").click();
    await browser.waitUntil(
      async () => /needs a tool name/.test(await $("#rules-warnings").getText()),
      { timeoutMsg: "the empty tool was not refused" },
    );
    expect(await rowCount("#rules-list .rule-row")).toEqual(before);
  });

  it("adds a rule, shows exactly what it stored, then revokes it", async () => {
    const before = await rowCount("#rules-list .rule-row");
    await $("#rule-tool").setValue("read_file");
    await $("#rule-scope").selectByAttribute("value", "global");
    await $("#rule-effect").selectByAttribute("value", "allow");
    await $("#rule-add").click();
    await browser.waitUntil(async () => (await rowCount("#rules-list .rule-row")) === before + 1, {
      timeoutMsg: "the new rule never appeared in the book",
    });
    const row = (await $$("#rules-list .rule-row"))[before];
    expect(await nestedText(row, ".rule-tool")).toEqual("read_file");
    expect(await nestedText(row, ".rule-effect")).toEqual("allow");
    expect(await nestedText(row, ".rule-scope")).toEqual("global");
    expect(await nestedText(row, ".rule-arg")).toEqual("any args");
    // Revoking must take it back out of the book — and off the disk.
    await row.$(".btn").click();
    await browser.waitUntil(async () => (await rowCount("#rules-list .rule-row")) === before, {
      timeoutMsg: "the revoked rule stayed in the book",
    });
    expect(await textOf("#rules-list")).not.toContain("read_file");
  });

  it("keeps a record of the decisions it has made", async () => {
    const decisions = await rowCount("#audit-list .audit-row");
    if (decisions === 0) {
      expect(await textOf("#audit-list")).toContain("No decisions recorded yet");
    } else {
      await expect($("#audit-list .audit-row .audit-when")).toBeDisplayed();
      await expect($("#audit-list .audit-row .rule-effect")).toBeDisplayed();
    }
  });
});
