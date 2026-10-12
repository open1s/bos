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

    // ---- M13 polish: language, shortcut reference, theme, a11y -----------
    // These read the *shipped* page, so they prove the marks and controls are
    // wired to the shell's own preferences rather than to a private copy.
    const polish = window.Polish;
    assert(
      `the polish module carries a shortcut table (${polish && polish.SHORTCUTS.length} entries)`,
      !!polish &&
        Array.isArray(polish.SHORTCUTS) &&
        polish.SHORTCUTS.length >= 12 &&
        polish.SHORTCUTS.every((s) => s.keys && s.action && s.group),
    );

    const tabLabel = () => d.getElementById("tab-chat").textContent.trim();
    const shippedTab = tabLabel();
    // A mark in the sidebar's own Workspaces header: the shipped chrome, not a
    // fixture copy, and one that survives the sidebar being the conversation
    // list plus workspaces and nothing else.
    const navSpan = d.querySelector("#side-workspaces [data-i18n]");
    if (polish) polish.setLang("zh");
    const zhTab = tabLabel();
    const zhNav = navSpan ? navSpan.textContent.trim() : "";
    assert(
      `choosing 中文 translates the chrome (${shippedTab}→${zhTab}, nav=${zhNav})`,
      zhTab === "对话" &&
        zhNav === "工作区" &&
        /发送消息/.test(d.getElementById("input").placeholder) &&
        /^zh/i.test(d.documentElement.lang),
    );
    // The claim is "one source of truth", so assert the choice reached the
    // shell's own state and its persisted record — a module that only painted
    // the DOM would pass a text-only check and still lose the setting.
    const shell = typeof Shell !== "undefined" ? Shell : null;
    const persisted = (() => {
      try {
        return d.defaultView.localStorage.getItem("bos.ui.v1") || "";
      } catch (err) {
        return "";
      }
    })();
    assert(
      `中文 is owned by the shell and persisted (state=${shell && shell.state.language}, stored=${
        /"language":"zh"/.test(persisted)
      })`,
      !!shell && shell.state.language === "zh" && /"language":"zh"/.test(persisted),
    );
    if (polish) polish.setLang("en");
    assert(
      `switching back restores the shipped strings (${tabLabel()})`,
      tabLabel() === shippedTab &&
        /Send a message/.test(d.getElementById("input").placeholder) &&
        /^en/i.test(d.documentElement.lang) &&
        !!shell &&
        shell.state.language === "en",
    );

    const shortcutOpener = d.getElementById("open-settings");
    if (shortcutOpener) shortcutOpener.focus();
    if (polish) polish.openShortcuts();
    const smodal = d.getElementById("shortcuts-modal");
    const srows = smodal ? smodal.querySelectorAll(".shortcut-row") : [];
    const sTitle = smodal ? d.getElementById(smodal.getAttribute("aria-labelledby") || "") : null;
    assert(
      `? opens a labelled shortcut reference (${srows.length} rows, title=${
        sTitle && sTitle.textContent.trim()
      })`,
      !!smodal &&
        smodal.getAttribute("role") === "dialog" &&
        smodal.getAttribute("aria-modal") === "true" &&
        !smodal.classList.contains("hidden") &&
        srows.length >= 12 &&
        !!sTitle &&
        sTitle.textContent.trim().length > 0,
    );
    const sclosed = polish ? polish.closeShortcuts() : false;
    assert(
      `closing the reference hands the keyboard back (active=${
        d.activeElement && (d.activeElement.id || d.activeElement.tagName)
      })`,
      sclosed === true && !!smodal && smodal.classList.contains("hidden") && d.activeElement === shortcutOpener,
    );

    const themeBtn = d.getElementById("theme-toggle");
    const modeBefore = d.documentElement.dataset.appearance;
    if (themeBtn) themeBtn.click();
    const modeAfter = d.documentElement.dataset.appearance;
    assert(
      `the theme control cycles the shell's appearance (${modeBefore}→${modeAfter}, state=${
        shell && shell.state.appearance
      })`,
      !!themeBtn &&
        !!modeAfter &&
        modeAfter !== modeBefore &&
        !!shell &&
        shell.state.appearance === modeAfter,
    );
    assert(
      "dark and light resolve to different backgrounds",
      (() => {
        if (!polish) return false;
        polish.setTheme("dark");
        const dark = getComputedStyle(d.body).backgroundColor;
        polish.setTheme("light");
        const light = getComputedStyle(d.body).backgroundColor;
        return !!dark && !!light && dark !== light;
      })(),
    );
    if (polish) polish.setTheme("system");

    const skip = d.getElementById("skip-link");
    const live = d.getElementById("stream-chip");
    assert(
      `the page offers a skip link and a live stream state (href=${
        skip && skip.getAttribute("href")
      }, live=${live && live.getAttribute("aria-live")})`,
      !!skip &&
        skip.getAttribute("href") === "#messages" &&
        !!live &&
        live.getAttribute("aria-live") === "polite",
    );

    // The binding, not just the API: a real `?` keydown opens it, Escape closes
    // it, and typing `?` into the composer must not be hijacked.
    const pressKey = (key) => d.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
    const composerInput = d.getElementById("input");
    composerInput.focus();
    composerInput.dispatchEvent(new KeyboardEvent("keydown", { key: "?", bubbles: true }));
    const typedInField = !!smodal && !smodal.classList.contains("hidden");
    composerInput.blur();
    pressKey("?");
    const afterQuestion = !!smodal && !smodal.classList.contains("hidden");
    pressKey("Escape");
    const afterEscape = !!smodal && !smodal.classList.contains("hidden");
    assert(
      `? opens the reference and Escape closes it (inField=${typedInField}, open=${afterQuestion}, afterEsc=${afterEscape})`,
      typedInField === false && afterQuestion === true && afterEscape === false,
    );

    // Assistive tech can discover the two keys that are not clickable: the
    // composer's send keys and the sidebar's Mod+B.
    const railKey = d
      .getElementById("sidebar-toggle")
      .getAttribute("aria-keyshortcuts");
    const inputKey = composerInput.getAttribute("aria-keyshortcuts");
    assert(
      `the non-clickable keys are announced (rail=${railKey}, input=${inputKey})`,
      /Meta\+B/.test(railKey || "") && /Enter/.test(inputKey || ""),
    );

    // The shipped footer accepts an item from any surface, and removing it
    // leaves the footer as it was.
    const shippedBar = d.getElementById("status-right");
    const added =
      typeof Shell !== "undefined" &&
      typeof Shell.addStatusItem === "function" &&
      Shell.addStatusItem("page-check", { label: "checks", value: 36 });
    const rendered = d.querySelector('#status-items [data-status-item="page-check"]');
    const registryText = rendered
      ? `${rendered.querySelector(".status-item-label").textContent}=${
          rendered.querySelector(".status-item-value").textContent
        }`
      : "";
    const dropped = typeof Shell !== "undefined" && Shell.removeStatusItem("page-check");
    assert(
      `the shipped footer takes a registered item (${registryText}, removed=${dropped})`,
      !!shippedBar &&
        added === true &&
        registryText === "checks=36" &&
        dropped === true &&
        !d.querySelector('#status-items [data-status-item="page-check"]'),
    );

    // An id must be unique, and this one actually shipped twice: the static
    // drawer button and the shell's injected rail button both claimed
    // id="sidebar-toggle", so app.js's drawer handler bound to the shell's copy
    // while the static one sat dead — and equally visible under 720 px, where
    // the id selector shows every match.
    assert(
      `the header owns exactly one sidebar toggle (${d.querySelectorAll("#sidebar-toggle").length})`,
      d.querySelectorAll("#sidebar-toggle").length === 1,
    );

    // `?` must not stack the reference on top of an already-open dialog.
    const settingsModal = d.getElementById("settings-modal");
    settingsModal.classList.remove("hidden");
    pressKey("?");
    const stacked = !!smodal && !smodal.classList.contains("hidden");
    settingsModal.classList.add("hidden");
    assert(
      `? does not stack the reference over an open dialog (opened=${stacked})`,
      stacked === false,
    );

    // ---- E2E: drive every run surface through a recording IPC double -------
    // The shipped page is exercised for real: each panel's own refresh is
    // awaited after the bridge is replaced with a recording double, so these
    // checks prove the *whole* path — handler → invoke(name, args) → render —
    // and assert both the arguments that crossed the bridge and the elements
    // that appeared. `flush` yields only microtasks, which is all this harness
    // may rely on (it has to finish before the load event).
    const flush = async () => {
      for (let i = 0; i < 16; i += 1) await Promise.resolve();
    };
    const calls = [];
    const fixtures = {
      jobs_list: [
        { id: "j1", label: "cargo test", kind: "shell", state: "running", started_at: 100, finished_at: null, bytes: 2048, dropped_bytes: 0, error: null },
        { id: "j2", label: "lint", kind: "run", state: "failed", started_at: 100, finished_at: 105, bytes: 512, dropped_bytes: 0, error: "exit 1" },
      ],
      job_output: { id: "j1", state: "running", text: "compiling…\ndone", bytes: 16, total_bytes: 16, truncated: false, dropped_bytes: 0 },
      job_interrupt: { id: "j1", label: "cargo test", kind: "shell", state: "interrupted", started_at: 100, finished_at: 106, bytes: 2048, dropped_bytes: 0, error: null },
      token_meter: { session_id: "e2e", messages: 4, estimated: 1234, archived_estimated: 56, provider: 1300, delta: 66, within_tolerance: true, per_message: [] },
      compaction_status: { session_id: "e2e", title: "t", messages: 4, live_messages: 3, archived_messages: 1, estimated_tokens: 1234, budget_tokens: 8192, over_budget: false, can_compact: true, compacting: false, refusal: null },
      spill_list: [{ id: "sp1", file: "/tmp/spill/sp1.txt", bytes: 40000, preview: "lots of output" }],
      spill_read: { id: "sp1", file: "/tmp/spill/sp1.txt", bytes: 40000, truncated: true, text: "head of the output" },
      approval_rules_list: { rules: [{ id: "r1", tool: "bash", arg: { key: "command", match_kind: "prefix", value: "git " }, scope: "global", effect: "allow" }], warnings: ["line 3: bad entry"], audit_cap: 200 },
      approval_audit: [{ at: 1791631013, tool: "bash", args: "git status", decision: "ask", rule_id: null }],
      approval_rule_revoke: { rules: [], warnings: [], audit_cap: 200 },
      list_memories: [{ id: "m1", content: "the page harness remembers this", created_at_ms: 1791631000000 }],
      memory_status: { enabled: true, path: "/home/e2e/.bos/gui/memory.jsonl", shared_workspaces: 2, cap: 500, max_chars: 2000 },
      mcp_status: {
        connecting: false,
        servers: [
          {
            name: "filesystem", transport: "stdio", enabled: true, connected: true,
            tools: 2, tool_names: ["read", "write", "list"], error: null,
          },
        ],
      },
      mcp_policy_status: {
        now_ms: 1,
        default: "allow",
        rules: [{ server: "filesystem", tool: "write", effect: "deny" }],
        max_rules: 200,
        max_name_chars: 64,
        servers: [
          {
            name: "filesystem",
            attached: 2,
            denied: 1,
            tools: [
              { name: "read", effect: "allow", by: "default" },
              { name: "write", effect: "deny", by: "filesystem/write" },
              { name: "list", effect: "allow", by: "default" },
            ],
          },
        ],
        attached: 2,
        denied: 1,
        asked: 0,
        file_version: 1,
        expected_version: 1,
      },
      mcp_policy_set: {
        now_ms: 2,
        default: "allow",
        rules: [{ server: "filesystem", tool: "write", effect: "ask" }],
        max_rules: 200,
        max_name_chars: 64,
        servers: [
          {
            name: "filesystem",
            attached: 3,
            denied: 0,
            tools: [
              { name: "read", effect: "allow", by: "default" },
              { name: "write", effect: "ask", by: "filesystem/write" },
              { name: "list", effect: "allow", by: "default" },
            ],
          },
        ],
        attached: 3,
        denied: 0,
        asked: 1,
        file_version: 1,
        expected_version: 1,
      },
      mcp_policy_clear: {
        now_ms: 3,
        default: "allow",
        rules: [],
        max_rules: 200,
        max_name_chars: 64,
        servers: [
          {
            name: "filesystem",
            attached: 3,
            denied: 0,
            tools: [
              { name: "read", effect: "allow", by: "default" },
              { name: "write", effect: "allow", by: "default" },
              { name: "list", effect: "allow", by: "default" },
            ],
          },
        ],
        attached: 3,
        denied: 0,
        asked: 0,
        file_version: 1,
        expected_version: 1,
      },
      mcp_policy_default: {
        now_ms: 4,
        default: "deny",
        rules: [],
        max_rules: 200,
        max_name_chars: 64,
        servers: [
          {
            name: "filesystem",
            attached: 0,
            denied: 3,
            tools: [
              { name: "read", effect: "deny", by: "default" },
              { name: "write", effect: "deny", by: "default" },
              { name: "list", effect: "deny", by: "default" },
            ],
          },
        ],
        attached: 0,
        denied: 3,
        asked: 0,
        file_version: 1,
        expected_version: 1,
      },
      list_capabilities: {
        tools: [],
        async_tools: [],
        skills: [{ name: "pr-review", description: "Review a pull request" }],
        plugins: [],
        hook_events: ["pre_tool_use"],
      },
      mcp_catalog: {
        now_ms: Date.now(),
        servers: [
          {
            server: "filesystem", connected: true,
            resources_supported: true, prompts_supported: true,
            resources: [
              {
                uri: "file:///repo/schema.json", name: "schema",
                description: "the repo's schema", mime_type: "application/json", clipped: false,
              },
              {
                uri: "file:///repo/runbook.md", name: "runbook",
                description: "on-call notes", mime_type: "text/markdown", clipped: false,
              },
            ],
            prompts: [
              { name: "review", description: "Review a diff", arguments: 2, required: 1, clipped: false },
            ],
            resources_total: 2, prompts_total: 1, error: null,
          },
          {
            server: "gone", connected: false,
            resources_supported: false, prompts_supported: false,
            resources: [], prompts: [], resources_total: 0, prompts_total: 0, error: null,
          },
          {
            server: "broken", connected: true,
            resources_supported: true, prompts_supported: false,
            resources: [], prompts: [], resources_total: 0, prompts_total: 0,
            error: "resources/list: connection reset",
          },
          {
            server: "second", connected: true,
            resources_supported: true, prompts_supported: false,
            resources: [
              {
                uri: "file:///notes/todo.md", name: "todo",
                description: "what is left", mime_type: "text/markdown", clipped: false,
              },
            ],
            prompts: [], resources_total: 1, prompts_total: 0, error: null,
          },
        ],
        max_rows: 200, max_text_chars: 20000, max_desc_chars: 240,
      },
      mcp_read_resource: {
        now_ms: Date.now(),
        server: "filesystem", uri: "file:///repo/schema.json",
        mime_type: "application/json", text: '{"type":"object"}',
        chars: 17, source_chars: 17, truncated: false, parts: 1, empty: false,
      },
      skill_list: {
        now_ms: Date.now(),
        dir: "/home/e2e/.bos/skills",
        exists: true,
        skills: [
          {
            name: "code-review", description: "Review a diff",
            path: "/home/e2e/.bos/skills/code-review/SKILL.md",
            instructions_chars: 34, denied: false,
          },
          {
            name: "release-notes", description: "Draft release notes",
            path: "/home/e2e/.bos/skills/release-notes/SKILL.md",
            instructions_chars: 12, denied: true,
          },
        ],
        max_body_chars: 12000,
        max_name_chars: 128,
        max_description_chars: 1024,
      },
      choose_folder: () => {
        if (pickerFails.value) throw new Error("no folder dialog here: osascript is not installed");
        return pickerChoice.value;
      },
      list_folder: (args) => ({
        path: args.path || "/home/e2e",
        parent: "/home",
        entries:
          args.path === "/home/e2e/Documents"
            ? []
            : [
                { name: "bos", path: "/home/e2e/bos" },
                { name: "Documents", path: "/home/e2e/Documents" },
              ],
        truncated: 0,
        unreadable: 0,
      }),
      remove_workspace: {
        workspaces: [],
        active_workspace: "",
        presets: [],
        model: "glm-4.7",
        context_budget: 8192,
      },
      create_workspace: {
        workspaces: [
          { id: "w1", name: "bos", root: "/tmp/bos", model: "" },
          { id: "w2", name: "trino", root: "/tmp/trino", model: "" },
          { id: "w3", name: "Documents", root: "/home/e2e/Documents", model: "glm-4.7" },
        ],
        active_workspace: "w3",
        presets: [],
        model: "glm-4.7",
        context_budget: 8192,
      },
      list_sessions: [
        { id: "s1", title: "pinned chat", updated_at: 300, workspace: "w1", pinned: true, archived: false },
        { id: "s2", title: "plain chat", updated_at: 200, workspace: "w1", pinned: false, archived: false },
        { id: "s3", title: "archived chat", updated_at: 100, workspace: "w1", pinned: false, archived: true },
        { id: "s4", title: "elsewhere", updated_at: 150, workspace: "w2", pinned: false, archived: false },
      ],
      rename_session: { id: "s2", title: "renamed here", updated_at: 200, workspace: "w1", pinned: false, archived: false },
      set_session_pinned: { id: "s1", title: "pinned chat", updated_at: 300, workspace: "w1", pinned: false, archived: false },
      set_session_archived: { id: "s2", title: "plain chat", updated_at: 200, workspace: "w1", pinned: false, archived: true },
      create_session: { id: "s9", title: "New chat", updated_at: 400, workspace: "w1", pinned: false, archived: false },
      // The composer's first message in an empty workspace reaches this one, so it
      // needs an answer: a runaway stream with no summary is what a settled fixture
      // cannot fake by omission (it crashed the harness instead).
      send_message: { session_id: "s9", turn: 1, started_ms: Date.now(), text: "" },
      set_active_workspace: { workspaces: [{ id: "w1", name: "bos", root: "/tmp/bos", model: "" }], active_workspace: "w1", presets: [], model: "m", context_budget: 8192 },
      skill_activation: {
        now_ms: Date.now(),
        session: "e2e",
        explicit: false,
        active: ["code-review"],
        available: ["code-review"],
        denied: ["release-notes"],
        path: "/home/e2e/.bos/gui/skill-activation.json",
        sessions_known: 0,
        max_sessions: 500,
      },
      skill_set_active: {
        now_ms: Date.now(),
        session: "e2e",
        explicit: true,
        active: [],
        available: ["code-review"],
        denied: ["release-notes"],
        path: "/home/e2e/.bos/gui/skill-activation.json",
        sessions_known: 1,
        max_sessions: 500,
      },
      skill_inherit: {
        now_ms: Date.now(),
        session: "e2e",
        explicit: false,
        active: ["code-review"],
        available: ["code-review"],
        denied: ["release-notes"],
        path: "/home/e2e/.bos/gui/skill-activation.json",
        sessions_known: 0,
        max_sessions: 500,
      },
      skill_author: {
        now_ms: Date.now(),
        dir: "/home/e2e/.bos/skills",
        exists: true,
        skills: [
          {
            name: "code-review", description: "Review a diff",
            path: "/home/e2e/.bos/skills/code-review/SKILL.md",
            instructions_chars: 34, denied: false,
          },
          {
            name: "pr-review", description: "Review a pull request",
            path: "/home/e2e/.bos/skills/pr-review/SKILL.md",
            instructions_chars: 41, denied: false,
          },
        ],
        max_body_chars: 12000,
        max_name_chars: 128,
        max_description_chars: 1024,
      },
      knowledge_list: {
        now_ms: Date.now(),
        query: "",
        total: 2,
        matches: [
          {
            id: "kb-1", title: "Deploy ritual", tags: ["ops", "release"],
            body: "Tag the commit\nThen push main and watch the workflow.",
            created_ms: Date.now() - 86400000, updated_ms: Date.now() - 5 * 60000,
          },
          {
            id: "kb-2", title: "House style", tags: [],
            body: "Two-space indent, no semicolons.",
            created_ms: Date.now() - 3600000, updated_ms: Date.now() - 3600000,
          },
        ],
        caps: { max_entries: 500, max_title_chars: 120, max_body_chars: 4000, max_tags: 8, max_tag_chars: 32, tool_body_chars: 1200, tool_max_limit: 20 },
      },
      knowledge_search: {
        now_ms: Date.now(), query: "deploy", total: 2,
        matches: [{
          id: "kb-1", title: "Deploy ritual", tags: ["ops", "release"],
          body: "Tag the commit\nThen push main and watch the workflow.",
          created_ms: Date.now() - 86400000, updated_ms: Date.now() - 5 * 60000,
        }],
        caps: { max_entries: 500, max_title_chars: 120, max_body_chars: 4000, max_tags: 8, max_tag_chars: 32, tool_body_chars: 1200, tool_max_limit: 20 },
      },
      knowledge_add: {
        now_ms: Date.now(), query: "", total: 3,
        matches: [{
          id: "kb-3", title: "Rebase policy", tags: ["git"], body: "Never rewrite main.",
          created_ms: Date.now(), updated_ms: Date.now(),
        }],
        caps: { max_entries: 500, max_title_chars: 120, max_body_chars: 4000, max_tags: 8, max_tag_chars: 32, tool_body_chars: 1200, tool_max_limit: 20 },
      },
      knowledge_update: {
        now_ms: Date.now(), query: "", total: 1,
        matches: [{
          id: "kb-1", title: "Deploy ritual, edited", tags: ["ops"], body: "Copy the release notes first.",
          created_ms: Date.now() - 86400000, updated_ms: Date.now(),
        }],
        caps: { max_entries: 500, max_title_chars: 120, max_body_chars: 4000, max_tags: 8, max_tag_chars: 32, tool_body_chars: 1200, tool_max_limit: 20 },
      },
      knowledge_delete: {
        now_ms: Date.now(), query: "", total: 1,
        matches: [{
          id: "kb-2", title: "House style", tags: [], body: "Two-space indent, no semicolons.",
          created_ms: Date.now() - 3600000, updated_ms: Date.now() - 3600000,
        }],
        caps: { max_entries: 500, max_title_chars: 120, max_body_chars: 4000, max_tags: 8, max_tag_chars: 32, tool_body_chars: 1200, tool_max_limit: 20 },
      },
      schedule_list: {
        now_ms: 1791631000000,
        schedules: [{
          id: "s1", text: "stand up and stretch",
          cadence: { kind: "every", every_minutes: 5, next_ms: 1791631300000 },
          created_ms: 1791631000000, last_fired_ms: null, fired: 0,
        }],
        history: [{ schedule_id: "s0", text: "drink water", at_ms: 1791630900000, noticed_ms: 1791630900000 }],
        fired: [],
        caps: { min_every_minutes: 1, max_schedules: 50, max_history: 200, max_text_chars: 500 },
      },
      schedule_tick: {
        now_ms: 1791631300000,
        schedules: [{
          id: "s1", text: "stand up and stretch",
          cadence: { kind: "every", every_minutes: 5, next_ms: 1791631600000 },
          created_ms: 1791631000000, last_fired_ms: 1791631300000, fired: 1,
        }],
        history: [
          { schedule_id: "s1", text: "stand up and stretch", at_ms: 1791631300000, noticed_ms: 1791631300000 },
          { schedule_id: "s0", text: "drink water", at_ms: 1791630900000, noticed_ms: 1791630900000 },
        ],
        fired: [{ schedule_id: "s1", text: "stand up and stretch", at_ms: 1791631300000, noticed_ms: 1791631300000 }],
        caps: { min_every_minutes: 1, max_schedules: 50, max_history: 200, max_text_chars: 500 },
      },
      self_inspect: { tools: 9, async_tools: 3, skills: 2, plugins: 1, hook_events: 4, mcp_servers: 1, mcp_tools: 2, memories: 6, approval_rules: 1, active_goal: "ship it", goal_state: "active", goal_rounds: 7, goal_remaining: 20, model: "m", workspace: "/w", bash_enabled: true, file_tools_enabled: true, skills_dir: "~/.bos/skills" },
      evaluate_transcript: { session_id: "e2e", score: 88, grade: "B+", reasons: ["2 blank assistant turns"], signals: { turns: 4 } },
      replay_session: { session_id: "e2e", title: "t", workspace: "/w", entries: [{ order: 1, index: 0, role: "user", text: "hi", reasoning: "", tools: [{ name: "bash", args: "{}", failed: false, ms: 1200 }], error: null, archived: false }], metrics: { turns: 4, user_turns: 2, assistant_turns: 2, archived_turns: 0, tool_calls: 1, tool_failures: 0, tool_ms_total: 1200, tool_ms_missing: 0, errors: 0, blank_turns: 0, estimated_tokens: 1234 } },
      export_bundle: { path: "/tmp/bos-bundle.zip", bytes: 4096, schema_version: 1, entries: 5, files: [{ name: "transcript.json", size: 100, crc32: "ab" }] },
      compact_now: { session_id: "e2e", title: "t", messages: 4, live_messages: 0, archived_messages: 4, estimated_tokens: 10, budget_tokens: 8192, over_budget: false, can_compact: false, compacting: false, refusal: null },
      restore_compacted: 4,
    };
    // The system panel is a modal and no guard can drive one, so what is checked
    // here is what this app decides around it: the picked path lands, a dismissal
    // changes nothing, and an absent panel reveals the fallback with the reason.
    const pickerChoice = { value: "/home/e2e/Documents" };
    const pickerFails = { value: false };
    d.defaultView.__TAURI__.core.invoke = (cmd, args) => {
      calls.push({ cmd, args: args || {} });
      const answer = fixtures[cmd];
      // A fixture may be a function: the settled guards answer one call at a
      // time, and a command whose *answer depends on its argument* (a folder
      // listing) cannot be checked with a single frozen value.
      return Promise.resolve(
        typeof answer === "function" ? answer(args || {}) : Object.prototype.hasOwnProperty.call(fixtures, cmd) ? answer : null,
      );
    };
    const called = (cmd) => calls.filter((c) => c.cmd === cmd);
    const lastArgs = (cmd) => {
      const hits = called(cmd);
      return hits.length ? hits[hits.length - 1].args : null;
    };
    // The Memory pane. `openSettings` loads it, so a pane nothing looks at here
    // is a pane whose wiring can rot while this guard stays green.
    await window.openSettings();
    await flush();
    const memPane = d.getElementById("mem-items");
    assert(
      `E2E: the memory pane lists what is remembered and names the file (${
        memPane.textContent.replace(/\s+/g, " ").slice(0, 92)})`,
      /the page harness remembers this/.test(memPane.textContent) &&
        /home\/e2e\/\.bos\/gui\/memory\.jsonl/.test(memPane.textContent) &&
        /shared by 2 workspaces/.test(memPane.textContent) &&
        called("list_memories").length > 0 &&
        called("memory_status").length > 0,
    );

    // The Schedule pane: a durable reminder, what it has delivered, and the
    // bounds it enforces. The tick is driven by hand here because the page
    // harness never reaches the 30 s interval.
    // The pane is filled when Settings opens, whether or not it is the tab on
    // screen — and a tick while it is *closed* must not repaint it, which is
    // what makes a reminder cost nothing per interval until someone looks.
    const schedPane = d.getElementById("sched-items");
    const schedHistory = d.getElementById("sched-history");
    const schedNote = d.getElementById("sched-note");
    const beforeTick = `${schedPane.innerHTML}|${schedHistory.innerHTML}|${schedNote.innerHTML}`;
    assert(
      `E2E: the schedule pane lists a reminder, its next due and its bounds (${
        schedPane.textContent.replace(/\s+/g, " ").slice(0, 80)})`,
      window.schedulePaneOpen() === false &&
        /stand up and stretch/.test(schedPane.textContent) &&
        /every 5 min/.test(schedPane.textContent) &&
        /in 5 min/.test(schedPane.textContent) &&
        /drink water/.test(schedHistory.textContent) &&
        /1 of 50 reminders/.test(schedNote.textContent) &&
        /200 deliveries kept/.test(schedNote.textContent),
    );
    await window.tickSchedule();
    await flush();
    const toasts = [...d.querySelectorAll(".toast")].map((t) => t.textContent);
    assert(
      `E2E: a tick announces what it delivered without repainting a closed pane (${
        toasts.join(" | ")})`,
      toasts.includes("Reminder: stand up and stretch") &&
        `${schedPane.innerHTML}|${schedHistory.innerHTML}|${schedNote.innerHTML}` ===
          beforeTick &&
        called("schedule_tick").length > 0 &&
        typeof lastArgs("schedule_tick").nowMs === "number",
    );

    // Watching the list is what makes the next tick redraw it.
    d.getElementById("set-tab-schedule-section").click();
    await flush();
    await window.tickSchedule();
    await flush();
    assert(
      `E2E: an open pane catches up with what was delivered (${
        schedPane.textContent.replace(/\s+/g, " ").slice(0, 72)})`,
      window.schedulePaneOpen() === true &&
        /delivered 1×/.test(schedPane.textContent) &&
        /stand up and stretch/.test(schedHistory.textContent) &&
        /2 of 200 deliveries kept/.test(schedNote.textContent),
    );

    // The Knowledge pane: the notes it lists, and each control reaching the
    // command it names with the values the form actually holds.
    d.getElementById("set-tab-knowledge-section").click();
    await flush();
    const kbHost = d.getElementById("kb-items");
    const kbText = () => kbHost.textContent.replace(/\s+/g, " ");
    assert(
      `E2E: the knowledge pane lists notes with tags, age and a body preview (${
        kbText().slice(0, 84)})`,
      /Deploy ritual/.test(kbText()) &&
        /ops · release/.test(kbText()) &&
        /edited 5 min ago/.test(kbText()) &&
        /Tag the commit Then push main and watch the workflow\./.test(kbText()) &&
        /2 of 500 entries/.test(d.getElementById("kb-note").textContent) &&
        /bodies 4000/.test(d.getElementById("kb-note").textContent) &&
        d.querySelectorAll("#kb-items .kb-row").length === 2,
    );

    d.getElementById("kb-query").value = "deploy";
    d.getElementById("kb-search").click();
    await flush();
    assert(
      `E2E: searching asks the backend for matches and draws only them (${
        called("knowledge_search").length} call, ${kbText().slice(0, 40)})`,
      called("knowledge_search").length === 1 &&
        lastArgs("knowledge_search").query === "deploy" &&
        typeof lastArgs("knowledge_search").limit === "number" &&
        typeof lastArgs("knowledge_search").nowMs === "number" &&
        d.querySelectorAll("#kb-items .kb-row").length === 1 &&
        !/House style/.test(kbText()),
    );
    d.getElementById("kb-all").click();
    await flush();
    assert("clearing the search asks for the whole list again", called("knowledge_list").length >= 2);

    d.getElementById("kb-title").value = "Rebase policy";
    d.getElementById("kb-tags").value = "git";
    d.getElementById("kb-body").value = "Never rewrite main.";
    d.getElementById("kb-save").click();
    await flush();
    assert(
      "adding sends what the form holds, trimmed and unmangled",
      called("knowledge_add").length === 1 &&
        lastArgs("knowledge_add").title === "Rebase policy" &&
        lastArgs("knowledge_add").tags === "git" &&
        lastArgs("knowledge_add").body === "Never rewrite main." &&
        typeof lastArgs("knowledge_add").nowMs === "number" &&
        d.getElementById("kb-title").value === "",
    );

    d.querySelector("#kb-items .kb-row .kb-edit").click();
    await flush();
    assert(
      `editing fills the form and turns Add into Save (${
        d.getElementById("kb-save").textContent})`,
      d.getElementById("kb-save").textContent === "Save" &&
        d.getElementById("kb-title").value === "Rebase policy" &&
        d.getElementById("kb-tags").value === "git" &&
        d.getElementById("kb-body").value === "Never rewrite main." &&
        d.getElementById("kb-cancel").hidden === false &&
        state.kbEditing === "kb-3",
    );
    d.getElementById("kb-title").value = "Deploy ritual, edited";
    d.getElementById("kb-save").click();
    await flush();
    assert(
      "saving an edit sends the id it is editing, and the form is cleared",
      called("knowledge_update").length === 1 &&
        lastArgs("knowledge_update").id === "kb-3" &&
        lastArgs("knowledge_update").title === "Deploy ritual, edited" &&
        state.kbEditing === null &&
        d.getElementById("kb-save").textContent === "Add" &&
        d.getElementById("kb-cancel").hidden === true,
    );

    d.querySelector("#kb-items .kb-row .kb-del").click();
    await flush();
    assert(
      "deleting sends the row's own id",
      called("knowledge_delete").length === 1 &&
        lastArgs("knowledge_delete").id === "kb-1" &&
        typeof lastArgs("knowledge_delete").nowMs === "number",
    );

    // The MCP per-tool policy: what it lists, and each control reaching the
    // command it names with the pair the row is about.
    d.getElementById("set-tab-set-mcp").click();
    await flush();
    const polText = () => d.getElementById("pol-servers").textContent.replace(/\s+/g, " ");
    const polRuleText = () => d.getElementById("pol-rules").textContent.replace(/\s+/g, " ");
    const polNote = () => d.getElementById("pol-msg").textContent;
    assert(
      `E2E: the policy pane lists each tool with the rule that decided it (${
        polText().slice(0, 78)})`,
      /write/.test(polText()) &&
        /by rule filesystem\/write/.test(polText()) &&
        // Two unruled tools say where their answer comes from, twice.
        (polText().match(/follows the default/g) || []).length === 2 &&
        /2 of 3 tools · 1 denied/.test(polText()) &&
        /3 of 3 tools attached/.test(polNote()) === false &&
        /2 of 3 tools attached · 1 denied · 0 ask · 1 of 200 rules · default allow/.test(polNote()) &&
        /filesystem\/write/.test(polRuleText()) &&
        /Deny/.test(polRuleText()) &&
        d.getElementById("pol-default").value === "allow",
    );

    const writeRow = [...d.querySelectorAll("#pol-servers .pol-row")].find((r) =>
      /write/.test(r.textContent),
    );
    writeRow.querySelector('[aria-label="Ask filesystem/write"]').click();
    await flush();
    assert(
      "asking about one tool sends that pair and the chosen effect",
      called("mcp_policy_set").length === 1 &&
        lastArgs("mcp_policy_set").server === "filesystem" &&
        lastArgs("mcp_policy_set").tool === "write" &&
        lastArgs("mcp_policy_set").effect === "ask" &&
        typeof lastArgs("mcp_policy_set").nowMs === "number" &&
        /1 ask/.test(polNote()),
    );

    d.getElementById("pol-server").value = "filesystem";
    d.getElementById("pol-tool").value = "";
    d.getElementById("pol-effect").value = "deny";
    d.getElementById("pol-add").click();
    await flush();
    assert(
      "a half-written rule is refused here, before any round trip",
      called("mcp_policy_set").length === 1 &&
        /needs a server and a tool/.test(polNote()) &&
        d.getElementById("pol-msg").classList.contains("err"),
    );

    d.getElementById("pol-tool").value = "truncate";
    d.getElementById("pol-add").click();
    await flush();
    assert(
      "adding a rule sends both halves and the effect, then clears the form",
      called("mcp_policy_set").length === 2 &&
        lastArgs("mcp_policy_set").server === "filesystem" &&
        lastArgs("mcp_policy_set").tool === "truncate" &&
        lastArgs("mcp_policy_set").effect === "deny" &&
        d.getElementById("pol-server").value === "" &&
        d.getElementById("pol-tool").value === "" &&
        /Rule saved: filesystem\/truncate → Deny\./.test(polNote()),
    );

    d.querySelector("#pol-rules .pol-clear").click();
    await flush();
    assert(
      "clearing a rule sends the pair it was stored under",
      called("mcp_policy_clear").length === 1 &&
        lastArgs("mcp_policy_clear").server === "filesystem" &&
        lastArgs("mcp_policy_clear").tool === "write" &&
        !/filesystem\/write/.test(polRuleText()),
    );

    const defaultSelect = d.getElementById("pol-default");
    defaultSelect.value = "deny";
    defaultSelect.dispatchEvent(new d.defaultView.Event("change"));
    await flush();
    assert(
      `changing the default sends it, and the line follows (${
        polNote().slice(0, 46)})`,
      called("mcp_policy_default").length === 1 &&
        lastArgs("mcp_policy_default").effect === "deny" &&
        /default deny/.test(polNote()) &&
        /0 of 3 tools attached/.test(polNote()),
    );

    // M8 — authoring a skill: what the directory holds, a local refusal, the
    // round trip's arguments, and the two readings of the same directory that
    // have to agree (the skills pane and the capability list).
    await loadSkills();
    const skillRows = [...d.querySelectorAll("#skill-list .skill-row")];
    const skillNote = () => d.getElementById("skill-note").textContent;
    assert(
      `M8: the skills pane lists what the loader would hand the agent (${skillRows.length} row(s))`,
      called("skill_list").length > 0 &&
        skillRows.length === 2 &&
        /2 skills · 1 off for this workspace/.test(skillNote()) &&
        /home\/e2e\/\.bos\/skills/.test(skillNote()) &&
        /34 chars of instruction/.test(skillRows[0].textContent) &&
        /release-notes/.test(skillRows[1].textContent),
    );
    skillRows[0].querySelector(".skill-use").click();
    await flush();
    assert(
      `M8: Use prefills the composer with the skill's name (${d.getElementById("input").value})`,
      d.getElementById("input").value === "Use the code-review skill: ",
    );
    // The palette caches the skill list on its first open since the last close,
    // and that cache is what a save has to invalidate — so fill it first, or
    // there is nothing for the check after the save to catch.
    d.getElementById("input").value = "/c";
    d.getElementById("input").dispatchEvent(new Event("input", { bubbles: true }));
    await flush();
    assert(
      `M8: the palette cached the skill list before the save (${state.skills.length} cached)`,
      state.skills.length > 0,
    );
    d.getElementById("input").value = "an ordinary message";
    d.getElementById("input").dispatchEvent(new Event("input", { bubbles: true }));
    await flush();
    d.getElementById("skill-name").value = "pr-review";
    d.getElementById("skill-desc").value = "Review a pull request";
    d.getElementById("skill-body").value = "   ";
    d.getElementById("skill-save").click();
    await flush();
    assert(
      `M8: a skill with no body is refused without a round trip (${skillNote()})`,
      called("skill_author").length === 0 && /needs a body/.test(skillNote()),
    );
    d.getElementById("skill-body").value = "Read the diff, then list findings.";
    const capsCallsBeforeSave = called("list_capabilities").length;
    d.getElementById("skill-save").click();
    await flush();
    assert(
      `M8: saving sends the skill and clears the form (${JSON.stringify(
        lastArgs("skill_author"),
      ).slice(0, 72)})`,
      called("skill_author").length === 1 &&
        lastArgs("skill_author").name === "pr-review" &&
        lastArgs("skill_author").description === "Review a pull request" &&
        lastArgs("skill_author").body === "Read the diff, then list findings." &&
        lastArgs("skill_author").overwrite === false &&
        typeof lastArgs("skill_author").nowMs === "number" &&
        d.getElementById("skill-name").value === "" &&
        d.getElementById("skill-desc").value === "" &&
        d.getElementById("skill-body").value === "" &&
        /saved pr-review/.test(skillNote()),
    );
    // Both readings of the directory have to be *asked again*: the pane's
    // capability list, and the palette's own cache. Counting the calls is the
    // only way to see a cache drop through a settled fixture — the fixture
    // answers the same thing before and after, so the effect is the re-read.
    const capsCallsAfterSave = called("list_capabilities").length;
    const paletteCallsBefore = capsCallsAfterSave;
    d.getElementById("input").value = "/p";
    d.getElementById("input").dispatchEvent(new Event("input", { bubbles: true }));
    await flush();
    const offered = [...d.querySelectorAll("#cmd-list [role=option]")].map((o) => o.textContent);
    assert(
      `M8: a save re-reads both views — the capability list and the palette's cache (${
        capsCallsBeforeSave}→${capsCallsAfterSave} then ${paletteCallsBefore}→${
        called("list_capabilities").length}, offered=${JSON.stringify(offered.slice(0, 3))})`,
      /pr-review/.test(d.getElementById("cap-items").textContent) &&
        capsCallsAfterSave > capsCallsBeforeSave &&
        called("list_capabilities").length > paletteCallsBefore &&
        state.skills.length > 0 &&
        offered.some((text) => /pr-review/.test(text)),
    );
    d.getElementById("input").value = "an ordinary message";
    d.getElementById("input").dispatchEvent(new Event("input", { bubbles: true }));
    await flush();
    d.getElementById("skill-name").value = "code-review";
    d.getElementById("skill-desc").value = "Review a diff";
    d.getElementById("skill-body").value = "Rewritten.";
    d.getElementById("skill-overwrite").checked = true;
    d.getElementById("skill-save").click();
    await flush();
    assert(
      "M8: Replace reaches the command as overwrite",
      called("skill_author").length === 2 && lastArgs("skill_author").overwrite === true,
    );

    // M8 — an MCP catalog: what each server publishes beyond tools, and a read
    // that is a gesture rather than a side effect of being connected.
    const catCallsBefore = called("mcp_catalog").length;
    d.getElementById("set-tab-set-mcp").click();
    await flush();
    assert(
      "M8: opening the MCP pane asks for the catalog",
      called("mcp_catalog").length > catCallsBefore &&
        typeof lastArgs("mcp_catalog").nowMs === "number",
    );
    const catHost = () => d.getElementById("mcp-catalog");
    const catHeads = () =>
      [...catHost().querySelectorAll(".cat-server")].map(
        (block) => block.querySelector(".cat-head").textContent,
      );
    const catRows = () => [...catHost().querySelectorAll(".cat-row")];
    const readMsg = () => d.getElementById("mcp-read-msg").textContent;
    assert(
      `M8: every server is drawn with what it publishes (${catHeads().join(" | ")})`,
      catHeads().length === 4 &&
        /filesystem/.test(catHeads()[0]) &&
        /2 resources · 1 prompt/.test(catHeads()[0]) &&
        /not connected this pass/.test(catHeads()[1]) &&
        /broken/.test(catHeads()[2]) &&
        /no prompts capability/.test(catHeads()[2]) &&
        /second/.test(catHeads()[3]) &&
        /1 resource · no prompts capability/.test(catHeads()[3]) &&
        catRows().length === 4 &&
        /application\/json/.test(catRows()[0].textContent) &&
        /2 arguments · 1 required/.test(catRows()[2].textContent) &&
        /file:\/\/\/notes\/todo\.md|todo/.test(catRows()[3].textContent) &&
        /4 servers · rows up to 200 · a read is cut at 20000 chars/.test(
          d.getElementById("mcp-cat-msg").textContent,
        ),
    );
    assert(
      "M8: a listing failure is that server's own line, and the picker offers only live servers",
      /connection reset/.test(catHost().textContent) &&
        [...d.getElementById("mcp-read-server").options].map((o) => o.value).join(",") ===
          "filesystem,broken,second",
    );
    catRows()[0].querySelector(".cat-use").click();
    await flush();
    const readOut = d.getElementById("mcp-read-out");
    assert(
      `M8: a resource row reads that URI and shows the text (${readMsg()})`,
      called("mcp_read_resource").length === 1 &&
        lastArgs("mcp_read_resource").server === "filesystem" &&
        lastArgs("mcp_read_resource").uri === "file:///repo/schema.json" &&
        typeof lastArgs("mcp_read_resource").nowMs === "number" &&
        readOut.textContent === '{"type":"object"}' &&
        !readOut.hidden &&
        /filesystem\/file:\/\/\/repo\/schema.json · 17 chars · application\/json/.test(readMsg()),
    );
    d.getElementById("mcp-uri").value = "   ";
    d.getElementById("mcp-read-btn").click();
    await flush();
    assert(
      "M8: a blank URI is refused here, with no round trip",
      called("mcp_read_resource").length === 1 && /needs a URI/.test(readMsg()),
    );
    d.getElementById("mcp-uri").value = " file:///repo/runbook.md ";
    d.getElementById("mcp-read-btn").click();
    await flush();
    assert(
      `M8: a typed URI is trimmed and sent as typed (${lastArgs("mcp_read_resource").uri})`,
      called("mcp_read_resource").length === 2 &&
        lastArgs("mcp_read_resource").uri === "file:///repo/runbook.md" &&
        lastArgs("mcp_read_resource").server === "filesystem",
    );
    // A row belongs to its own server: the picker currently names filesystem,
    // and this row is second's, so a row that read from the picker instead of
    // from its server would be caught here.
    d.getElementById("mcp-read-server").value = "filesystem";
    catRows()[3].querySelector(".cat-use").click();
    await flush();
    assert(
      `M8: a resource row reads from its own server (${lastArgs("mcp_read_resource").server}/${lastArgs("mcp_read_resource").uri})`,
      called("mcp_read_resource").length === 3 &&
        lastArgs("mcp_read_resource").server === "second" &&
        lastArgs("mcp_read_resource").uri === "file:///notes/todo.md",
    );

    // M8 — a chat's own skills: the scope it carries, the switch that changes
    // it, and the workspace denial that is a floor under both.
    const restoreScopeSession = state.activeId;
    state.activeId = "e2e";
    await loadSkills();
    const scopeRows = () => [...d.querySelectorAll("#skill-list .skill-row")];
    const scopeToggles = () => scopeRows().map((row) => row.querySelector(".skill-toggle"));
    const inheritBtn = d.getElementById("skill-inherit");
    assert(
      `M8: the skills pane reads the open chat's own list (${called("skill_activation").length} read(s) for ${
        lastArgs("skill_activation") && lastArgs("skill_activation").session
      })`,
      called("skill_activation").length > 0 &&
        lastArgs("skill_activation").session === "e2e" &&
        typeof lastArgs("skill_activation").nowMs === "number",
    );
    assert(
      `M8: every skill row carries this chat's switch, pressed where the chat carries it (${
        scopeToggles().map((t) => t.getAttribute("aria-pressed")).join("/")
      })`,
      scopeToggles().length === 2 &&
        scopeToggles()[0].getAttribute("aria-pressed") === "true" &&
        scopeToggles()[1].getAttribute("aria-pressed") === "false" &&
        scopeToggles()[0].disabled === false &&
        inheritBtn.hidden === true &&
        d.getElementById("skill-scope").textContent.length > 0,
    );
    assert(
      "M8: a skill the workspace switched off is present and disabled, so the reason is readable",
      scopeToggles()[1].disabled === true && scopeToggles()[1].title.length > 0,
    );
    const listsBefore = called("skill_list").length;
    scopeToggles()[0].click();
    await flush();
    assert(
      `M8: switching a skill off sends the chat, the skill and the direction (${
        lastArgs("skill_set_active") && lastArgs("skill_set_active").name
      }/${lastArgs("skill_set_active") && lastArgs("skill_set_active").active})`,
      called("skill_set_active").length === 1 &&
        lastArgs("skill_set_active").session === "e2e" &&
        lastArgs("skill_set_active").name === "code-review" &&
        lastArgs("skill_set_active").active === false &&
        typeof lastArgs("skill_set_active").nowMs === "number",
    );
    assert(
      `M8: the switch redraws from its own answer without re-reading the directory (${
        called("skill_list").length - listsBefore
      } re-read(s), inherit=${inheritBtn.hidden})`,
      called("skill_list").length === listsBefore &&
        scopeToggles()[0].getAttribute("aria-pressed") === "false" &&
        inheritBtn.hidden === false,
    );
    inheritBtn.click();
    await flush();
    assert(
      `M8: following the workspace again is one call, and the pane stops offering it (${
        lastArgs("skill_inherit") && lastArgs("skill_inherit").session
      })`,
      called("skill_inherit").length === 1 &&
        lastArgs("skill_inherit").session === "e2e" &&
        scopeToggles()[0].getAttribute("aria-pressed") === "true" &&
        inheritBtn.hidden === true,
    );
    // A report that belongs to another chat is not this chat's answer: the
    // rows must not claim On/Off from it, and nothing may be clicked.
    const readsBefore = called("skill_activation").length;
    state.skillActivation = { session: "some-other-chat", explicit: false, active: ["code-review"] };
    renderSkills();
    assert(
      "M8: another chat's activation is not drawn as this chat's, and no switch is armed",
      scopeToggles().every((t) => t.disabled === true) &&
        scopeToggles().every((t) => t.getAttribute("aria-pressed") === "false") &&
        called("skill_activation").length === readsBefore,
    );
    // The inventory follows the chat too, and asks about no chat when none is
    // open — a session-scoped panel refuses rather than calling with an empty id.
    await loadCapabilities();
    const capArgs = lastArgs("list_capabilities");
    const scopeSession = state.activeId;
    state.activeId = "";
    await loadSkills();
    const offlineReads = called("skill_activation").length;
    assert(
      `M8: every chat-scoped read names the open chat or none (${capArgs && capArgs.session}, ${
        scopeToggles().filter((t) => t.disabled).length
      } disabled)`,
      capArgs.session === scopeSession &&
        scopeToggles().every((t) => t.disabled === true) &&
        called("skill_activation").length === offlineReads,
    );
    state.activeId = restoreScopeSession;
    state.skillActivation = null;

    // Sidebar — the reference row menu, on the real markup. The three chats are
    // seeded rather than fetched because this guard's fixtures are settled: the
    // promises asserted here are *which command a gesture sends and with what*,
    // and whether the menu costs a round trip. What the backend does with that
    // command is the live guard's business.
    const restoreConfirm = window.confirm;
    const restoreSidebar = {
      sessions: state.sessions,
      workspaces: state.workspaces,
      activeWorkspace: state.activeWorkspace,
      activeId: state.activeId,
    };
    // The MCP switches live on the workspace the sidebar already holds, and one
    // server starts *off* on purpose: a dialog that ticked everything, or a
    // payload that ignored the boxes, would be indistinguishable if all of them
    // were on.
    const w1 = {
      id: "w1", name: "bos", root: "/tmp/bos", model: "",
      mcp_servers: [
        { name: "filesystem", transport: "stdio", command: "mcp-filesystem", url: "", enabled: true },
        { name: "notes", transport: "stdio", command: "mcp-notes", url: "", enabled: false },
      ],
    };
    // Two workspaces, because the "+" has two different jobs: in the workspace
    // that is already active it only creates, and in another one it switches
    // first. One workspace cannot tell those apart.
    state.workspaces = [w1, { id: "w2", name: "trino", root: "/tmp/trino", model: "" }];
    state.activeWorkspace = "w1";
    state.sessions = [
      // The pinned chat is the *older* one on purpose: with it newer, a list
      // that ignored the pin would happen to put it first anyway, and the check
      // could not fail for the reason it exists.
      { id: "s1", title: "pinned chat", updated_at: 100, workspace: "w1", pinned: true, archived: false },
      { id: "s2", title: "plain chat", updated_at: 300, workspace: "w1", pinned: false, archived: false },
      { id: "s3", title: "archived chat", updated_at: 100, workspace: "w1", pinned: false, archived: true },
      { id: "s4", title: "elsewhere", updated_at: 150, workspace: "w2", pinned: false, archived: false },
    ];
    state.view.filter = "hide";
    renderSidebar();
    const rows = () => [...d.querySelectorAll("#session-list .session-row")];
    const rowOf = (id) => d.querySelector(`#session-list .session-row[data-id="${id}"]`);
    const openMenu = (host) => d.querySelector(`.row-menu[aria-label="${host}"]`);
    assert(
      `the pinned chat floats and the archived one is not in the list (${rows()
        .map((r) => r.dataset.id)
        .join(",")})`,
      rows().length === 3 &&
        rows()
          .map((r) => r.dataset.id)
          .join(",") === "s1,s2,s4" &&
        rows()[0].dataset.id === "s1" &&
        rowOf("s1").querySelector(".sess-pin").getAttribute("aria-pressed") === "true" &&
        rowOf("s2").querySelector(".sess-pin").getAttribute("aria-pressed") === "false" &&
        !rowOf("s3") &&
        state.view.filter === "hide",
    );

    const beforeMenu = calls.length;
    rowOf("s2").querySelector(".sess-menu").click();
    const menu = openMenu("Actions for plain chat");
    const items = menu ? [...menu.querySelectorAll('[role="menuitem"]')] : [];
    assert(
      `the row menu is a menu whose entries name the reference actions (${
        items.map((i) => i.dataset.run).join("/")})`,
      !!menu &&
        menu.getAttribute("role") === "menu" &&
        items.length === 5 &&
        items.map((i) => i.dataset.run).join(",") === "pin,rename,fork,archive,delete" &&
        /Delete session/.test(items[4].textContent) &&
        items[4].classList.contains("danger") &&
        items.every((i) => i.textContent.trim().length > 0) &&
        d.activeElement === items[0] &&
        calls.length === beforeMenu,
    );
    assert(
      "the row menu spells out the keys it answers",
      /Cmd\/Ctrl\+G/.test(items[1].textContent) && /Cmd\/Ctrl\+F/.test(items[2].textContent),
    );
    // Dispatched at the focused entry, the way a real keypress arrives: an
    // event fired at `document` never reaches a listener on the menu.
    items[0].dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    assert(
      `Escape closes the menu and hands the keyboard back (open=${!!openMenu("Actions for plain chat")}, active=${
        d.activeElement && d.activeElement.className
      })`,
      !openMenu("Actions for plain chat") &&
        d.activeElement === rowOf("s2").querySelector(".sess-menu") &&
        rowOf("s2").querySelector(".sess-menu").getAttribute("aria-expanded") === "false",
    );
    const menuNow = () => d.querySelector(".row-menu");
    // The head's sliders: one menu for how the list is drawn, and the only
    // writer of that state — so every mode is entered through its own item, the
    // way a person does, rather than by setting the state the menu sets.
    const viewBtn = d.getElementById("session-view");
    const viewItems = () => [...d.querySelectorAll(".row-menu .row-menu-item")];
    // Every mode is entered through a named step: a mode that cannot be reached
    // must say *which* one it was, not just "undefined is not a function".
    const step = (name, fn) => {
      try {
        return fn();
      } catch (e) {
        throw new Error(`${name}: ${e && e.message}`);
      }
    };
    const pickView = (name) =>
      step(name, () => {
        const item = viewItems().find((b) => new RegExp(`^${name}`).test(b.textContent));
        if (!item) throw new Error(`no ${name} item among [${viewItems().map((b) => b.dataset.run).join(",")}]`);
        item.click();
      });
    const viewItem = (re) => viewItems().find((b) => re.test(b.textContent));
    viewBtn.click();
    assert(
      `the head's sliders opens a radio menu of the three choices (${viewItems()
        .map((b) => b.dataset.run)
        .join(" / ")})`,
      !!menuNow() &&
        viewItems().length === 9 &&
        viewItems()
          .slice(0, 8)
          .every((b) => b.getAttribute("role") === "menuitemradio") &&
        viewItems()[8].getAttribute("role") === "menuitem" &&
        viewItems()
          .filter((b) => b.getAttribute("aria-checked") === "true")
          .map((b) => b.dataset.run)
          .join(",") === "group:tree,order:updated,filter:hide" &&
        [...d.querySelectorAll(".row-menu .row-menu-head")].map((h) => h.textContent).join(",") ===
          "Group by,Order by,Filter sessions,Chats" &&
        viewBtn.getAttribute("aria-expanded") === "true",
    );
    assert(
      `the menu counts the archived chats and ticks the choice that hides them (${viewItem(/^Hide archived/).textContent.trim()})`,
      /Hide archived \(1\)/.test(viewItem(/^Hide archived/).textContent) &&
        /Archived only \(1\)/.test(viewItem(/^Archived only/).textContent) &&
        /^Reload chats/.test(viewItems()[8].textContent),
    );
    // "In one list" is the mode with no workspace rows, so the row has to carry
    // the workspace itself — otherwise the choice would drop a fact.
    pickView("In one list");
    assert(
      `"In one list" drops the workspace rows and names each row's workspace (${rows()
        .map((r) => r.dataset.id)
        .join(",")} / ${[...d.querySelectorAll("#session-list .sess-ws-chip")]
        .map((c) => c.textContent)
        .join(",")})`,
      !d.querySelector("#session-list .ws-head") &&
        rows()
          .map((r) => r.dataset.id)
          .join(",") === "s1,s2,s4" &&
        [...d.querySelectorAll("#session-list .sess-ws-chip")].map((c) => c.textContent).join(",") ===
          "bos,bos,trino" &&
        viewItem(/^In one list/).getAttribute("aria-checked") === "true" &&
        !!menuNow(),
    );
    // "WorkSpace" keeps every workspace as a row and opens only the active one.
    pickView("WorkSpace");
    assert(
      `"WorkSpace" keeps a row per workspace and opens only the active one (heads=${d.querySelectorAll(
        "#session-list .ws-head",
      ).length}, folds=${d.querySelectorAll("#session-list .ws-fold").length}, rows=${rows()
        .map((r) => r.dataset.id)
        .join(",")})`,
      d.querySelectorAll("#session-list .ws-head").length === 2 &&
        d.querySelectorAll("#session-list .ws-fold").length === 1 &&
        rows()
          .map((r) => r.dataset.id)
          .join(",") === "s1,s2",
    );
    // A fixture whose array order is *not* its recency order, because "Manual"
    // and "Last updated" would otherwise be indistinguishable — and one that can
    // tell them apart is the only one that tests either.
    const restoreView = state.sessions;
    state.sessions = [
      { id: "o1", title: "older", updated_at: 50, workspace: "w1", pinned: false, archived: false },
      { id: "o2", title: "newer", updated_at: 900, workspace: "w1", pinned: false, archived: false },
    ];
    state.view = { group: "tree", order: "updated", filter: "hide" };
    renderSidebar();
    const drawn = () => rows().map((r) => r.dataset.id).join(",");
    const freshest = drawn();
    // The menu is still open from the pick above — and that is a promise of its
    // own: the three choices are usually set together, so a pick leaves it up.
    pickView("Manual");
    assert(
      `"Manual" keeps the store's order where "Last updated" reorders (updated=${freshest}, manual=${drawn()})`,
      freshest === "o2,o1" && drawn() === "o1,o2",
    );
    // The filter is the archive's only door now, in all three of its states.
    state.sessions = restoreView;
    state.view = { group: "tree", order: "updated", filter: "hide" };
    renderSidebar();
    pickView("All conversations");
    assert(
      `"All conversations" puts an archived chat back among its own workspace's (${drawn()} / heads=${
        d.querySelectorAll("#session-list .ws-head").length
      })`,
      drawn() === "s1,s2,s3,s4" &&
        !!rowOf("s3").classList.contains("archived-row") &&
        !!rowOf("s3").querySelector(".sess-restore") &&
        d.querySelectorAll("#session-list .ws-head").length === 2,
    );
    pickView("Archived only");
    assert(
      `"Archived only" lists the archive alone, with no group for a workspace that has nothing archived (${drawn()} / heads=${
        d.querySelectorAll("#session-list .ws-head").length
      })`,
      drawn() === "s3" && d.querySelectorAll("#session-list .ws-head").length === 1,
    );
    assert(
      `the choice is written down as one blob the next run reads back (${localStorage.getItem(
        "bos.sessionView",
      )})`,
      JSON.parse(localStorage.getItem("bos.sessionView")).filter === "archived" &&
        parseViewBlob({ group: "nope", order: "manual" }).group === "tree" &&
        parseViewBlob({ group: "nope", order: "manual" }).order === "manual" &&
        parseViewBlob(null).filter === "hide" &&
        parseViewBlob("garbage").group === "tree",
    );
    closeRowMenu(false);
    localStorage.removeItem("bos.sessionView");
    state.sessions = restoreView;
    state.view = { group: "tree", order: "updated", filter: "hide" };
    renderSidebar();
    // Every entry is reached through a helper that reopens the menu and reports
    // what it found, so a missing menu fails *this* check instead of throwing
    // and taking every check after it with it.
    const pick = (id, run) => {
      rowOf(id).querySelector(".sess-menu").click();
      const m = menuNow();
      const item = m ? m.querySelector(`[data-run="${run}"]`) : null;
      if (item) item.click();
      return { opened: !!m, clicked: !!item, items: m ? m.children.length : 0 };
    };
    const picked = pick("s2", "pin");
    await flush();
    assert(
      `the pin entry pins the chat whose row opened the menu (${
        picked.clicked ? JSON.stringify(lastArgs("set_session_pinned")) : JSON.stringify(picked)
      })`,
      picked.clicked &&
        called("set_session_pinned").length === 1 &&
        lastArgs("set_session_pinned").id === "s2" &&
        lastArgs("set_session_pinned").pinned === true,
    );
    // The confirm is stubbed *before* the click: letting the real one run puts a
    // native dialog in front of a headless browser that has nobody to answer it,
    // and the whole guard hangs instead of failing.
    window.confirm = () => false;
    pick("s2", "delete");
    await flush();
    assert(
      "the destructive entry asks before it deletes, and does nothing when refused",
      called("delete_session").length === 0,
    );
    window.confirm = () => true;
    pick("s2", "delete");
    await flush();
    assert(
      `Delete in the menu reaches the trash (${JSON.stringify(lastArgs("delete_session"))})`,
      called("delete_session").length === 1 && lastArgs("delete_session").id === "s2",
    );
    pick("s2", "rename");
    const field = rowOf("s2").querySelector(".sess-rename");
    assert(
      `Rename turns the row's own title into a focused field (${field && field.value})`,
      !!field && d.activeElement === field && field.value === "plain chat",
    );
    field.value = "  renamed here  ";
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await flush();
    assert(
      `Enter sends the name as typed and the row comes back (${JSON.stringify(lastArgs("rename_session"))})`,
      called("rename_session").length === 1 &&
        lastArgs("rename_session").id === "s2" &&
        lastArgs("rename_session").title === "  renamed here  " &&
        !rowOf("s2").querySelector(".sess-rename"),
    );
    pick("s2", "rename");
    const field2 = rowOf("s2").querySelector(".sess-rename");
    if (field2) field2.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flush();
    assert(
      `Escape leaves the name alone (fields=${rowOf("s2").querySelectorAll(".sess-rename").length})`,
      called("rename_session").length === 1 && !rowOf("s2").querySelector(".sess-rename"),
    );
    // A row menu opened from an archived row carries the same entries, and the
    // archived chat's own entry offers to put it back rather than to archive it.
    // The filter the view menu sets is what puts that row in the list: there is
    // no second disclosure writing the same choice.
    state.view.filter = "all";
    renderSidebar();
    const archivedRow = rowOf("s3");
    if (archivedRow && archivedRow.querySelector(".sess-menu")) {
      archivedRow.querySelector(".sess-menu").click();
    }
    const archivedItems = menuNow() ? [...menuNow().querySelectorAll(".row-menu-item")] : [];
    assert(
      `an archived chat's menu offers Restore instead of Archive (rows=${rows()
        .map((r) => r.dataset.id)
        .join(",")}, items=${archivedItems.map((i) => i.textContent.trim()).join(" / ")})`,
      !!archivedRow &&
        archivedItems.length === 5 &&
        /Restore session/.test(archivedItems[3].textContent) &&
        archivedRow.querySelector(".sess-restore") !== null,
    );
    closeRowMenu(false);
    pick("s2", "archive");
    await flush();
    assert(
      `Archive asks the backend to hide the chat (${JSON.stringify(lastArgs("set_session_archived"))})`,
      called("set_session_archived").length === 1 &&
        lastArgs("set_session_archived").id === "s2" &&
        lastArgs("set_session_archived").archived === true,
    );
    // The workspace header: "+" starts a chat in *that* workspace, which means
    // switching to it first — creating in a workspace that is not active would
    // need a second way to say which workspace a new chat belongs to.
    const wsCalls = calls.length;
    const trinoAdd = d.querySelectorAll("#session-list .ws-new")[1];
    const trinoLabel = trinoAdd.getAttribute("aria-label");
    trinoAdd.click();
    await flush();
    const wsSeq = calls
      .slice(wsCalls)
      .map((c) => c.cmd)
      .filter((cmd) => cmd === "set_active_workspace" || cmd === "create_session");
    assert(
      `the + on another workspace switches to it and only then starts a chat (${wsSeq.join(" then ")})`,
      wsSeq.join(",") === "set_active_workspace,create_session" &&
        trinoLabel.includes("trino"),
    );
    const homeCalls = calls.length;
    d.querySelectorAll("#session-list .ws-new")[0].click();
    await flush();
    assert(
      "the + on the active workspace does not switch away and back",
      calls
        .slice(homeCalls)
        .filter((c) => c.cmd === "set_active_workspace").length === 0 &&
        calls.slice(homeCalls).some((c) => c.cmd === "create_session"),
    );
    // The row's anatomy, which the user drew a box around (rev 116) and then asked to
    // match the reference sidebar's icons (rev 117): a folder at rest, the name, then
    // the menu and the row's own ⊕ in the reference's order, then the active dot and
    // the chat count. No model on the row — so the first workspace is given one first,
    // because "no chip" on a row with no model to show is a check that could not fail.
    const label = (n) =>
      n.classList.contains("ws-name") ? "name"
        : n.classList.contains("ws-new") ? "+"
          : n.classList.contains("ws-menu") ? "…"
            : n.classList.contains("ws-fold") ? "fold"
              : n.classList.contains("ws-folder") ? "folder"
                : n.classList.contains("ws-dot") ? "dot"
                  : n.classList.contains("ws-count") ? "count"
                    : n.className;
    const reads = (h) => {
      const order = [...h.children].map(label);
      const at = (k) => order.indexOf(k);
      return (
        at("folder") > -1 &&
        at("folder") < at("name") &&
        at("name") < at("…") &&
        at("…") < at("+") &&
        // Every row has a count; only the active one carries the dot, so the dot's
        // own position is checked where it exists (and exactly one row must have it).
        (at("dot") === -1 || at("+") < at("dot")) &&
        at("+") < at("count") &&
        // The disclosure comes first — the state reads before the identity — and a row
        // that cannot fold simply has none (it keeps the column with a spacer).
        (at("fold") === -1 || at("fold") < at("folder")) &&
        !h.querySelector(".ws-model")
      );
    };
    const hadModels = state.workspaces.map((w) => w.model);
    if (state.workspaces.length) state.workspaces[0].model = "glm-4.7";
    renderSidebar();
    const wshHeads = [...d.querySelectorAll("#session-list .ws-head")];
    const wshRows = wshHeads.map((h) => [...h.children].map(label).join(","));
    assert(
      `every workspace row reads [fold, folder], name, …, +, [dot,] count (${wshRows.join(" | ")})`,
      wshHeads.length >= 2 &&
        wshHeads.every(reads) &&
        wshRows.filter((r) => r.includes("dot")).length === 1,
    );
    state.workspaces.forEach((w, i) => {
      w.model = hadModels[i];
    });
    renderSidebar();
    // rev 119: the disclosure is the one control that must read *before* the pointer
    // arrives, so its two states are two different icons. The stylesheet is what keeps
    // it visible at rest (markup_selftest.sh reads that rule and its size); what only
    // this layer can show is that the two states really are two *different* shapes —
    // flipping a row has to change the icon, and flipping it back has to restore it.
    const dOf = () => {
      const h = [...d.querySelectorAll("#session-list .ws-head")].find((x) => x.querySelector(".ws-fold"));
      return h ? h.querySelector(".ws-fold .ico path").getAttribute("d") : "";
    };
    const flipFold = async () => {
      const h = [...d.querySelectorAll("#session-list .ws-head")].find((x) => x.querySelector(".ws-fold"));
      h.querySelector(".ws-fold").click();
      await flush();
    };
    const arrowOpen = dOf();
    await flipFold();
    const arrowShut = dOf();
    await flipFold();
    const arrowBack = dOf();
    assert(
      `the disclosure's two states are two different icons (${arrowOpen} vs ${arrowShut}, back to ${arrowBack})`,
      !!arrowOpen && !!arrowShut && arrowOpen !== arrowShut && arrowBack === arrowOpen,
    );
    // rev 119: a chat that is streaming is the one row in this list that is doing
    // something, so it and the workspace that owns it take the success colour. The
    // colour is a stylesheet fact (markup_selftest.sh reads both rules); the state has
    // to reach both rows, and has to leave them when the stream ends — which is what a
    // fixture that can set a stream proves.
    const liveId = d.querySelector("#session-list .session-row").dataset.id;
    state.streams[liveId] = { session_id: liveId, gen: 1 };
    renderAll();
    const liveRow = d.querySelector(`#session-list .session-row[data-id="${liveId}"]`);
    const headOf = (row) => {
      let n = row.previousElementSibling;
      while (n && !n.classList.contains("ws-head")) n = n.previousElementSibling;
      return n;
    };
    const liveHead = headOf(liveRow);
    assert(
      `a streaming chat and its workspace row both read as working (${liveRow.className} / ${liveHead ? liveHead.className : "no head"})`,
      liveRow.classList.contains("working") &&
        !!liveRow.querySelector(".sess-mark.live") &&
        !!liveHead &&
        liveHead.classList.contains("working"),
    );
    delete state.streams[liveId];
    renderAll();
    assert(
      "a finished chat stops reading as working",
      !d.querySelector("#session-list .session-row.working") &&
        !d.querySelector("#session-list .ws-head.working"),
    );

    const wsMenuBtn = d.querySelector("#session-list .ws-menu");
    wsMenuBtn.click();
    const wsItems = [...d.querySelectorAll(".row-menu .row-menu-item")];
    assert(
      `the workspace menu offers the reference actions and opens the remove form (${
        wsItems.map((i) => i.dataset.run).join("/")})`,
      wsItems.map((i) => i.dataset.run).join(",") === "new,rename,delete" &&
        /Delete workspace/.test(wsItems[2].textContent),
    );
    const beforeForm = calls.length;
    wsItems[1].click();
    await flush();
    assert(
      "Rename workspace opens the form instead of calling the backend blind",
      calls.length === beforeForm &&
        !!d.querySelector("#session-list .ws-form") &&
        !d.querySelector(".row-menu"),
    );
    // One invariant over everything the sidebar drew: an icon-only control has
    // an accessible name. The static guard can only see the wiring; this sees
    // the buttons, including the ones a row builds.
    const glyph = /^[^\p{L}\p{N}]+$/u;
    const unnamed = [...d.querySelectorAll("#session-list button, .row-menu button")].filter(
      (b) => glyph.test(b.textContent.trim()) && !b.getAttribute("aria-label"),
    );
    assert(
      `every glyph-only control in the sidebar has a name (${unnamed
        .map((b) => b.className)
        .join(",") || "none"})`,
      unnamed.length === 0,
    );
    window.confirm = restoreConfirm;
    state.sessions = restoreSidebar.sessions;
    state.workspaces = restoreSidebar.workspaces;
    state.activeWorkspace = restoreSidebar.activeWorkspace;
    state.activeId = restoreSidebar.activeId;
    state.view.filter = "hide";
    renderSidebar();

    // Adding a workspace — its own flow, on the real markup. It is checked here
    // rather than in Settings because the promise is precisely that it is *not*
    // in Settings: the button opens a dialog of its own, and the folder, the
    // MCP switches and the rest travel in one creation payload.
    const addModal = d.getElementById("ws-add");
    const addLabel = d.getElementById(addModal.getAttribute("aria-labelledby") || "");
    assert(
      `add-workspace is a labelled modal dialog of its own (${addLabel && addLabel.textContent.trim()})`,
      addModal.getAttribute("role") === "dialog" &&
        addModal.getAttribute("aria-modal") === "true" &&
        !!addLabel &&
        addLabel.textContent.trim() === "Add a workspace" &&
        !d.querySelector("#settings-modal #ws-add"),
    );
    state.workspaces = [
      { ...w1 },
      { id: "w2", name: "trino", root: "/tmp/trino", model: "" },
    ];
    state.activeWorkspace = "w1";
    const addOpener = d.getElementById("workspace-add-primary");
    const settingsReads = called("get_settings").length;
    const capsReads = called("list_capabilities").length;
    // The gesture is the folder, so the panel is asked first (the checks below do
    // that); the form is what a machine without one falls back to, and this is
    // that machine.
    state.error = "";
    pickerFails.value = true;
    addOpener.focus();
    addOpener.click();
    await flush();
    assert(
      `with no panel the folder-plus opens the form, its browser and the reason, and leaves Settings shut (${d.getElementById("ws-add-browse-hint").textContent}; ${called("get_settings").length - settingsReads} settings reads)`,
      !addModal.classList.contains("hidden") &&
        !d.getElementById("ws-add-browser").classList.contains("hidden") &&
        d.getElementById("ws-add-browse-hint").textContent.includes("osascript is not installed") &&
        d.getElementById("settings-modal").classList.contains("hidden") &&
        called("get_settings").length === settingsReads,
    );
    assert(
      `the dialog offers every MCP server the workspace carries, with its own switch (${[...d.querySelectorAll(".ws-add-mcp-on")].map((b) => b.value + "=" + b.checked).join(",")})`,
      [...d.querySelectorAll(".ws-add-mcp-on")].map((b) => b.value).join(",") === "filesystem,notes" &&
        d.querySelector('.ws-add-mcp-on[value="filesystem"]').checked === true &&
        d.querySelector('.ws-add-mcp-on[value="notes"]').checked === false,
    );
    // The two inventory lists are re-read on open, so the dialog shows plugins
    // and skills the backend registered rather than a remembered copy.
    assert(
      `opening the dialog re-reads the capability inventory (${called("list_capabilities").length - capsReads} read, ${[...d.querySelectorAll(".ws-add-skill-on")].map((b) => b.value).join(",") || "no"} skill, ${d.getElementById("ws-add-plugins").textContent})`,
      called("list_capabilities").length === capsReads + 1 &&
        [...d.querySelectorAll(".ws-add-skill-on")].map((b) => b.value).join(",") === "pr-review" &&
        d.querySelectorAll(".ws-add-plugin-on").length === 0 &&
        d.getElementById("ws-add-plugins").textContent.includes("No plugins"),
    );
    // The panel is the door the user expects: "Choose folder…" asks the backend
    // for the system dialog and adopts the path it returns. The in-app browser is
    // not opened at all — it is the fallback, not a second way of doing this.
    pickerFails.value = false;
    const asksBeforePick = called("choose_folder").length;
    d.getElementById("ws-add-root").value = "/home/e2e";
    d.getElementById("ws-add-browse").click();
    await flush();
    assert(
      `Choose folder… asks the system panel for a folder, starting where the field points (${JSON.stringify(lastArgs("choose_folder"))})`,
      called("choose_folder").length === asksBeforePick + 1 &&
        lastArgs("choose_folder").start === "/home/e2e" &&
        d.getElementById("ws-add-root").value === "/home/e2e/Documents" &&
        d.getElementById("ws-add-browser").classList.contains("hidden") &&
        d.getElementById("ws-add-name").value === "Documents",
    );
    // Dismissing the panel is a decision: the field keeps what it had, and no
    // error is written for it.
    const asksBeforeMiss = called("choose_folder").length;
    pickerChoice.value = null;
    d.getElementById("ws-add-root").value = "/home/e2e";
    d.getElementById("ws-add-browse").click();
    await flush();
    assert(
      `dismissing the panel changes nothing and is not reported as a failure (${d.getElementById("ws-add-root").value} / ${d.getElementById("ws-add-err").textContent})`,
      called("choose_folder").length === asksBeforeMiss + 1 &&
        d.getElementById("ws-add-root").value === "/home/e2e" &&
        d.getElementById("ws-add-err").textContent === "",
    );
    // A machine with no panel: the fallback opens with the reason, and the field
    // is not left dead — this is the path that reveals the browser from a click.
    const listsBeforeGone = called("list_folder").length;
    pickerChoice.value = "/home/e2e/Documents";
    pickerFails.value = true;
    d.getElementById("ws-add-browse").click();
    await flush();
    assert(
      `a system panel that is not there opens the in-app browser and says why (${d.getElementById("ws-add-browse-hint").textContent})`,
      !d.getElementById("ws-add-browser").classList.contains("hidden") &&
        d.getElementById("ws-add-browse-manual").getAttribute("aria-expanded") === "true" &&
        d.getElementById("ws-add-browse-hint").textContent.includes("osascript is not installed") &&
        called("list_folder").length === listsBeforeGone + 1 &&
        lastArgs("list_folder").path === "/home/e2e",
    );
    // The panel is available again, and its path closes the fallback that was
    // open a moment ago: the two doors end in one function, so landing a path
    // cannot leave the other door's list sitting open behind it. This check needs
    // the browser *open* to mean anything — with it shut, "still hidden" would be
    // true however the code behaved.
    pickerFails.value = false;
    d.getElementById("ws-add-root").value = "/home/e2e";
    d.getElementById("ws-add-browse").click();
    await flush();
    assert(
      `a path from the panel closes the fallback that was open (browser ${d.getElementById("ws-add-browser").className.trim()}, root ${d.getElementById("ws-add-root").value})`,
      d.getElementById("ws-add-root").value === "/home/e2e/Documents" &&
        d.getElementById("ws-add-browser").classList.contains("hidden") &&
        d.getElementById("ws-add-browse-manual").getAttribute("aria-expanded") === "false",
    );
    // The browser itself: "Browse in-app" opens it at the field's folder, each row
    // descends, and "Use this folder" writes the resolved path.
    const listsBeforeBrowse = called("list_folder").length;
    d.getElementById("ws-add-root").value = "/home/e2e";
    d.getElementById("ws-add-browse-manual").click();
    await flush();
    assert(
      `Browse in-app opens the listing at the field's folder (${JSON.stringify(lastArgs("list_folder"))})`,
      called("list_folder").length === listsBeforeBrowse + 1 &&
        lastArgs("list_folder").path === "/home/e2e" &&
        d.getElementById("ws-browse-path").textContent === "/home/e2e" &&
        [...d.querySelectorAll("#ws-browse-list .ws-browse-row")].map((r) => r.textContent).join(",") ===
          "bos,Documents",
    );
    d.querySelector('#ws-browse-list .ws-browse-row[data-path="/home/e2e/Documents"]').click();
    await flush();
    assert(
      `a sub-folder row descends into that folder (${JSON.stringify(lastArgs("list_folder").path)})`,
      lastArgs("list_folder").path === "/home/e2e/Documents",
    );
    d.getElementById("ws-browse-use").click();
    await flush();
    assert(
      `Use this folder fills the root field and suggests the folder's name (${d.getElementById("ws-add-root").value} / ${d.getElementById("ws-add-name").value})`,
      d.getElementById("ws-add-root").value === "/home/e2e/Documents" &&
        d.getElementById("ws-add-name").value === "Documents" &&
        d.getElementById("ws-add-browser").classList.contains("hidden"),
    );
    // A creation with no folder is refused before the backend is asked: the
    // message belongs next to the field, not in the status line behind a dialog.
    const createCalls = called("create_workspace").length;
    d.getElementById("ws-add-root").value = "   ";
    d.getElementById("ws-add-create").click();
    await flush();
    assert(
      `a creation with no folder is refused here, not by the backend (${d.getElementById("ws-add-err").textContent})`,
      called("create_workspace").length === createCalls &&
        d.getElementById("ws-add-err").textContent.includes("root folder"),
    );
    // Everything the form collects crosses in one payload: the unticked boxes
    // are the deny-lists, and the MCP list is exactly the servers left ticked.
    d.getElementById("ws-add-root").value = "/home/e2e/Documents";
    d.getElementById("ws-add-name").value = "docs";
    d.getElementById("ws-add-model").value = " glm-4.7 ";
    d.getElementById("ws-add-files").checked = false;
    d.querySelector('.ws-add-tool-on[value="plan"]').checked = false;
    d.querySelector('.ws-add-mcp-on[value="filesystem"]').checked = false;
    d.querySelector('.ws-add-mcp-on[value="notes"]').checked = true;
    d.querySelector('.ws-add-skill-on[value="pr-review"]').checked = false;
    state.error = "";
    d.getElementById("ws-add-create").click();
    await flush();
    const draft = lastArgs("create_workspace").config || {};
    assert(
      `Add workspace sends the folder, the name and the whole draft in one call (${JSON.stringify({ root: lastArgs("create_workspace").root, name: lastArgs("create_workspace").name, model: draft.model, tools: draft.disabled_tools, skills: draft.disabled_skills, mcp: draft.mcp_enabled })})`,
      called("create_workspace").length === createCalls + 1 &&
        lastArgs("create_workspace").root === "/home/e2e/Documents" &&
        lastArgs("create_workspace").name === "docs" &&
        draft.model === "glm-4.7" &&
        draft.disabled_tools.join(",") === "plan" &&
        draft.disabled_skills.join(",") === "pr-review" &&
        draft.mcp_enabled.join(",") === "notes" &&
        draft.file_tools_enabled === false &&
        draft.bash_enabled === true &&
        draft.require_approval === true,
    );
    assert(
      `the dialog closes once the workspace exists and the sidebar shows it (${[...d.querySelectorAll("#session-list .ws-name")].map((n) => n.textContent).join(",")})`,
      addModal.classList.contains("hidden") && !!d.querySelector("#session-list .ws-name"),
    );
    // Escape closes it, and hand focus back to what opened it. The form has to be
    // open for that: the panel answers first, so this machine has none again.
    pickerFails.value = true;
    addOpener.click();
    await flush();
    d.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flush();
    assert(
      `Escape closes Add-workspace and returns focus to its opener (${d.activeElement && d.activeElement.id})`,
      addModal.classList.contains("hidden") && d.activeElement === addOpener,
    );
    // The door itself, last because it leaves a workspace behind: the folder-plus
    // asks the system's panel *first*, and its answer becomes the workspace —
    // named for the folder, seeded from the installation — with the form never
    // opened at all. Driving it here rather than in Settings is the point: adding
    // a workspace is not inspecting one.
    const pickerReads = called("choose_folder").length;
    const createdByPick = called("create_workspace").length;
    state.error = "";
    pickerFails.value = false;
    pickerChoice.value = "/home/e2e/bos";
    addOpener.click();
    await flush();
    assert(
      `the folder-plus asks the system panel directly and the pick is the workspace (${JSON.stringify(lastArgs("create_workspace"))}, form ${addModal.className.trim()})`,
      called("choose_folder").length === pickerReads + 1 &&
        called("create_workspace").length === createdByPick + 1 &&
        lastArgs("create_workspace").root === "/home/e2e/bos" &&
        lastArgs("create_workspace").name === "bos" &&
        lastArgs("create_workspace").config === null &&
        addModal.classList.contains("hidden") &&
        d.getElementById("settings-modal").classList.contains("hidden"),
    );
    // A folder that is already a workspace's root is focused rather than recorded
    // twice: with a panel, one click on a folder you already have is the easy
    // mistake, and two records for one folder is a state nobody asked for.
    pickerChoice.value = "/tmp/trino";
    addOpener.click();
    await flush();
    assert(
      `a folder that already is a workspace is focused instead of added again (${called("create_workspace").length} creations, asked for ${JSON.stringify(lastArgs("set_active_workspace"))})`,
      called("create_workspace").length === createdByPick + 1 &&
        lastArgs("set_active_workspace").id === "w2" &&
        addModal.classList.contains("hidden"),
    );
    // The row that used to sit under the list ("+ New workspace") is gone, and
    // the header's folder-plus above is the whole of the door: the check is that
    // nothing was left behind, driven by rendering rather than by reading. It is
    // not vacuous — the same assertion is what reddens when the row comes back.
    state.wsEdit = null;
    renderSidebar();
    assert(
      `the sidebar draws no add row of its own (rows=${d.querySelectorAll("#session-list .ws-add").length}, presets=${d.querySelectorAll("#session-list .ws-preset-row").length}, text=${/New workspace/.test(d.getElementById("session-list").textContent)})`,
      !d.querySelector("#session-list .ws-add") &&
        !d.querySelector("#session-list .ws-form") &&
        !/New workspace/.test(d.getElementById("session-list").textContent),
    );
    state.wsEdit = null;
    renderSidebar();
    // The active-workspace dot is an element, not a `::before`: a status drawn
    // only by CSS cannot be asserted on, and it cannot be announced either.
    const heads = Array.from(d.querySelectorAll("#session-list .ws-head"));
    const activeHeads = heads.filter((h) => h.classList.contains("active"));
    const dotted = heads.filter((h) => h.querySelector(".ws-dot"));
    assert(
      `exactly the active workspace's header carries the dot and aria-current (${heads.length} headers, ${activeHeads.length} active, ${dotted.length} dotted)`,
      heads.length >= 2 &&
        activeHeads.length === 1 &&
        dotted.length === 1 &&
        dotted[0] === activeHeads[0] &&
        activeHeads[0].getAttribute("aria-current") === "true" &&
        heads.filter((h) => h.getAttribute("aria-current")).length === 1,
    );

    const restoreSession = state.activeId;
    state.activeId = "e2e";

    // M7 — jobs: list, output tail, and stopping one.
    await refreshJobs();
    const jobRows = d.querySelectorAll("#jobs-list .job-row");
    const jobStates = Array.from(jobRows).map((r) => (r.querySelector(".job-state") || {}).textContent);
    assert(
      `M7: the job list renders each job with its state (${jobStates.join(",")})`,
      called("jobs_list").length === 1 &&
        jobRows.length === 2 &&
        jobStates.join(",") === "running,failed" &&
        /2 shown/.test(d.getElementById("jobs-count").textContent),
    );
    jobRows[0].click();
    await flush();
    const detail = d.getElementById("job-detail");
    assert(
      `M7: picking a job reads its tail and offers to stop it (${detail.querySelector(".job-output").textContent.replace(/\n/g, "\\n")})`,
      called("job_output").length === 1 &&
        lastArgs("job_output").id === "j1" &&
        typeof lastArgs("job_output").max_bytes === "number" &&
        !detail.classList.contains("hidden") &&
        /compiling/.test(detail.querySelector(".job-output").textContent) &&
        !!detail.querySelector(".btn"),
    );
    detail.querySelector(".job-detail-head .btn").click();
    await flush();
    assert(
      `M7: stopping a job calls job_interrupt with that id (${lastArgs("job_interrupt") && lastArgs("job_interrupt").id})`,
      called("job_interrupt").length === 1 && lastArgs("job_interrupt").id === "j1",
    );

    // M6 — context: token meter, compaction, and a spilled output.
    await refreshContext();
    const meterText = d.getElementById("token-meter").textContent;
    const compactText = d.getElementById("compact-status").textContent;
    assert(
      `M6: the token meter shows the estimate and the reconciliation (${meterText.replace(/\s+/g, " ").slice(0, 72)})`,
      called("token_meter").length === 1 &&
        lastArgs("token_meter").session_id === "e2e" &&
        lastArgs("token_meter").provider_tokens === null &&
        /1234 tok/.test(meterText) &&
        /within tolerance/.test(meterText) &&
        /yes/.test(meterText),
    );
    assert(
      `M6: compaction status shows the budget and what may happen (${compactText.replace(/\s+/g, " ").slice(0, 60)})`,
      called("compaction_status").length === 1 && /8192 tok/.test(compactText) && /can compact/.test(compactText),
    );
    const spillRows = d.querySelectorAll("#spill-list .spill-row");
    spillRows[0].click();
    await flush();
    assert(
      `M6: a spilled output lists, then reads back through spill_read (${spillRows.length} row(s), ${lastArgs("spill_read") && lastArgs("spill_read").id})`,
      called("spill_list").length === 1 &&
        spillRows.length === 1 &&
        called("spill_read").length === 1 &&
        lastArgs("spill_read").id === "sp1" &&
        /head of the output/.test(d.getElementById("spill-detail").textContent),
    );
    d.getElementById("ctx-compact").click();
    await flush();
    assert(
      `M6: Compact now calls compact_now and says what it folded (${d.getElementById("ctx-note").textContent})`,
      called("compact_now").length === 1 &&
        lastArgs("compact_now").session_id === "e2e" &&
        /Folded 4 turn/.test(d.getElementById("ctx-note").textContent),
    );
    d.getElementById("ctx-restore").click();
    await flush();
    assert(
      `M6: Restore folded turns calls restore_compacted (${lastArgs("restore_compacted") && lastArgs("restore_compacted").session_id}, note=${d.getElementById("ctx-note").textContent})`,
      called("restore_compacted").length === 1 &&
        lastArgs("restore_compacted").session_id === "e2e" &&
        /Restored 4/.test(d.getElementById("ctx-note").textContent),
    );

    // M4 — rules: the book, its warnings, the audit ring, add and revoke.
    await refreshRules();
    const ruleRows = d.querySelectorAll("#rules-list .rule-row");
    assert(
      `M4: the rule book renders effect, tool and predicate (${ruleRows.length} row(s), arg=${ruleRows[0] && ruleRows[0].querySelector(".rule-arg").textContent})`,
      called("approval_rules_list").length === 1 &&
        ruleRows.length === 1 &&
        !!ruleRows[0].querySelector(".rule-effect.e-allow") &&
        /command prefix "git "/.test(ruleRows[0].querySelector(".rule-arg").textContent),
    );
    assert(
      `M4: a malformed rules file is reported, not swallowed (${d.getElementById("rules-warnings").textContent})`,
      /= "|problem\(s\)/.test(d.getElementById("rules-warnings").textContent) &&
        /problem\(s\) in the rules file/.test(d.getElementById("rules-warnings").textContent),
    );
    assert(
      `M4: the audit ring renders each decision (${d.querySelectorAll("#audit-list .audit-row").length} row(s))`,
      called("approval_audit").length === 1 &&
        lastArgs("approval_audit").limit === 20 &&
        d.querySelectorAll("#audit-list .audit-row").length === 1 &&
        /ask/.test(d.querySelector("#audit-list .audit-row .rule-effect").textContent),
    );
    d.getElementById("rule-tool").value = "bash";
    d.getElementById("rule-scope").value = "session";
    d.getElementById("rule-effect").value = "deny";
    d.getElementById("rule-key").value = "command";
    d.getElementById("rule-match").value = "prefix";
    d.getElementById("rule-value").value = "rm -rf";
    d.getElementById("rule-add").click();
    await flush();
    const e2eAdded = lastArgs("approval_rule_add");
    assert(
      `M4: Add rule sends the whole predicate (${e2eAdded && `${e2eAdded.tool}/${e2eAdded.scope}/${e2eAdded.effect}/${e2eAdded.arg_key}:${e2eAdded.arg_match}=${e2eAdded.arg_value}/session=${e2eAdded.session_id}`})`,
      called("approval_rule_add").length === 1 &&
        e2eAdded.tool === "bash" &&
        e2eAdded.scope === "session" &&
        e2eAdded.effect === "deny" &&
        e2eAdded.arg_key === "command" &&
        e2eAdded.arg_match === "prefix" &&
        e2eAdded.arg_value === "rm -rf" &&
        e2eAdded.workspace === null &&
        e2eAdded.session_id === "e2e" &&
        d.getElementById("rule-tool").value === "",
    );
    const revoker = d.querySelector("#rules-list .rule-row .btn");
    if (revoker) revoker.click();
    await flush();
    assert(
      `M4: Revoke calls approval_rule_revoke with the rule id (${lastArgs("approval_rule_revoke") && lastArgs("approval_rule_revoke").id})`,
      called("approval_rule_revoke").length === 1 && lastArgs("approval_rule_revoke").id === "r1",
    );

    // M11 + M10 + M14 — self-inspection, deterministic replay, bundle export.
    await refreshInspect();
    const grid = d.getElementById("insight-grid").textContent;
    assert(
      `M11: self-inspection reports the live inventory (${grid.replace(/\s+/g, " ").slice(0, 60)})`,
      called("self_inspect").length === 1 &&
        lastArgs("self_inspect").session_id === "e2e" &&
        /tools/.test(grid) &&
        /9/.test(grid) &&
        /active/.test(grid),
    );
    d.getElementById("insight-evaluate").click();
    await flush();
    const scoreBox = d.getElementById("insight-score");
    assert(
      `M11: Evaluate calls the critic and shows grade plus reasons (${scoreBox.textContent.replace(/\s+/g, " ").slice(0, 60)})`,
      called("evaluate_transcript").length === 1 &&
        lastArgs("evaluate_transcript").session_id === "e2e" &&
        !scoreBox.classList.contains("hidden") &&
        /B\+/.test(scoreBox.textContent) &&
        /blank assistant turns/.test(scoreBox.textContent),
    );
    d.getElementById("replay-run").click();
    await flush();
    const replayBox = d.getElementById("replay-report");
    assert(
      `M10: replay renders the run's metrics and entries (${replayBox.textContent.replace(/\s+/g, " ").slice(0, 64)})`,
      called("replay_session").length === 1 &&
        !replayBox.classList.contains("hidden") &&
        /1\.2 s/.test(replayBox.textContent) &&
        /1 tool/.test(replayBox.textContent) &&
        d.querySelectorAll("#replay-report .replay-row").length === 1,
    );
    d.getElementById("bundle-export").click();
    await flush();
    assert(
      `M14: Export bundle reports the archive it wrote (${d.getElementById("bundle-result").textContent})`,
      called("export_bundle").length === 1 &&
        lastArgs("export_bundle").session_id === "e2e" &&
        /bos-bundle\.zip/.test(d.getElementById("bundle-result").textContent) &&
        /4\.0 KiB/.test(d.getElementById("bundle-result").textContent) &&
        /5 entries/.test(d.getElementById("bundle-result").textContent) &&
        !d.getElementById("bundle-result").classList.contains("err"),
    );

    // The whole sweep must have touched every new command exactly as named.
    const wiredCommands = [
      "jobs_list", "job_output", "job_interrupt",
      "token_meter", "compaction_status", "compact_now", "restore_compacted", "spill_list", "spill_read",
      "approval_rules_list", "approval_audit", "approval_rule_add", "approval_rule_revoke",
      "self_inspect", "evaluate_transcript", "replay_session", "export_bundle",
      "skill_activation", "skill_set_active", "skill_inherit",
    ];
    const unreached = wiredCommands.filter((cmd) => called(cmd).length === 0);
    assert(
      `every new command was exercised through a real UI handler (${unreached.length} unreached: ${unreached.join(",")})`,
      unreached.length === 0,
    );

    // A session-scoped panel must refuse politely rather than fire a bad call.
    const before = calls.length;
    state.activeId = "";
    await refreshContext();
    assert(
      `a session-scoped panel asks for a chat instead of calling with an empty id (${d.getElementById("ctx-note").textContent})`,
      calls.length === before && /Open a chat first/.test(d.getElementById("ctx-note").textContent),
    );
    state.activeId = restoreSession;

    // Removing the *last* workspace (rev 115, §24.6). The fixture has three, so
    // this drives the registry down to the state the user was in — one left — and
    // then to none, which is the state the panel had no face for before.
    const wholeRegistry = state.workspaces;
    const wholeActive = state.activeWorkspace;
    const wholeSessions = state.sessions;
    const lastOne = state.workspaces[0];
    state.workspaces = [lastOne];
    state.activeWorkspace = lastOne.id;
    openWorkspaceForm("remove", lastOne.id);
    const lastForm = d.querySelector("#session-list .ws-form");
    assert(
      `the last workspace's remove form states where its chats go instead of offering a choice (${
        lastForm.querySelector(".ws-form-hint") ? lastForm.querySelector(".ws-form-hint").textContent.slice(0, 48) : "no hint"
      })`,
      !!lastForm.querySelector(".ws-form-hint") && !lastForm.querySelector(".ws-del-chats"),
    );
    const beforeLast = calls.length;
    lastForm.querySelector(".ws-form-go").click();
    await flush();
    const lastRemove = lastArgs("remove_workspace");
    assert(
      `and sends deleteSessions for it even though no box was ticked (${JSON.stringify(lastRemove)})`,
      called("remove_workspace").length === 1 &&
        lastRemove.id === lastOne.id &&
        lastRemove.deleteSessions === true,
    );

    // An emptied registry is a state with a face: the invitation is the
    // folder-plus's own gesture, and nothing creates a chat into the void.
    // The chats went with it (the fixture's answer to the removal above), so the
    // sessions go too: a registry with none and sessions still pointing at the
    // removed row is a different state, and one that deliberately still draws
    // them, because §24.6's keep path leaves a chat reachable.
    state.workspaces = [];
    state.sessions = [];
    state.wsEdit = null;
    state.activeId = null;
    renderSidebar();
    const emptyState = d.querySelector("#session-list .ws-empty");
    assert(
      `an emptied registry invites a folder rather than drawing nothing (${
        emptyState ? emptyState.textContent.trim().slice(0, 48) : "nothing drawn"
      })`,
      !!emptyState &&
        !!emptyState.querySelector(".ws-empty-add") &&
        !d.querySelector("#session-list .ws-head .ws-name"),
    );
    const beforePick = called("choose_folder").length;
    emptyState.querySelector(".ws-empty-add").click();
    await flush();
    assert(
      "the invitation asks the platform for a folder",
      called("choose_folder").length === beforePick + 1,
    );

    // The pick above *became* a workspace, so a chat in it is right. The void is
    // the state with no workspace at all, which is what the three gestures that
    // used to create one blindly now refuse.
    state.workspaces = [];
    state.activeWorkspace = "";
    state.activeId = null;
    renderSidebar();
    const beforeVoid = calls.length;
    void newChat();
    await flush();
    assert(
      `no chat is created into the void (${calls.slice(beforeVoid).filter((c) => c.cmd === "create_session").length} create_session calls)`,
      calls.slice(beforeVoid).every((c) => c.cmd !== "create_session"),
    );
    await sendText("hello");
    assert(
      "and the composer refuses to send into it",
      calls.slice(beforeVoid).every((c) => c.cmd !== "send_message"),
    );
    assert(
      `the refusal is said out loud, not swallowed (${
        [...d.querySelectorAll(".toast")]
          .map((t) => t.textContent)
          .filter((t) => /Add a workspace first/.test(t))
          .join(" / ")
          .slice(0, 56)
      })`,
      [...d.querySelectorAll(".toast")].some((t) => /Add a workspace first/.test(t.textContent)),
    );
    state.workspaces = wholeRegistry;
    state.activeWorkspace = wholeActive;
    state.sessions = wholeSessions;

    // Adding a workspace creates a workspace, not a chat (rev 117 — the user's bug:
    // "adding a workspace adds a chat session by default"). The round trips are the
    // observable, because a settled fixture draws the same sidebar either way.
    const addStart = calls.length;
    await addWorkspaceFromPanel();
    await flush();
    const addCmds = calls.slice(addStart).map((c) => c.cmd);
    assert(
      `the folder-plus creates the workspace and no chat with it (${addCmds.join(",")})`,
      addCmds.includes("choose_folder") &&
        addCmds.includes("create_workspace") &&
        !addCmds.includes("create_session"),
    );
    // Nothing is open after it, so the composer is the door: the same create, at the
    // moment something is actually asked.
    const chatStart = calls.length;
    await sendText("hello");
    await flush();
    const chatCmds = calls.slice(chatStart).map((c) => c.cmd);
    assert(
      `and the first message of an empty workspace starts its chat (${chatCmds.join(",")})`,
      chatCmds.includes("create_session") && chatCmds.includes("send_message"),
    );

  } catch (err) {
    assert(`the live-page harness ran to completion (${err})`, false);
  }
  const pre = d.createElement("pre");
  pre.id = "result";
  pre.textContent = JSON.stringify(R);
  d.body.appendChild(pre);
})();
