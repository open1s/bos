/** Shared steps for the BOS end-to-end specs. */
const { browser, $, $$ } = require("@wdio/globals");

const SETTINGS = "#settings-modal";

/** Open Settings and wait for it to actually be on screen. */
async function openSettings() {
  if (!(await $(SETTINGS).isDisplayed())) {
    await $("#open-settings").click();
  }
  await browser.waitUntil(async () => (await $(SETTINGS).getAttribute("class")) &&
    !(await $(SETTINGS).getAttribute("class")).includes("hidden"), {
    timeoutMsg: "Settings never became visible",
  });
}

/** Close Settings with Escape and wait for it to go away. */
async function closeSettings() {
  await browser.keys(["Escape"]);
  await browser.waitUntil(
    async () => ((await $(SETTINGS).getAttribute("class")) || "").includes("hidden"),
    { timeoutMsg: "Settings never closed" },
  );
}

/**
 * Open a named Settings section and hand back its container. The section nav
 * scrolls rather than swaps, so the check is that the panel is displayed.
 */
async function openSection(sectionId) {
  await openSettings();
  const tab = await $(`[data-section="${sectionId}"]`);
  await tab.click();
  const panel = await $(`#${sectionId}`);
  await panel.waitForDisplayed({ timeout: 10000 });
  return panel;
}

/** Start a fresh chat so session-scoped panels have something to describe. */
async function newChat() {
  await $("#new-chat").click();
  await browser.waitUntil(
    async () => (await $("#input").isDisplayed()),
    { timeoutMsg: "the composer never appeared" },
  );
}

/** Text of an element, with runs of whitespace collapsed for comparison. */
async function textOf(selector) {
  return (await $(selector).getText()).replace(/\s+/g, " ").trim();
}

/** Text of a nested element, without a second round trip to the driver. */
async function nestedText(root, selector) {
  return (await root.$(selector).getText()).replace(/\s+/g, " ").trim();
}

const rowCount = async (selector) => (await $$(selector)).length;

module.exports = {
  SETTINGS,
  openSettings,
  closeSettings,
  openSection,
  newChat,
  textOf,
  nestedText,
  rowCount,
};
