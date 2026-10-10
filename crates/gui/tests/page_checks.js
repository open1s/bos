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
    // The opener must be an element that can really take focus at this
    // viewport. The first version of this check used #sidebar-toggle, which
    // is display:none above 720px — focus() on it is a silent no-op, so the
    // recorded "opener" was BODY; the check only ever passed while a broken
    // <base> left the page unstyled and every element was focusable.
    const opener = d.getElementById("open-settings");
    opener.focus();
    assert(
      `the focus anchor really takes focus here (active=${
        d.activeElement && d.activeElement.id
      })`,
      d.activeElement === opener,
    );
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

    // D2. A dialog opened with nothing focusable on the way in must not
    //     strand the keyboard once it closes: with BODY as the recorded
    //     opener, the pre-fix probe left activeElement on set-model — a field
    //     that is display:none at that point. blur() so the opener really is
    //     body, then assert close lands on something still on screen.
    if (d.activeElement && typeof d.activeElement.blur === "function") {
      d.activeElement.blur();
    }
    await window.openSettings();
    window.closeSettings();
    const strandedOn = d.activeElement;
    assert(
      `closing with no focusable opener lands focus on screen (active=${
        strandedOn && (strandedOn.id || strandedOn.tagName)
      }, visible=${
        strandedOn && strandedOn.getClientRects
          ? strandedOn.getClientRects().length > 0
          : false
      })`,
      !!strandedOn &&
        strandedOn !== d.getElementById("set-model") &&
        typeof strandedOn.getClientRects === "function" &&
        strandedOn.getClientRects().length > 0,
    );
    // E. The ongoing goal follows the reference layout: a card above the
    //    composer, folded without a goal, state machine visible in the chips.
    const goalCard = d.getElementById("goal-card");
    const goalForm = d.getElementById("goal-form");
    const composer = d.getElementById("composer");
    assert(
      `the goal card sits above the composer (${
        goalCard && composer
          ? !!(goalCard.compareDocumentPosition(composer) & Node.DOCUMENT_POSITION_FOLLOWING)
          : "markup missing"
      })`,
      !!goalCard &&
        !!composer &&
        !!(goalCard.compareDocumentPosition(composer) &
          Node.DOCUMENT_POSITION_FOLLOWING),
    );
    // A bare `hidden` class used to hide nothing (only components with their
    // own `.x.hidden` rule ever closed); the global rule is what makes this
    // folded state real, so assert the computed display, not just the class.
    assert(
      `no goal means no card in layout (class-hidden=${goalCard.classList.contains(
        "hidden",
      )}, display=${getComputedStyle(goalCard).display})`,
      goalCard.classList.contains("hidden") &&
        getComputedStyle(goalCard).display === "none",
    );
    window.renderGoal({
      objective: "ship the goal card",
      state: "active",
      rounds: 3,
      max_rounds: 12,
      blocked_reason: null,
    });
    assert(
      `an active goal shows objective, state and rounds (chip=${d
        .getElementById("goal-state")
        .textContent.trim()}, rounds=${d
        .getElementById("goal-rounds")
        .textContent.trim()}, +1=${d.getElementById("goal-advance").disabled ? "off" : "on"})`,
      !goalCard.classList.contains("hidden") &&
        d.getElementById("goal-objective").textContent === "ship the goal card" &&
        d.getElementById("goal-state").textContent === "active" &&
        d.getElementById("goal-rounds").textContent.includes("3 of 12") &&
        !d.getElementById("goal-advance").disabled &&
        !d.getElementById("goal-pause").disabled,
    );
    window.renderGoal({
      objective: "x",
      state: "active",
      rounds: 12,
      max_rounds: 12,
      blocked_reason: null,
    });
    assert(
      `reaching the round cap disables +1 (${d
        .getElementById("goal-rounds")
        .textContent.trim()})`,
      d.getElementById("goal-advance").disabled &&
        /cap/.test(d.getElementById("goal-rounds").textContent),
    );
    window.renderGoal({
      objective: "x",
      state: "blocked",
      rounds: 4,
      max_rounds: null,
      blocked_reason: "waiting on deps",
    });
    assert(
      `a blocked goal states its reason and locks the state actions (blocked=${d
        .getElementById("goal-blocked")
        .textContent.trim()})`,
      !d.getElementById("goal-blocked").classList.contains("hidden") &&
        /waiting on deps/.test(d.getElementById("goal-blocked").textContent) &&
        d.getElementById("goal-block").disabled &&
        d.getElementById("goal-complete").disabled &&
        d.getElementById("goal-pause").disabled,
    );
    // The head chip is the entry point while the card is folded.
    window.renderGoal(null);
    d.getElementById("goal-open").click();
    const chipOpened =
      !goalCard.classList.contains("hidden") && !goalForm.classList.contains("hidden");
    d.getElementById("goal-cancel").click();
    assert(
      `the head chip opens the card with its editor and Cancel folds it away (opened=${chipOpened}, after=${goalCard.classList.contains(
        "hidden",
      )})`,
      chipOpened && goalCard.classList.contains("hidden"),
    );
    // F. The header's Chat | Trajectory strip (M10): a tablist that owns its
    //    tabs, a switch that really swaps the panes, and a trajectory built
    //    from the real message cache — roles, tool timings, running tools,
    //    errors, the rebuild guard and the empty state.
    const strip = d.getElementById("center-tabs");
    const tabChat = d.getElementById("tab-chat");
    const tabTraj = d.getElementById("tab-trajectory");
    const msgsPane = d.getElementById("messages");
    const trajPane = d.getElementById("trajectory");
    const vis = (el) => getComputedStyle(el).display !== "none";
    assert(
      `the header carries a tablist owning its two tabs (role=${strip && strip.getAttribute(
        "role",
      )}, children=${strip ? strip.querySelectorAll('[role="tab"]').length : 0})`,
      !!strip &&
        strip.getAttribute("role") === "tablist" &&
        strip.querySelectorAll('[role="tab"]').length === 2 &&
        !!tabChat &&
        !!tabTraj &&
        tabChat.parentElement === strip &&
        tabTraj.parentElement === strip,
    );
    assert(
      `each tab names its pane and the pane names it back (controls=${tabChat.getAttribute(
        "aria-controls",
      )},${tabTraj.getAttribute("aria-controls")})`,
      !!msgsPane &&
        !!trajPane &&
        d.getElementById(tabChat.getAttribute("aria-controls") || "") === msgsPane &&
        d.getElementById(tabTraj.getAttribute("aria-controls") || "") === trajPane &&
        msgsPane.getAttribute("aria-labelledby") === "tab-chat" &&
        trajPane.getAttribute("aria-labelledby") === "tab-trajectory",
    );
    assert(
      `Chat loads as the visible pane (messages=${vis(msgsPane)}, trajectory=${vis(
        trajPane,
      )}, selected=${tabChat.getAttribute("aria-selected")})`,
      !msgsPane.classList.contains("hidden") &&
        vis(msgsPane) &&
        !vis(trajPane) &&
        tabChat.getAttribute("aria-selected") === "true" &&
        tabTraj.getAttribute("aria-selected") === "false" &&
        tabChat.tabIndex === 0 &&
        tabTraj.tabIndex === -1,
    );
    // Seed the cache the transcript really reads, then drive the real tab.
    state.activeId = "page-checks-g";
    state.cache["page-checks-g"] = [
      { role: "User", text: "first turn", reasoning: null, tools: [], error: null },
      {
        role: "Assistant",
        text: "on it",
        reasoning: "think",
        tools: [
          // args is a JSON *string* in the shipped cache (the stream's own
          // shape — signature and summary both parse it), so seed that.
          { name: "read", args: JSON.stringify({ path: "a.txt" }), output: "ok", ms: 42 },
          { name: "shell", args: JSON.stringify({ cmd: "ls" }), output: null, ms: null },
        ],
        error: null,
      },
      { role: "Assistant", text: "", reasoning: null, tools: [], error: "boom" },
    ];
    window.renderMessages();
    tabTraj.click();
    const steps = trajPane.querySelectorAll(".traj-step");
    assert(
      `Trajectory lists every turn as a step (${steps.length} steps, ${trajPane.querySelectorAll(
        ".traj-tool",
      ).length} tool rows, panes swapped=${vis(trajPane) && !vis(msgsPane)})`,
      tabTraj.getAttribute("aria-selected") === "true" &&
        tabChat.getAttribute("aria-selected") === "false" &&
        tabTraj.tabIndex === 0 &&
        tabChat.tabIndex === -1 &&
        vis(trajPane) &&
        !vis(msgsPane) &&
        steps.length === 3,
    );
    const trajText = trajPane.textContent;
    assert(
      "a step states its tools with timings, and a running tool says so in words",
      /42 ms/.test(trajText) &&
        /running…/.test(trajText) &&
        /read/.test(trajText) &&
        /shell/.test(trajText),
    );
    assert(
      "roles and errors are carried in text, not color alone",
      /Assistant/.test(trajText) && /User/.test(trajText) && /⚠ boom/.test(trajText),
    );
    const firstStep = steps[0];
    window.renderMessages();
    assert(
      "an unchanged conversation does not rebuild the trajectory (same node)",
      trajPane.querySelectorAll(".traj-step")[0] === firstStep,
    );
    // Arrows move and activate; one tab stop serves the strip. Start from
    // Chat explicitly — the flow above left Trajectory selected, and a
    // direction key wraps from wherever selection actually is.
    tabChat.click();
    tabChat.focus();
    strip.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    assert(
      `ArrowRight moves the strip to Trajectory and activates it (active=${
        d.activeElement && d.activeElement.id
      })`,
      d.activeElement === tabTraj &&
        tabTraj.getAttribute("aria-selected") === "true" &&
        vis(trajPane),
    );
    strip.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    assert(
      `ArrowLeft moves it back to Chat (active=${d.activeElement && d.activeElement.id}, visible=${vis(
        msgsPane,
      )})`,
      d.activeElement === tabChat &&
        tabChat.getAttribute("aria-selected") === "true" &&
        vis(msgsPane),
    );
    // Empty state: an empty chat explains itself instead of going blank.
    state.cache["page-checks-g"] = [];
    tabTraj.click();
    const empty = trajPane.querySelector(".traj-empty");
    assert(
      `an empty chat says so in the trajectory (${empty ? empty.textContent.trim().split("\n")[0] : "no state"})`,
      !!empty && /No turns yet/.test(trajPane.textContent) && !vis(msgsPane),
    );
    tabChat.click();
    state.activeId = "";
    delete state.cache["page-checks-g"];
  } catch (err) {
    assert(`the live-page harness ran to completion (${err})`, false);
  }
  const pre = d.createElement("pre");
  pre.id = "result";
  pre.textContent = JSON.stringify(R);
  d.body.appendChild(pre);
})();
