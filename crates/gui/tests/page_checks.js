/* Checks that run *inside the shipped page* (ui/index.html + ui/app.js).
 *
 * Every other guard in this directory builds its own fixtures, so nothing that
 * needs the real markup — the dialog landmarks, a listbox owning its options,
 * focus hand-back — could be observed before this one existed. The runner
 * injects the Tauri stub ahead of app.js and appends this file at the end of
 * the body; everything else is the shipped bytes.
 *
 * Only promises that settle in a microtask are awaited, so the whole suite
 * finishes before the load event and `--dump-dom` captures the result.
 */
(async () => {
  const R = { checks: {}, failures: 0 };
  const assert = (desc, cond) => {
    R.checks[desc] = cond ? "PASS" : "FAIL";
    if (!cond) R.failures += 1;
  };
  const d = document;
  try {
    // A. The shipped shell describes its dialogs.
    const modal = d.getElementById("settings-modal");
    const mLabel = d.getElementById(modal.getAttribute("aria-labelledby") || "");
    assert(
      `settings is a labelled modal dialog (${modal.getAttribute("role")}/${
        modal.getAttribute("aria-modal")
      }/${mLabel && mLabel.textContent.trim()})`,
      modal.getAttribute("role") === "dialog" &&
        modal.getAttribute("aria-modal") === "true" &&
        !!mLabel &&
        mLabel.textContent.trim() === "Settings",
    );
    const approval = d.getElementById("approval-modal");
    const aLabel = d.getElementById(approval.getAttribute("aria-labelledby") || "");
    assert(
      `the approval dialog is labelled (${aLabel && aLabel.textContent.trim()})`,
      approval.getAttribute("role") === "dialog" &&
        approval.getAttribute("aria-modal") === "true" &&
        !!aLabel &&
        aLabel.textContent.trim() === "Approval required",
    );
    const allow = d.getElementById("approval-allow");
    assert(
      `the approval's explicit choice is focusable (${allow && allow.tagName})`,
      !!allow && allow.tagName === "BUTTON" && !allow.disabled,
    );

    // B. The real palette, driven through the real composer.
    const input = d.getElementById("input");
    const list = d.getElementById("cmd-list");
    input.value = "/";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    const opts = [...list.querySelectorAll('[role="option"]')];
    const selected = opts.filter((o) => o.getAttribute("aria-selected") === "true");
    assert(
      `the palette is a listbox owning its options (${opts.length} options, ${selected.length} selected)`,
      list.getAttribute("role") === "listbox" &&
        opts.length > 0 &&
        opts.every((o) => o.parentElement === list) &&
        selected.length === 1,
    );
    assert(
      `the composer names the active option (${input.getAttribute("aria-activedescendant")} == ${
        selected[0] && selected[0].id
      })`,
      input.getAttribute("aria-expanded") === "true" &&
        !!selected[0] &&
        selected[0].id === input.getAttribute("aria-activedescendant"),
    );
    input.value = "an ordinary message";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    assert(
      `closing the palette clears the active option (expanded=${input.getAttribute(
        "aria-expanded",
      )}, ad=${input.getAttribute("aria-activedescendant")})`,
      input.getAttribute("aria-expanded") === "false" &&
        !input.hasAttribute("aria-activedescendant"),
    );

    // C. Focus hand-back, on the real dialog, with the host answering.
    window.__TAURI__.core.invoke = () => Promise.resolve({});
    const opener = d.getElementById("sidebar-toggle");
    opener.focus();
    await window.openSettings();
    const shown = !modal.classList.contains("hidden");
    window.closeSettings();
    assert(
      `closing settings hands focus back to the opener (shown=${shown}, active=${
        d.activeElement && d.activeElement.id
      })`,
      shown && d.activeElement === opener,
    );

    // D. Escape closes it too, and focus still comes back.
    opener.focus();
    await window.openSettings();
    d.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    assert(
      `Escape closes settings and hands focus back (hidden=${modal.classList.contains(
        "hidden",
      )}, active=${d.activeElement && d.activeElement.id})`,
      modal.classList.contains("hidden") && d.activeElement === opener,
    );
  } catch (err) {
    assert(`the live-page harness ran to completion (${err})`, false);
  }
  const pre = d.createElement("pre");
  pre.id = "result";
  pre.textContent = JSON.stringify(R);
  d.body.appendChild(pre);
})();
