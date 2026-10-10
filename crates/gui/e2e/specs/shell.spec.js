/** The chrome every feature sits in: sidebar, Settings, theme, language, keys. */
const { browser, $, expect } = require("@wdio/globals");
const { openSettings, closeSettings } = require("./helpers");

describe("shell chrome", () => {
  it("shows the workspace, the composer and a way to start a chat", async () => {
    await expect($("#sidebar")).toBeDisplayed();
    await expect($("#new-chat")).toBeDisplayed();
    await expect($("#input")).toBeDisplayed();
  });

  it("opens Settings from the sidebar and closes it again", async () => {
    await openSettings();
    await expect($("#settings-title")).toHaveText("Settings");
    await closeSettings();
  });

  it("offers a skip link straight into the transcript", async () => {
    await expect($("#skip-link")).toHaveAttribute("href", "#messages");
  });

  it("cycles the theme, repainting the document element", async () => {
    const appearance = () => $("html").getAttribute("data-appearance");
    const started = await appearance();
    await $("#theme-toggle").click();
    await browser.waitUntil(async () => (await appearance()) !== started, {
      timeoutMsg: "the appearance never changed",
    });
    // Return to where it began so a run leaves the profile as it found it.
    await browser.waitUntil(
      async () => {
        if ((await appearance()) === started) return true;
        await $("#theme-toggle").click();
        return false;
      },
      { timeoutMsg: "the appearance never came back" },
    );
  });

  it("translates the chrome into Chinese and back", async () => {
    const tab = $("#tab-chat");
    const before = (await tab.getText()).trim();
    await $("#lang-toggle").click();
    await browser.waitUntil(async () => (await $("html").getAttribute("lang")) === "zh", {
      timeoutMsg: "the page never switched to Chinese",
    });
    const translated = (await tab.getText()).trim();
    expect(translated).not.toEqual(before);
    await $("#lang-toggle").click();
    await browser.waitUntil(async () => (await $("html").getAttribute("lang")) === "en", {
      timeoutMsg: "the page never switched back to English",
    });
    expect((await tab.getText()).trim()).toEqual(before);
  });

  it("opens the keyboard reference with ? and closes it with Escape", async () => {
    await browser.keys(["?"]);
    const dialog = $("#shortcuts-modal");
    await dialog.waitForDisplayed({ timeoutMsg: "? did not open the keyboard reference" });
    await browser.keys(["Escape"]);
    await browser.waitUntil(
      async () => ((await dialog.getAttribute("class")) || "").includes("hidden"),
      { timeoutMsg: "Escape did not close the keyboard reference" },
    );
  });

  it("keeps a live status line in the footer", async () => {
    await expect($("#status-items")).toBeExisting();
  });
});
