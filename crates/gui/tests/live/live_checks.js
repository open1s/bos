/* Checks that drive the shipped page against the live backend (live/bridge.js).
 *
 * Where page_checks.js settles fixtures in one microtask pass, this file
 * awaits the real asynchronous behaviors: streams that end, queues that flush,
 * approvals that must be answered, saves that round-trip, failures that roll
 * back. The bridge holds state across commands, so each check can assert the
 * whole path — composer → invoke → store → event → DOM.
 */
(async () => {
  const R = { checks: {}, failures: 0, page: "live" };
  const assert = (desc, cond) => {
    R.checks[desc] = cond ? "PASS" : "FAIL";
    if (!cond) R.failures += 1;
  };
  const d = document;
  const LIVE = window.__LIVE__;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const waitFor = async (fn, what, timeout) => {
    const budget = timeout || 6000;
    const t0 = Date.now();
    while (!fn()) {
      if (Date.now() - t0 > budget) throw new Error(`waitFor: ${what}`);
      await sleep(15);
    }
  };
  const messagesText = () => d.getElementById("messages").textContent;
  const activeRow = () =>
    [...d.querySelectorAll("#session-list .session-row")].find((r) =>
      r.classList.contains("active"));
  const rowByTitle = (title) =>
    [...d.querySelectorAll("#session-list .session-row")].find((r) =>
      (r.querySelector(".title") || {}).textContent === title);
  // Typing like a user does: the value alone does nothing — the button stays
  // disabled and the palette never sees the keystroke — until the input event.
  const type = (text) => {
    const input = d.getElementById("input");
    input.value = text;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  };
  // Headless Chrome under --virtual-time-budget produces its first frames and
  // then starves rAF (measured: a nested rAF never fires), so a repaint the
  // app schedules for the next frame never happens here. Running that frame's
  // work directly is the same renderMessages over the same state — the
  // coalescing itself is what ui_selftest measures, not this harness.
  const repaint = () => {
    if (typeof window.renderMessages === "function") window.renderMessages();
  };

  // Let a handler's promise chain run: the page's commands resolve in
  // microtasks, and an interaction that only schedules work would otherwise be
  // read before it landed.
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

    const openPane = (section) => {
      d.getElementById("open-settings").click();
      [...d.querySelectorAll(".set-tab")]
        .find((t) => t.dataset.section === section)
        .click();
    };
    const pairs = (host) =>
      [...host.querySelectorAll(".meter-key")].map((k) => [
        k.textContent,
        k.nextElementSibling ? k.nextElementSibling.textContent : "",
      ]);
    const meter = (key) => {
      const hit = pairs(d.getElementById("token-meter")).find(([k]) => k === key);
      return hit ? hit[1] : null;
    };

  try {
    /* ---- boot: the seeded workspace arrives as a working app ---------- */
    await waitFor(() => d.querySelectorAll("#session-list .session-row").length >= 2,
      "session list to render");
    assert(
      "boot: the seeded transcript renders user, assistant and a timed tool row",
      /Design the streaming harness/.test(messagesText()) &&
        /Drafting the plan now\./.test(messagesText()) &&
        /read_file/.test(messagesText()) &&
        /42 ms/.test(messagesText()),
    );
    // The tool's output lives behind the collapsed card; opening it is how a
    // user reads what the tool did, so assert that path too.
    const toolHead = d.querySelector("#messages .tool-head");
    if (toolHead) toolHead.click();
    repaint();
    await waitFor(
      () => /read 120 bytes/.test(messagesText()),
      "the expanded tool card to show its output",
    );
    assert(
      "boot: expanding the tool card reveals its output (read 120 bytes)",
      /read 120 bytes/.test(messagesText()),
    );
    const left = d.getElementById("status-left").textContent;
    assert(
      `boot: model and chat count reach the status line (${left.replace(/\s+/g, " ").slice(0, 40)})`,
      /glm-4\.6/.test(left) && /2 chats/.test(left) && /4 msgs/.test(left) && /1 tools/.test(left),
    );
    assert(
      `boot: the live goal reaches its card (objective=${
        d.getElementById("goal-objective").textContent})`,
      !d.getElementById("goal-card").classList.contains("hidden") &&
        d.getElementById("goal-objective").textContent === "ship the streaming harness" &&
        /3 of 12/.test(d.getElementById("goal-rounds").textContent),
    );
    const callsAtBoot = LIVE.calls.length;

    /* ---- send: text crosses the bridge and a turn streams back -------- */
    const input = d.getElementById("input");
    const sendBtn = d.getElementById("send");
    type("hello harness");
    sendBtn.click();
    await waitFor(() => LIVE.called("send_message").length === 1, "send_message to fire");
    assert(
      `send: the composer's text crosses with the active session id (args=${
        JSON.stringify(LIVE.lastArgs("send_message"))})`,
      LIVE.lastArgs("send_message").text === "hello harness" &&
        typeof LIVE.lastArgs("send_message").sessionId === "string" &&
        LIVE.lastArgs("send_message").sessionId.startsWith("live-") &&
        LIVE.lastArgs("send_message").truncateFrom === null,
    );
    await waitFor(() => /hello harness/.test(messagesText()), "user bubble to appear");
    assert(
      `send: the pair appears at once and the button turns into Stop (btn=${
        sendBtn.textContent.trim()})`,
      /hello harness/.test(messagesText()) && sendBtn.classList.contains("stop"),
    );
    await waitFor(() => /Hello, world — from the live harness\./.test(messagesText()),
      "streamed reply to finish");
    await waitFor(() => sendBtn.textContent.trim() === "Send", "the turn to end");
    assert(
      "stream: chunked text lands complete and Stop returns to Send",
      /Hello, world — from the live harness\. \(you said: hello harness\)/.test(messagesText()) &&
        !sendBtn.classList.contains("stop"),
    );
    assert(
      `stream: the usage line reports the provider's tokens (${
        [...d.querySelectorAll("#status-right .usage")].map((u) => u.textContent).join("|")})`,
      /↑ 120 ↓ 45 tok/.test(
        [...d.querySelectorAll("#status-right .usage")].map((u) => u.textContent).join("|")),
    );
    await waitFor(
      () => /Hello, world/.test((d.getElementById("chat-title") || {}).textContent || "") ||
        LIVE.called("load_session").some((c) => c.args.id === LIVE.lastArgs("send_message").sessionId),
      "store reload after finish",
    );

    /* ---- queue: a mid-stream follow-up waits, then flushes FIFO --------
       The send button means Stop while a turn streams, so queueing is the
       composer's Enter path — the same gesture a user makes. */
    type("slow please");
    sendBtn.click();
    await waitFor(() => sendBtn.classList.contains("stop"), "the slow turn to start");
    type("queued follow-up");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await waitFor(
      () => !d.getElementById("queue-list").classList.contains("hidden"),
      "the queue row to render",
    );
    assert(
      `queue: a mid-stream follow-up waits in the queue (${d
        .getElementById("queue-list")
        .textContent.trim().slice(0, 30)}) and never reaches the provider (sends=${
        LIVE.called("send_message").length})`,
      /queued follow-up/.test(d.getElementById("queue-list").textContent) &&
        LIVE.called("send_message").length === 2,
    );
    await waitFor(() => LIVE.called("send_message").length === 3, "the queued turn to auto-send");
    await waitFor(
      () => !sendBtn.classList.contains("stop") &&
        /you said: queued follow-up/.test(messagesText()),
      "the queued turn to finish",
    );
    assert(
      "queue: the follow-up flushes on finish and its reply lands",
      /you said: queued follow-up/.test(messagesText()) &&
        d.getElementById("queue-list").classList.contains("hidden"),
    );

    /* ---- stop: cancelling mid-stream settles the turn ------------------ */
    type("slow again");
    sendBtn.click();
    await waitFor(() => sendBtn.classList.contains("stop"), "the second slow turn to start");
    sendBtn.click(); // the button is now Stop
    await waitFor(() => LIVE.called("stop_streaming").length === 1, "stop_streaming to fire");
    assert(
      `stop: Stop answers with the chat's id (args=${JSON.stringify(LIVE.lastArgs("stop_streaming"))})`,
      LIVE.lastArgs("stop_streaming").sessionId === LIVE.lastArgs("send_message").sessionId,
    );
    await waitFor(
      () => !sendBtn.classList.contains("stop") && /chunk/.test(messagesText()),
      "the cancelled turn to settle",
    );
    assert(
      "stop: the partial text stays on screen and the composer is free again",
      /chunk 0/.test(messagesText()) && sendBtn.textContent.trim() === "Send",
    );
    await waitFor(
      () => !LIVE.lastArgs("send_message") ||
        sendBtn.textContent.trim() === "Send",
      "idle before the approval scenario",
    );

    /* ---- approval: a write tool must be answered before it runs -------- */
    const approvalModal = d.getElementById("approval-modal");
    type("write notes/a.txt now");
    sendBtn.click();
    await waitFor(() => !approvalModal.classList.contains("hidden"), "the approval dialog to open");
    const dialogText = approvalModal.textContent;
    assert(
      `approval: the dialog names the tool and previews the write as a diff (text=${dialogText
        .replace(/\s+/g, " ")
        .slice(0, 60)})`,
      /write_file/.test(dialogText) &&
        /live harness was here/.test(
          [...approvalModal.querySelectorAll(".d-add")].map((e) => e.textContent).join("\n"),
        ),
    );
    d.getElementById("approval-allow").click();
    await waitFor(() => approvalModal.classList.contains("hidden"), "the dialog to close");
    assert(
      `approval: Allow answers the request (args=${JSON.stringify(LIVE.lastArgs("respond_approval"))})`,
      LIVE.lastArgs("respond_approval").approved === true &&
        typeof LIVE.lastArgs("respond_approval").id === "string",
    );
    await waitFor(() => sendBtn.textContent.trim() === "Send", "the approval turn to finish");
    // The tool's output hides behind its collapsed card, like any transcript
    // tool row: open it, then read what the tool did.
    const streamedTool = [...d.querySelectorAll("#messages .tool-head")].pop();
    if (streamedTool) streamedTool.click();
    repaint();
    await waitFor(() => /wrote 2 bytes/.test(messagesText()), "the tool result to land");
    assert(
      `approval: the tool result attaches to the turn and the counter counts it (${
        [...d.querySelectorAll("#status-right .tools")].map((t) => t.textContent).join("|")})`,
      /wrote 2 bytes/.test(messagesText()) && /12 ms/.test(messagesText()) &&
        /⚙ 1 tool call/.test(
          [...d.querySelectorAll("#status-right .tools")].map((t) => t.textContent).join("|")),
    );

    type("write a second file");
    sendBtn.click();
    await waitFor(() => !approvalModal.classList.contains("hidden"), "the second dialog to open");
    d.getElementById("approval-deny").click();
    await waitFor(() => approvalModal.classList.contains("hidden"), "the second dialog to close");
    await waitFor(() => sendBtn.textContent.trim() === "Send", "the denied turn to settle");
    const deniedTool = [...d.querySelectorAll("#messages .tool-head")].pop();
    if (deniedTool) deniedTool.click();
    repaint();
    await waitFor(() => /denied by the user/.test(messagesText()), "the denial to land");
    assert(
      `approval: Deny refuses the tool and the turn still settles (last=${
        JSON.stringify(LIVE.lastArgs("respond_approval"))})`,
      LIVE.lastArgs("respond_approval").approved === false &&
        /denied by the user/.test(messagesText()) &&
        /Understood — I left the file alone\./.test(messagesText()),
    );
    await waitFor(() => sendBtn.textContent.trim() === "Send", "idle after approvals");

    /* ---- M4: a scoped rule settles the next call, and the ring says who did */
    openPane("rules-section");
    await waitFor(() => LIVE.called("approval_rules_list").length >= 1, "the rule book to be read");
    const ruleRows = () => [...d.querySelectorAll("#rules-list .rule-row")];
    const auditRow = (i) => d.querySelectorAll("#audit-list .audit-row")[i];
    const auditText = () => d.getElementById("audit-list").textContent.replace(/\s+/g, " ").trim();
    assert(
      `M4: an empty rule book explains itself ("${d
        .getElementById("rules-list")
        .textContent.trim()
        .slice(0, 46)}")`,
      ruleRows().length === 0 &&
        /No scoped rules yet/.test(d.getElementById("rules-list").textContent),
    );

    d.getElementById("rule-tool").value = "write_*";
    d.getElementById("rule-effect").value = "allow";
    d.getElementById("rule-add").click();
    await waitFor(() => ruleRows().length === 1, "the rule to be listed");
    const added = LIVE.lastArgs("approval_rule_add");
    const ruleRow = d.querySelector("#rules-list .rule-row");
    assert(
      `M4: adding a scoped rule lists it (tool=${ruleRow.querySelector(".rule-tool").textContent}, effect=${
        ruleRow.querySelector(".rule-effect").textContent}, scope=${
        ruleRow.querySelector(".rule-scope").textContent}, note="${
        d.getElementById("rules-warnings").textContent.trim()}")`,
      added.tool === "write_*" &&
        added.effect === "allow" &&
        added.scope === "global" &&
        added.arg_key === null &&
        added.session_id === null &&
        ruleRow.querySelector(".rule-tool").textContent === "write_*" &&
        ruleRow.querySelector(".rule-effect").classList.contains("e-allow") &&
        ruleRow.querySelector(".rule-scope").textContent === "global" &&
        ruleRow.querySelector(".rule-arg").textContent === "any args" &&
        d.getElementById("rule-tool").value === "" &&
        /Added: write_\* \(global\)\./.test(d.getElementById("rules-warnings").textContent),
    );

    // Counting occurrences, not matching once: the Allow turn above already put
    // the same words in this transcript, so a bare match would pass before the
    // rule settled this turn.
    const countOf = (needle) => messagesText().split(needle).length - 1;
    const doneBefore = countOf("Done — notes/a.txt is written.");
    const answeredBefore = LIVE.called("respond_approval").length;
    const wroteBefore = countOf("wrote 2 bytes");
    type("write notes/a.txt now");
    sendBtn.click();
    await waitFor(
      () => countOf("Done — notes/a.txt is written.") > doneBefore,
      "the rule-allowed turn",
    );
    await waitFor(() => sendBtn.textContent.trim() === "Send", "the allowed turn to settle");
    const allowedTool = [...d.querySelectorAll("#messages .tool-head")].pop();
    if (allowedTool) allowedTool.click();
    repaint();
    await waitFor(() => countOf("wrote 2 bytes") > wroteBefore, "the tool result");
    assert(
      `M4: a matching allow rule settles the call, so no prompt is raised (dialog=${
        d.getElementById("approval-modal").classList.contains("hidden") ? "hidden" : "OPEN"}, answered=${
        LIVE.called("respond_approval").length - answeredBefore})`,
      d.getElementById("approval-modal").classList.contains("hidden") &&
        LIVE.called("respond_approval").length === answeredBefore &&
        countOf("wrote 2 bytes") > wroteBefore,
    );

    d.getElementById("rules-refresh").click();
    await waitFor(() => /allow/.test(auditText()), "the ring to record the decision");
    assert(
      `M4: the ring names the rule that decided the call (newest="${
        auditRow(0).textContent.replace(/\s+/g, " ").slice(0, 42)}", cap=${
        LIVE.lastArgs("approval_audit").limit})`,
      LIVE.lastArgs("approval_audit").limit === 20 &&
        auditRow(0).querySelector(".rule-effect").textContent === "allow" &&
        auditRow(0).querySelector(".rule-tool").textContent === "write_file" &&
        /rule-1/.test(auditRow(0).textContent),
    );

    d.querySelector("#rules-list .rule-row button").click();
    await waitFor(() => ruleRows().length === 0, "the rule to be revoked");
    assert(
      `M4: Revoke removes the rule and the book says so again (args=${
        JSON.stringify(LIVE.lastArgs("approval_rule_revoke"))}, rows=${ruleRows().length})`,
      LIVE.lastArgs("approval_rule_revoke").id === "rule-1" &&
        ruleRows().length === 0 &&
        /No scoped rules yet/.test(d.getElementById("rules-list").textContent),
    );

    const askedBefore = LIVE.called("respond_approval").length;
    type("write notes/a.txt again");
    sendBtn.click();
    await waitFor(
      () => !d.getElementById("approval-modal").classList.contains("hidden"),
      "the prompt to come back",
    );
    const promptedAgain = !d.getElementById("approval-modal").classList.contains("hidden");
    d.getElementById("approval-deny").click();
    await waitFor(() => sendBtn.textContent.trim() === "Send", "the denied turn to settle");
    d.getElementById("rules-refresh").click();
    await waitFor(() => auditRow(0) !== undefined && /ask/.test(auditRow(0).textContent), "the ask in the ring");
    assert(
      `M4: revoking the rule brings the prompt back, and the ring shows the call went back to asking (newest="${
        auditRow(0).textContent.replace(/\s+/g, " ").slice(0, 42)}")`,
      promptedAgain &&
        (auditRow(0).querySelector(".rule-effect") || {}).textContent === "ask" &&
        !auditRow(0).querySelector(".rule-scope") &&
        auditRow(0).querySelector(".rule-tool").textContent === "write_file" &&
        auditRow(1) !== undefined &&
        (auditRow(1).querySelector(".rule-effect") || {}).textContent === "allow" &&
        LIVE.called("respond_approval").length === askedBefore + 1,
    );

    /* ---- error: a failed send rolls the pair back ---------------------- */
    LIVE.failNext("send_message", "provider offline (injected)");
    type("this send will fail");
    sendBtn.click();
    await waitFor(
      () => [...d.querySelectorAll("#status-right .err")].some((e) =>
        /provider offline \(injected\)/.test(e.textContent)),
      "the error to reach the status line",
    );
    assert(
      `error: the failed send reports itself (err=${
        [...d.querySelectorAll("#status-right .err")].map((e) => e.textContent).join("|")})`,
      /provider offline \(injected\)/.test(
        [...d.querySelectorAll("#status-right .err")].map((e) => e.textContent).join("|")),
    );
    await sleep(300);
    assert(
      `error: the pair is rolled back and the composer keeps the text (input=${
        input.value.slice(0, 24)})`,
      !/this send will fail/.test(messagesText()) && input.value === "this send will fail",
    );
    type("hello again");
    sendBtn.click();
    await waitFor(
      () => /you said: hello again/.test(messagesText()) && sendBtn.textContent.trim() === "Send",
      "the retry to go through",
    );
    assert("error: the next send after the failure goes through", true);

    /* ---- terminal: attach, replay, write, stream back, kill ---------- */
    const termOut = () => d.getElementById("run-output").textContent;
    const termChip = () => d.getElementById("run-chip").textContent.trim();
    // The run panel paints on its own rAF, which this runner starves exactly as
    // it starves the transcript's, so its frame's work is run on every poll.
    const paintTerm = () => {
      if (typeof window.renderRunPanel === "function") window.renderRunPanel();
    };
    const waitTermOut = (re, what) =>
      waitFor(() => {
        paintTerm();
        return re.test(termOut());
      }, what);
    const ptyId = "bos-pty-" + (state.workspaceRoot || "default");
    d.getElementById("term-open").click();
    await waitFor(() => LIVE.called("pty_start").length === 1, "pty_start to fire");
    await waitTermOut(/bos harness shell/, "the tail to replay");
    assert(
      `terminal: the Shell button attaches to this workspace's shell (args=${
        JSON.stringify(LIVE.lastArgs("pty_start"))}, chip=${termChip()})`,
      LIVE.lastArgs("pty_start").sessionId === ptyId &&
        LIVE.lastArgs("pty_start").shell === null &&
        termChip() === "shell" &&
        /bos harness shell/.test(termOut()),
    );

    /* A command goes to the shell and is not echoed locally — the PTY echoes. */
    const runInput = d.getElementById("run-input");
    const echoesBefore = termOut().split("echo hi").length - 1;
    runInput.value = "echo hi";
    d.getElementById("run-form").dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await waitFor(() => LIVE.called("pty_write").length === 1, "pty_write to fire");
    await waitTermOut(/echo hi: live harness/, "the shell's answer to stream back");
    assert(
      `terminal: a submitted line crosses to the PTY and its answer streams back (data=${
        JSON.stringify(LIVE.lastArgs("pty_write").data)}, it appears ${
        termOut().split("echo hi").length - 1 - echoesBefore}x)`,
      LIVE.lastArgs("pty_write").data === "echo hi\n" &&
        LIVE.lastArgs("pty_write").sessionId === ptyId &&
        runInput.value === "" &&
        termOut().split("echo hi").length - 1 - echoesBefore === 2,
    );

    /* Re-attach must adopt the live shell and replay its tail, not start over. */
    const tailBefore = termOut();
    d.getElementById("term-open").click();
    await waitFor(() => LIVE.called("pty_start").length === 2, "the second pty_start");
    await waitFor(() => {
      paintTerm();
      return termChip() === "attached";
    }, "the re-attached chip");
    assert(
      `terminal: a second attach adopts the live shell and replays its tail (chip=${
        termChip()}, ${termOut().length} chars shown)`,
      termChip() === "attached" &&
        /echo hi: live harness/.test(termOut()) &&
        termOut().length >= tailBefore.length,
    );

    /* The kill path is narrated by the exit event, not by the click. */
    d.getElementById("term-kill").click();
    await waitFor(() => LIVE.called("pty_kill").length === 1, "pty_kill to fire");
    await waitTermOut(/session ended · killed/, "the exit note");
    assert(
      `terminal: Kill ends the shell and the exit event narrates it (args=${
        JSON.stringify(LIVE.lastArgs("pty_kill"))}, chip=${termChip()})`,
      LIVE.lastArgs("pty_kill").sessionId === ptyId &&
        termChip() === "killed" &&
        /session ended · killed/.test(termOut()),
    );

    /* A shell that refuses to start has to say so, not look attached. */
    LIVE.failNext("pty_start", "no shell here");
    d.getElementById("term-open").click();
    await waitTermOut(/shell unavailable:.*no shell here/, "the refusal note");
    assert(
      `terminal: a refused shell reports itself instead of looking attached (chip=${
        termChip()}, note=${
        (termOut().match(/\[shell unavailable[^\]]*\]/) || [""])[0].slice(0, 44)})`,
      termChip() === "no shell" &&
        /\[shell unavailable:.*no shell here\]/.test(termOut()),
    );

    /* ---- M11: the Insight pane — the agent reading itself ------------ */
    openPane("insight-section");
    await waitFor(() => LIVE.called("self_inspect").length === 1, "the composition to be read");
    const insightRow = (key) => {
      const hit = pairs(d.getElementById("insight-grid")).find(([k]) => k === key);
      return hit ? hit[1] : null;
    };
    await waitFor(() => insightRow("model") === "harness-model", "the inventory to render");
    assert(
      `M11: the Insight pane reads the agent's own composition (tools=${
        insightRow("tools")}, async=${
        insightRow("async tools")}, skills=${
        insightRow("skills")}, memories=${
        insightRow("memories")}, model=${
        insightRow("model")}, workspace=${
        insightRow("workspace")}, bash=${
        insightRow("bash")}, files=${
        insightRow("file tools")}, skillsDir=${insightRow("skills dir")})`,
      LIVE.lastArgs("self_inspect").session_id === state.activeId &&
        insightRow("tools") === "11" &&
        insightRow("async tools") === "4" &&
        insightRow("skills") === "2" &&
        insightRow("plugins") === "1" &&
        insightRow("hook events") === "5" &&
        insightRow("MCP servers") === "1" &&
        insightRow("MCP tools") === "3" &&
        insightRow("memories") === "1" &&
        insightRow("model") === "harness-model" &&
        insightRow("workspace") === "/tmp/live" &&
        insightRow("bash") === "on" &&
        insightRow("file tools") === "on" &&
        insightRow("skills dir") === "/tmp/live/skills",
    );

    /* ---- M8: the MCP per-tool policy --------------------------------- */
    // Run after the inventory above, because it opens that pane again: the
    // self-inspection count is asserted exactly there, and a second visit is
    // this section's own evidence, not a stray call.
    openPane("set-mcp");
    await waitFor(() => LIVE.called("mcp_policy_status").length >= 1, "the policy to be read");
    const polHost = () => d.getElementById("pol-servers");
    const polText = () => polHost().textContent.replace(/\s+/g, " ");
    const polNote = () => d.getElementById("pol-msg").textContent;
    const polRuleText = () => d.getElementById("pol-rules").textContent.replace(/\s+/g, " ");
    const polRow = (tool) =>
      [...polHost().querySelectorAll(".pol-row")].find(
        (row) => row.querySelector(".pol-name").textContent === tool,
      );
    const polButton = (tool, effect) =>
      polRow(tool).querySelector(`[aria-label="${effect} filesystem/${tool}"]`);
    await waitFor(
      () => /filesystem/.test(polText()) && /list/.test(polText()),
      "the connected server's tools to be listed",
    );
    assert(
      `M8: the policy pane lists every advertised tool with the rule that decided it (${
        polText().slice(0, 74)})`,
      (polText().match(/follows the default/g) || []).length === 3 &&
        /3 of 3 tools attached/.test(polNote()) &&
        /0 denied · 0 ask · 0 of 200 rules · default allow/.test(polNote()) &&
        /No rule yet: every tool follows the default\./.test(polRuleText()),
    );

    polButton("write", "Deny").click();
    await waitFor(() => /2 of 3 tools attached/.test(polNote()), "the denial to reach the pane");
    const badge = [...d.querySelectorAll("#mcp-list .mcp-row")]
      .map((row) => row.textContent.replace(/\s+/g, " "))
      .join("|");
    openPane("insight-section");
    await waitFor(
      () => insightRow("MCP tools") === "2",
      "the inventory to follow the policy",
    );
    assert(
      `M8: denying one tool reaches the row, the server badge and the inventory (badge=${
        badge.slice(0, 44)}, inventory=${insightRow("MCP tools")})`,
      /by rule filesystem\/write/.test(polText()) &&
        /1 denied/.test(polNote()) &&
        /filesystem\/write/.test(polRuleText()) &&
        /2 of 3 tools · 1 denied/.test(badge) &&
        insightRow("MCP tools") === "2",
    );

    openPane("set-mcp");
    polButton("read", "Ask").click();
    await waitFor(() => /1 ask/.test(polNote()), "the ask to be recorded");
    assert(
      `M8: an ask keeps the tool, records who asked, and leaves the denial alone (${
        polNote().slice(0, 58)})`,
      /2 of 3 tools attached · 1 denied · 1 ask · 2 of 200 rules · default allow/.test(polNote()) &&
        /filesystem\/read/.test(polRuleText()) &&
        /filesystem\/write/.test(polRuleText()) &&
        /Ask/.test(polRuleText()),
    );

    d.querySelector("#pol-rules .pol-clear").click();
    await waitFor(() => /0 denied/.test(polNote()), "the rule to be cleared");
    const defaultSelect = d.getElementById("pol-default");
    defaultSelect.value = "deny";
    defaultSelect.dispatchEvent(new d.defaultView.Event("change"));
    await waitFor(() => /default deny/.test(polNote()), "the default to change");
    openPane("insight-section");
    await waitFor(() => insightRow("MCP tools") === "1", "the inventory to follow the default");
    assert(
      `M8: clearing a rule returns the tool, and a deny default removes every unasked tool (${
        polNote().slice(0, 58)})`,
      /1 of 3 tools attached · 2 denied · 1 ask · 1 of 200 rules · default deny/.test(polNote()) &&
        /by rule filesystem\/read/.test(polRow("read").textContent) &&
        /follows the default/.test(polRow("write").textContent) &&
        /follows the default/.test(polRow("list").textContent) &&
        insightRow("MCP tools") === "1" &&
        LIVE.lastArgs("mcp_policy_default").effect === "deny",
    );

    /* ---- M8: authoring a skill --------------------------------------- */
    const skillHost = () => d.getElementById("skill-list");
    const skillNote = () => d.getElementById("skill-note").textContent;
    const skillRow = (name) =>
      [...skillHost().querySelectorAll(".skill-row")].find(
        (row) => (row.querySelector(".skill-name") || {}).textContent === name,
      );
    const skillText = () => skillHost().textContent.replace(/\s+/g, " ");
    d.getElementById("set-tab-cap-list").click();
    await waitFor(() => !!skillRow("code-review"), "the skills the directory already holds");
    assert(
      `M8: the pane lists what the loader would hand the agent (${skillText().slice(0, 60)})`,
      skillRow("code-review") !== undefined &&
        /Review a diff/.test(skillRow("code-review").textContent) &&
        /1 skill · \/home\/e2e\/\.bos\/skills/.test(skillNote()) &&
        /34 chars of instruction/.test(skillRow("code-review").textContent) &&
        LIVE.called("skill_list").length > 0,
    );

    skillRow("code-review").querySelector(".skill-use").click();
    await settle();
    assert(
      `M8: Use is the same gesture the palette offers (${d.getElementById("input").value})`,
      d.getElementById("input").value === "Use the code-review skill: ",
    );

    const authorSkill = async (name, description, body, replace) => {
      d.getElementById("skill-name").value = name;
      d.getElementById("skill-desc").value = description;
      d.getElementById("skill-body").value = body;
      d.getElementById("skill-overwrite").checked = !!replace;
      d.getElementById("skill-save").click();
      await settle();
    };
    await authorSkill("pr-review", "Review a pull request", "Read the diff, then list findings.", false);
    await waitFor(() => !!skillRow("pr-review"), "the saved skill to appear");
    assert(
      `M8: a saved skill appears in the pane, in the capability list and on disk (${
        skillText().slice(0, 72)})`,
      LIVE.lastArgs("skill_author").name === "pr-review" &&
        LIVE.lastArgs("skill_author").overwrite === false &&
        /pr-review/.test(d.getElementById("cap-items").textContent) &&
        /2 skills/.test(skillNote()) &&
        /saved pr-review/.test(skillNote()) &&
        d.getElementById("skill-name").value === "",
    );

    await authorSkill("pr-review", "Different", "Different body.", false);
    await waitFor(() => /already exists/.test(skillNote()), "the refusal to name the file");
    assert(
      `M8: a second save is refused by name until Replace is ticked (${skillNote().slice(0, 72)})`,
      /pr-review\/SKILL.md already exists/.test(skillNote()) &&
        /Review a pull request/.test(skillRow("pr-review").textContent) &&
        /34 chars of instruction/.test(skillRow("pr-review").textContent),
    );

    await authorSkill("pr-review", "Different", "Different body.", true);
    await waitFor(
      () => /15 chars of instruction/.test(skillRow("pr-review").textContent),
      "the replaced body to land",
    );
    assert(
      `M8: Replace overwrites in place instead of adding a second skill (${
        skillRow("pr-review").textContent.slice(0, 56)})`,
      LIVE.lastArgs("skill_author").overwrite === true &&
        /2 skills/.test(skillNote()) &&
        /15 chars of instruction/.test(skillRow("pr-review").textContent) &&
        /Different/.test(skillRow("pr-review").textContent),
    );

    /* ---- M8: a chat's own skills ---- */
    const skillToggle = (name) => (skillRow(name) || {}).querySelector
      ? skillRow(name).querySelector(".skill-toggle")
      : null;
    const scopeLine = () => d.getElementById("skill-scope").textContent;
    const inheritBtn = () => d.getElementById("skill-inherit");
    const chatA = state.activeId;
    await waitFor(() => !!skillToggle("code-review"), "the per-chat switch to be drawn");
    assert(
      `M8: each row carries this chat's switch, on where the chat carries the skill (${
        skillToggle("code-review").getAttribute("aria-pressed")}/${skillToggle("pr-review").getAttribute("aria-pressed")}, "${scopeLine()}")`,
      skillToggle("code-review").getAttribute("aria-pressed") === "true" &&
        skillToggle("pr-review").getAttribute("aria-pressed") === "true" &&
        inheritBtn().hidden === true &&
        /follows the workspace/.test(scopeLine()),
    );

    skillToggle("pr-review").click();
    await waitFor(
      () => skillToggle("pr-review").getAttribute("aria-pressed") === "false",
      "the switch to redraw the row",
    );
    assert(
      `M8: switching a skill off in this chat reaches the pane and the chat's own record (${
        LIVE.lastArgs("skill_set_active").name}/${
        LIVE.lastArgs("skill_set_active").active}/${
        LIVE.lastArgs("skill_set_active").session === chatA}, "${scopeLine()}")`,
      LIVE.lastArgs("skill_set_active").session === chatA &&
        LIVE.lastArgs("skill_set_active").name === "pr-review" &&
        LIVE.lastArgs("skill_set_active").active === false &&
        skillToggle("code-review").getAttribute("aria-pressed") === "true" &&
        inheritBtn().hidden === false &&
        /1 of 2 skills/.test(scopeLine()),
    );

    await loadCapabilities();
    await waitFor(
      () => !/pr-review/.test(d.getElementById("cap-items").textContent),
      "the inventory to follow the chat",
    );
    assert(
      `M8: the inventory the palette reads describes this chat's skills (${
        d.getElementById("cap-items").textContent.replace(/\s+/g, " ").slice(0, 64)})`,
      /code-review/.test(d.getElementById("cap-items").textContent) &&
        !/pr-review/.test(d.getElementById("cap-items").textContent) &&
        LIVE.lastArgs("list_capabilities").session === chatA,
    );

    // A chat starts from the keyboard now that the sidebar's prominent action
    // adds a workspace (rev 111); this is the shortcut the workspace row's "+"
    // advertises, so it is the gesture a user without the button would use.
    d.dispatchEvent(new KeyboardEvent("keydown", { key: "n", metaKey: true }));
    await waitFor(
      () => state.activeId !== chatA && !!skillToggle("pr-review"),
      "a new chat to open",
    );
    await waitFor(
      () => skillToggle("pr-review").getAttribute("aria-pressed") === "true",
      "the new chat to inherit the workspace",
    );
    assert(
      `M8: a chat nobody switched anything off in carries every skill (${scopeLine()})`,
      skillToggle("pr-review").getAttribute("aria-pressed") === "true" &&
        skillToggle("code-review").getAttribute("aria-pressed") === "true" &&
        inheritBtn().hidden === true &&
        /follows the workspace/.test(scopeLine()),
    );

    [...d.querySelectorAll("#session-list .session-row")]
      .find((row) => row.dataset.id === chatA)
      .click();
    await waitFor(
      () =>
        state.activeId === chatA &&
        skillToggle("pr-review").getAttribute("aria-pressed") === "false",
      "the first chat's own list to come back",
    );
    assert(
      `M8: the list belongs to the chat that made it (${scopeLine()}, inherit=${
        inheritBtn().hidden === false})`,
      inheritBtn().hidden === false && /1 of 2 skills/.test(scopeLine()),
    );

    inheritBtn().click();
    await waitFor(
      () => skillToggle("pr-review").getAttribute("aria-pressed") === "true",
      "the chat to follow the workspace again",
    );
    assert(
      `M8: following the workspace again is one call and puts every skill back (${
        LIVE.lastArgs("skill_inherit").session === chatA}, "${scopeLine()}")`,
      LIVE.lastArgs("skill_inherit").session === chatA &&
        skillToggle("pr-review").getAttribute("aria-pressed") === "true" &&
        inheritBtn().hidden === true &&
        /follows the workspace/.test(scopeLine()),
    );

    /* ---- M8: what the MCP servers publish ---- */
    openPane("set-mcp");
    await settle();
    const catHeads = () =>
      [...d.querySelectorAll("#mcp-catalog .cat-head")].map((head) => head.textContent);
    const catRows = () => [...d.querySelectorAll("#mcp-catalog .cat-row")];
    const readMsg = () => d.getElementById("mcp-read-msg").textContent;
    const readOut = () => d.getElementById("mcp-read-out");
    assert(
      `the MCP pane lists what the server publishes (${catHeads().join(" | ")} rows=${catRows()
        .map((row) => row.textContent)
        .join("|")})`,
      catHeads().length === 1 &&
        /filesystem/.test(catHeads()[0]) &&
        /2 resources · 1 prompt/.test(catHeads()[0]) &&
        catRows().length === 3 &&
        /schema/.test(catRows()[0].textContent) &&
        /application\/json/.test(catRows()[0].textContent) &&
        /review/.test(catRows()[2].textContent) &&
        /2 arguments · 1 required/.test(catRows()[2].textContent),
      catRows().map((row) => row.textContent).join(" | "),
    );
    catRows()[0].querySelector(".cat-use").click();
    await settle();
    assert(
      `a resource row reads it into the pane (${readMsg()})`,
      readOut().textContent === '{"type":"object"}' &&
        !readOut().hidden &&
        /filesystem\/file:\/\/\/repo\/schema.json · 17 chars · application\/json/.test(readMsg()) &&
        LIVE.lastArgs("mcp_read_resource").server === "filesystem" &&
        LIVE.lastArgs("mcp_read_resource").uri === "file:///repo/schema.json",
      JSON.stringify(LIVE.lastArgs("mcp_read_resource")),
    );
    d.getElementById("mcp-uri").value = "file:///repo/big.log";
    d.getElementById("mcp-read-btn").click();
    await settle();
    assert(
      `a body longer than the pane may draw is cut, and says so (${readMsg()})`,
      readOut().textContent.length === 20000 &&
        /20000 of 30000 chars · text\/plain · truncated/.test(readMsg()),
      `${readOut().textContent.length} chars drawn`,
    );
    d.getElementById("mcp-uri").value = "file:///nope";
    d.getElementById("mcp-read-btn").click();
    await settle();
    assert(
      `a failed read is reported and leaves no stale text (${readMsg()})`,
      /resources\/read: no resource at file:\/\/\/nope/.test(readMsg()) &&
        d.getElementById("mcp-read-msg").classList.contains("err") &&
        readOut().hidden &&
        readOut().textContent === "",
      `out=${readOut().textContent.length} hidden=${readOut().hidden}`,
    );
    d.getElementById("mcp-uri").value = "file:///repo/empty.bin";
    d.getElementById("mcp-read-btn").click();
    await settle();
    assert(
      `a resource with no text says so instead of drawing nothing (${readMsg()})`,
      /no text \(application\/octet-stream\)/.test(readMsg()) &&
        readOut().hidden &&
        !d.getElementById("mcp-read-msg").classList.contains("err"),
      readMsg(),
    );

    const goalCardState = d.getElementById("goal-state").textContent;
    const goalCardRounds = d.getElementById("goal-rounds").textContent;
    assert(
      `M11: self-inspection and the Goal card agree about the active goal (insight="${
        insightRow("goal")}", card="${goalCardState}", ${goalCardRounds})`,
      goalCardState === "active" &&
        goalCardRounds === "3 of 12 rounds" &&
        insightRow("goal") === "active · 3/12",
    );

    d.getElementById("insight-evaluate").click();
    await waitFor(() => LIVE.called("evaluate_transcript").length === 1, "the score to be asked for");
    await waitFor(
      () => /·/.test((d.querySelector("#insight-score .score-head") || {}).textContent || ""),
      "the score card",
    );
    const scoreHead = d.querySelector("#insight-score .score-head").textContent;
    const [grade, score] = scoreHead.split(" · ");
    const reasons = [...d.querySelectorAll("#insight-score .score-reasons li")].map(
      (li) => li.textContent,
    );
    assert(
      `M11: Evaluate this chat scores the transcript and explains itself (${scoreHead}, reasons=${
        reasons.length}, "${reasons[0]}")`,
      LIVE.lastArgs("evaluate_transcript").session_id === state.activeId &&
        (grade === "excellent") === Number(score) >= 90 &&
        Number(score) >= 0 &&
        reasons.length >= 1 &&
        reasons.every((r) => r.trim().length > 0) &&
        /blank|failed tool|No problems/.test(reasons.join(" ")),
    );

    d.getElementById("replay-run").click();
    await waitFor(() => LIVE.called("replay_session").length === 1, "the replay to be built");
    await waitFor(
      () => d.querySelectorAll("#replay-report .replay-row").length > 0,
      "the replay entries",
    );
    const rows = [...d.querySelectorAll("#replay-report .replay-row")];
    const replayHead = d.querySelector("#replay-report .score-head").textContent;
    const replayText = d.getElementById("replay-report").textContent;
    assert(
      `M10: deterministic replay reproduces the transcript's shape (head="${replayHead}", rows=${
        rows.length}, first="${rows[0].textContent.slice(0, 34)}", turns=${
        (pairs(d.querySelector("#replay-report .meter-grid")).find(([k]) => k === "turns") || [])[1]})`,
      LIVE.lastArgs("replay_session").session_id === state.activeId &&
        replayHead.startsWith("Replay of ") &&
        rows.length <= 20 &&
        rows[0].textContent.startsWith("0. user") &&
        /turns/.test(replayText) &&
        /tool calls/.test(replayText) &&
        /estimated/.test(replayText),
    );

    d.getElementById("bundle-export").click();
    await waitFor(() => LIVE.called("export_bundle").length === 1, "the bundle to be written");
    await waitFor(
      () => /Wrote /.test(d.getElementById("bundle-result").textContent),
      "the bundle report",
    );
    const bundleNote = d.getElementById("bundle-result").textContent.trim();
    assert(
      `M5: Export bundle reports what it wrote (${bundleNote})`,
      LIVE.lastArgs("export_bundle").session_id === state.activeId &&
        /^Wrote \/tmp\/live\/bundles\/[^ ]+\.zip — [\d.]+ (B|KiB|MiB), \d+ entr(y|ies), schema v1\.$/.test(
          bundleNote,
        ) &&
        !d.getElementById("bundle-result").classList.contains("err"),
    );

    /* ---- M6: the Context pane against a backend that holds state ----- */
    const compactRow = (key) => {
      const hit = pairs(d.getElementById("compact-status")).find(([k]) => k === key);
      return hit ? hit[1] : null;
    };
    openPane("context-section");
    await waitFor(() => LIVE.called("token_meter").length === 1, "the meter to be read");
    await waitFor(() => LIVE.called("compaction_status").length === 1, "the split to be read");
    await waitFor(() => meter("estimated") === "90 tok", "the estimate to render");
    assert(
      `M6: the Context pane reads the meter and the compaction split (messages=${
        meter("messages")}, estimated=${meter("estimated")}, provider=${meter("provider")}, live=${
        compactRow("live")}, folded=${compactRow("folded")}, budget=${compactRow("budget")})`,
      LIVE.lastArgs("token_meter").session_id === state.activeId &&
        LIVE.lastArgs("token_meter").provider_tokens === null &&
        meter("messages") === "4" &&
        meter("estimated") === "90 tok" &&
        meter("folded") === "30 tok" &&
        meter("provider") === "—" &&
        compactRow("live") === "3" &&
        compactRow("folded") === "1" &&
        compactRow("budget") === "8192 tok" &&
        compactRow("over budget") === "no" &&
        compactRow("can compact") === "yes",
    );

    d.getElementById("ctx-compact").click();
    await waitFor(() => LIVE.called("compact_now").length === 1, "compact_now to fire");
    await waitFor(() => compactRow("folded") === "4", "the folded count to rise");
    assert(
      `M6: Compact now folds the live turns and says so (live=${compactRow("live")}, folded=${
        compactRow("folded")}, canCompact=${compactRow("can compact")}, note="${
        d.getElementById("ctx-note").textContent.trim()}")`,
      LIVE.lastArgs("compact_now").session_id === state.activeId &&
        compactRow("live") === "0" &&
        compactRow("folded") === "4" &&
        compactRow("can compact") === "no" &&
        d.getElementById("ctx-note").textContent.trim() ===
          "Folded 4 turn(s); 0 live, 0 tok estimated.",
    );

    d.getElementById("ctx-restore").click();
    await waitFor(() => LIVE.called("restore_compacted").length === 1, "restore to fire");
    await waitFor(() => compactRow("folded") === "0", "the folded count to clear");
    assert(
      `M6: Restore folded turns brings them back (live=${compactRow("live")}, folded=${
        compactRow("folded")}, note="${d.getElementById("ctx-note").textContent.trim()}")`,
      compactRow("live") === "4" &&
        compactRow("folded") === "0" &&
        d.getElementById("ctx-note").textContent.trim() === "Restored 4 folded turn(s).",
    );

    d.querySelector("#spill-list .spill-row").click();
    await waitFor(() => LIVE.called("spill_read").length === 1, "the spill to be read");
    await waitFor(
      () => !d.getElementById("spill-detail").classList.contains("hidden"),
      "the spill detail to open",
    );
    const spillText = d.getElementById("spill-detail").textContent;
    assert(
      `M6: a spilled output opens with its truncation stated (args=${
        JSON.stringify(LIVE.lastArgs("spill_read"))}, head="${spillText.slice(0, 46)}")`,
      LIVE.lastArgs("spill_read").id === "spill-1" &&
        LIVE.lastArgs("spill_read").max_bytes === 8192 &&
        /spill-1\.txt \(truncated\)/.test(spillText) &&
        /spill line 0\b/.test(spillText),
    );

    /* ---- M7: jobs — list, a bounded tail, and stopping one ------------ */
    openPane("jobs-section");
    await waitFor(() => LIVE.called("jobs_list").length === 1, "the job list to be read");
    await waitFor(() => d.querySelectorAll("#jobs-list .job-row").length === 2, "two rows");
    const jobStates = () =>
      [...d.querySelectorAll("#jobs-list .job-row")].map(
        (r) => (r.querySelector(".job-state") || {}).textContent,
      );
    assert(
      `M7: the Jobs pane lists each job with its state (${jobStates().join(",")}, count="${
        d.getElementById("jobs-count").textContent.trim()}")`,
      jobStates().join(",") === "running,failed" &&
        d.getElementById("jobs-count").textContent.trim() === "2 shown",
    );

    d.querySelectorAll("#jobs-list .job-row")[0].click();
    await waitFor(() => LIVE.called("job_output").length === 1, "the job output to be read");
    await waitFor(() => /tail/.test(d.getElementById("job-detail").textContent), "the tail marker");
    const detail = d.getElementById("job-detail").textContent;
    assert(
      `M7: opening a job reads only its tail (args=${
        JSON.stringify(LIVE.lastArgs("job_output"))}, "${detail.slice(0, 52)}")`,
      LIVE.lastArgs("job_output").id === "job-1" &&
        LIVE.lastArgs("job_output").max_bytes === 16384 &&
        /test line 1999/.test(detail) &&
        !/test line 0$/.test(detail) &&
        /tail/.test(detail) &&
        /dropped/.test(detail),
    );

    [...d.getElementById("job-detail").querySelectorAll("button")].find(
      (b) => b.textContent === "Stop",
    ).click();
    await waitFor(() => LIVE.called("job_interrupt").length === 1, "the stop to cross");
    await waitFor(() => LIVE.called("jobs_list").length === 2, "the list to refresh");
    await waitFor(() => jobStates()[0] === "interrupted", "the row to show it stopped");
    assert(
      `M7: Stop interrupts the job and the list refreshes to that state (args=${
        JSON.stringify(LIVE.lastArgs("job_interrupt"))}, states=${jobStates().join(",")})`,
      LIVE.lastArgs("job_interrupt").id === "job-1" && jobStates()[0] === "interrupted",
    );

    /* ---- plan: a mid-stream update_plan repaints the todo surface ----- */
    type("make a plan for the turn");
    sendBtn.click();
    await waitFor(
      () =>
        !d.getElementById("plan-panel").classList.contains("hidden") &&
        /Seed the long chat/.test(d.getElementById("plan-list").textContent) &&
        !/wire the live harness/.test(d.getElementById("plan-list").textContent),
      "the plan to repaint from the stream",
    );
    assert(
      `plan: a mid-stream update_plan repaints the todo list from the session log (count=${
        d.getElementById("plan-count").textContent.trim()}, ${
        d.querySelectorAll("#plan-list .plan-item").length} rows, statuses=${
        [...d.querySelectorAll("#plan-list .plan-item")].map((n) =>
          n.className.replace("plan-item ", ""),
        ).join(",")})`,
      /1\/3 · 1 active/.test(d.getElementById("plan-count").textContent) &&
        d.querySelectorAll("#plan-list .plan-item").length === 3 &&
        !!d.querySelector("#plan-list .plan-item.status-completed"),
    );
    // The panel could not render before, so its geometry had never been
    // exercised: it must land in the viewport, above the composer, without
    // spilling sideways.
    const planBox = d.getElementById("plan-panel").getBoundingClientRect();
    const composerBox = d.getElementById("composer").getBoundingClientRect();
    assert(
      `plan: the rendered panel sits above the composer inside the viewport (top=${
        Math.round(planBox.top)}, bottom=${Math.round(planBox.bottom)}, composer=${
        Math.round(composerBox.top)}, left=${Math.round(planBox.left)}, right=${
        Math.round(planBox.right)} of ${d.documentElement.clientWidth})`,
      planBox.height > 20 &&
        planBox.bottom <= composerBox.top + 1 &&
        planBox.left >= -1 &&
        planBox.right <= d.documentElement.clientWidth + 1,
    );

    /* ---- plan editor: draft, reorder, save, cancel, keys, switch ----- */
    await waitFor(() => sendBtn.textContent.trim() === "Send", "the plan turn to settle");
    const draftInputs = () => [...d.querySelectorAll("#plan-list .plan-edit-input")];
    const draftText = () => draftInputs().map((i) => i.value);
    const draftRow = (text) =>
      [...d.querySelectorAll("#plan-list .plan-item")].find(
        (li) => (li.querySelector(".plan-edit-input") || {}).value === text,
      );
    const retitle = (el, text) => {
      el.value = text;
      el.dispatchEvent(new Event("input", { bubbles: true }));
    };
    await waitFor(
      () => !d.getElementById("plan-edit").classList.contains("hidden"),
      "Edit to appear once the chat is idle",
    );
    d.getElementById("plan-edit").click();
    await waitFor(() => draftInputs().length === 3, "the draft to open");
    assert(
      `plan editor: Edit opens the plan as a draft and hides itself (${
        draftText().join(" | ")})`,
      draftText().join("|") === "Read the harness|Seed the long chat|Measure the burst" &&
        d.getElementById("plan-edit").classList.contains("hidden") &&
        [...d.querySelectorAll(".plan-edit-foot button")].map((b) => b.textContent).join(",") ===
          "+ Add step,Save,Cancel",
    );

    draftRow("Read the harness").querySelector('button[title^="Move step down"]').click();
    assert(
      `plan editor: the down control reorders the draft in place (${
        draftText().join(" | ")})`,
      draftText().join("|") === "Seed the long chat|Read the harness|Measure the burst",
    );
    draftRow("Measure the burst").querySelector(".plan-status-btn").click();
    assert(
      `plan editor: the status control cycles the step (${draftRow("Measure the burst").className})`,
      draftRow("Measure the burst").classList.contains("status-in_progress"),
    );
    draftRow("Read the harness").querySelector('button[title="Remove step"]').click();
    const afterRemove = draftText();
    [...d.querySelectorAll(".plan-edit-foot button")]
      .find((b) => b.textContent === "+ Add step")
      .click();
    await waitFor(() => draftInputs().length === afterRemove.length + 1, "the blank row to appear");
    retitle(draftInputs()[draftInputs().length - 1], "Ship it");
    assert(
      `plan editor: remove drops a step and Add step appends one (${draftText().join(" | ")})`,
      afterRemove.join("|") === "Seed the long chat|Measure the burst" &&
        draftText().join("|") === "Seed the long chat|Measure the burst|Ship it",
    );

    const savesBefore = LIVE.called("set_plan").length;
    [...d.querySelectorAll(".plan-edit-foot button")]
      .find((b) => b.textContent === "Save")
      .click();
    await waitFor(() => LIVE.called("set_plan").length === savesBefore + 1, "set_plan to fire");
    const sentItems = LIVE.lastArgs("set_plan").items;
    assert(
      `plan editor: Save persists the draft in order, trimmed (${
        JSON.stringify(sentItems)})`,
      JSON.stringify(sentItems) === JSON.stringify([
        { text: "Seed the long chat", status: "in_progress" },
        { text: "Measure the burst", status: "in_progress" },
        { text: "Ship it", status: "pending" },
      ]) &&
        draftInputs().length === 0 &&
        d.getElementById("plan-count").textContent.trim() === "0/3 · 2 active",
    );

    /* Cancel and the two keys: neither may reach the backend. */
    const savesAfterSave = LIVE.called("set_plan").length;
    d.getElementById("plan-edit").click();
    await waitFor(() => draftInputs().length === 3, "the draft to reopen");
    retitle(draftInputs()[0], "Discard me");
    [...d.querySelectorAll(".plan-edit-foot button")]
      .find((b) => b.textContent === "Cancel")
      .click();
    assert(
      `plan editor: Cancel discards the draft without a write (${
        d.getElementById("plan-list").textContent.trim().slice(0, 30)}, writes=${
        LIVE.called("set_plan").length - savesAfterSave})`,
      LIVE.called("set_plan").length === savesAfterSave &&
        /Seed the long chat/.test(d.getElementById("plan-list").textContent) &&
        draftInputs().length === 0,
    );
    d.getElementById("plan-edit").click();
    await waitFor(() => draftInputs().length === 3, "the draft to reopen for the keys");
    retitle(draftInputs()[0], "Kept by Escape");
    draftInputs()[0].dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    const afterEscape = LIVE.called("set_plan").length;
    assert(
      `plan editor: Escape leaves the draft and writes nothing (${
        d.getElementById("plan-list").textContent.trim().slice(0, 24)})`,
      LIVE.called("set_plan").length === afterEscape &&
        !/Kept by Escape/.test(d.getElementById("plan-list").textContent) &&
        draftInputs().length === 0,
    );
    d.getElementById("plan-edit").click();
    await waitFor(() => draftInputs().length === 3, "the draft to reopen for Enter");
    retitle(draftInputs()[0], "Kept by Enter");
    draftInputs()[0].dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await waitFor(() => LIVE.called("set_plan").length === afterEscape + 1, "Enter to save");
    assert(
      `plan editor: Enter in a step saves it (${
        JSON.stringify(LIVE.lastArgs("set_plan").items[0])})`,
      LIVE.lastArgs("set_plan").items[0].text === "Kept by Enter" &&
        /Kept by Enter/.test(d.getElementById("plan-list").textContent),
    );

    /* The plan is per session: switching must restore that chat's own plan. */
    // Row titles get clipped by the sidebar, so the switch goes by the id the
    // row carries — the same reason the perf guard reads it.
    const longChatId = activeRow().dataset.id;
    rowByTitle("Empty notebook").click();
    await waitFor(
      () => /empty-chat step one/.test(d.getElementById("plan-list").textContent),
      "the other chat's plan to arrive",
    );
    assert(
      `plan: switching sessions restores that chat's plan (${
        d.getElementById("plan-list").textContent.trim().slice(0, 34)})`,
      /empty-chat step one/.test(d.getElementById("plan-list").textContent) &&
        !/Kept by Enter/.test(d.getElementById("plan-list").textContent),
    );
    [...d.querySelectorAll("#session-list .session-row")]
      .find((r) => r.dataset.id === longChatId)
      .click();
    await waitFor(
      () => /Kept by Enter/.test(d.getElementById("plan-list").textContent),
      "the first chat's plan to come back",
    );

    /* ---- goal: the state machine advances through the bridge ----------- */
    d.getElementById("goal-advance").click();
    await waitFor(() => LIVE.called("goal_action").length === 1, "goal_action to fire");
    assert(
      `goal: +1 advances the live counter through the bridge (args=${
        JSON.stringify(LIVE.lastArgs("goal_action"))}, chip=${
        d.getElementById("goal-rounds").textContent.trim()})`,
      LIVE.lastArgs("goal_action").action === "advance" &&
        /4 of 12/.test(d.getElementById("goal-rounds").textContent),
    );

    /* ---- settings: live read, live write, live memories, MCP line ------ */
    const modal = d.getElementById("settings-modal");
    const setModel = d.getElementById("set-model");
    d.getElementById("open-settings").click();
    await waitFor(() => !modal.classList.contains("hidden") && setModel.value !== "",
      "settings to load");
    assert(
      `settings: the dialog opens onto live values (model=${setModel.value})`,
      setModel.value === "glm-4.6",
    );
    await waitFor(
      () => /claude-sonnet/.test(d.getElementById("profiles-list").textContent),
      "profiles to render",
    );
    assert(
      `settings: profiles arrive from the backend (${d
        .getElementById("profiles-list")
        .textContent.replace(/\s+/g, " ")
        .slice(0, 40)})`,
      /claude-sonnet/.test(d.getElementById("profiles-list").textContent),
    );
    await waitFor(
      () => /remembers the streaming harness/.test(d.getElementById("mem-items").textContent),
      "memories to render",
    );
    d.getElementById("mem-text").value = "the live harness remembers this too";
    d.getElementById("mem-add").click();
    await waitFor(
      () => /the live harness remembers this too/.test(d.getElementById("mem-items").textContent),
      "the new memory to render",
    );
    assert(
      `settings: a new memory crosses the bridge and joins the list (args=${
        JSON.stringify(LIVE.lastArgs("remember_memory"))})`,
      LIVE.lastArgs("remember_memory").text === "the live harness remembers this too",
    );
    const memText = () => d.getElementById("mem-items").textContent;
    const memRows = () => [...d.querySelectorAll("#mem-items .mem-row")];
    await waitFor(() => /shared by 2 workspaces/.test(memText()), "the memory status line");
    assert(
      `settings: the memory pane names the file and who else lands on it (${
        memText().replace(/\s+/g, " ").slice(0, 96)})`,
      /\/home\/live\/\.bos\/gui\/memory\.jsonl/.test(memText()) &&
        /shared by 2 workspaces/.test(memText()) &&
        /2 of 500 kept/.test(memText()),
    );
    assert(
      `settings: both memories are rows, newest first (${memRows().length})`,
      memRows().length === 2 &&
        /the live harness remembers this too/.test(memRows()[0].textContent),
    );
    memRows()[0].querySelector("button").click();
    await waitFor(() => memRows().length === 1, "the forgotten memory to leave the list");
    assert(
      `settings: forgetting removes that memory, and only it (left=${
        memRows().map((r) => r.textContent.replace(/\s+/g, " ").slice(0, 22)).join("|")}, args=${
        JSON.stringify(LIVE.lastArgs("forget_memory"))})`,
      !/the live harness remembers this too/.test(memText()) &&
        /remembers the streaming harness/.test(memText()) &&
        /1 of 500 kept/.test(memText()) &&
        /^m-\d+$/.test((LIVE.lastArgs("forget_memory") || {}).id || ""),
    );
    await waitFor(
      () => /mcp 1\/1/.test(d.getElementById("status-right").textContent),
      "the mcp line to reach the status bar",
    );
    assert(
      `settings: the MCP status answers with connected servers (${
        [...d.querySelectorAll("#status-right .mcp")].map((m) => m.textContent).join("|")})`,
      /⛁ mcp 1\/1/.test(
        [...d.querySelectorAll("#status-right .mcp")].map((m) => m.textContent).join("|")),
    );
    setModel.value = "glm-4.7";
    d.getElementById("settings-form").dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await waitFor(() => modal.classList.contains("hidden"), "save to close the dialog");
    assert(
      `settings: save crosses the bridge with the new model (args=${
        JSON.stringify(LIVE.lastArgs("save_settings")).slice(0, 60)})`,
      LIVE.lastArgs("save_settings").model === "glm-4.7",
    );
    assert(
      `settings: the saved model owns the status line at once (${
        d.getElementById("status-left").textContent.slice(0, 12)})`,
      /glm-4\.7/.test(d.getElementById("status-left").textContent),
    );
    d.getElementById("open-settings").click();
    await waitFor(() => !modal.classList.contains("hidden") && setModel.value !== "",
      "settings to reload");
    assert(
      `settings: the model round-trips through the store (model=${setModel.value})`,
      setModel.value === "glm-4.7",
    );
    // The web tools fields ride the same save: endpoint and key round-trip,
    // and the switch goes off and back on.
    const setWebTools = d.getElementById("set-web-tools");
    const setSearchUrl = d.getElementById("set-search-url");
    const setSearchKey = d.getElementById("set-search-key");
    assert(
      `settings: the web tools fields read live values (url=${setSearchUrl.value}, on=${setWebTools.checked})`,
      setWebTools.checked === true && setSearchUrl.value === "" && setSearchKey.value === "",
    );
    setSearchUrl.value = "https://api.tavily.com/search";
    setSearchKey.value = "tvly-live";
    d.getElementById("settings-form").dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await waitFor(() => modal.classList.contains("hidden"), "the web settings to save");
    assert(
      `settings: the search endpoint and key cross the bridge (args=${
        JSON.stringify(LIVE.lastArgs("save_settings")).slice(0, 44)})`,
      LIVE.lastArgs("save_settings").searchApiUrl === "https://api.tavily.com/search" &&
        LIVE.lastArgs("save_settings").searchApiKey === "tvly-live",
    );
    d.getElementById("open-settings").click();
    await waitFor(() => !modal.classList.contains("hidden") && setModel.value !== "",
      "settings to reload for web tools");
    assert(
      `settings: the search endpoint and key round-trip (url=${setSearchUrl.value})`,
      setSearchUrl.value === "https://api.tavily.com/search" &&
        setSearchKey.value === "tvly-live",
    );
    setWebTools.checked = false;
    d.getElementById("settings-form").dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await waitFor(() => modal.classList.contains("hidden"), "the switch-off to save");
    d.getElementById("open-settings").click();
    await waitFor(() => !modal.classList.contains("hidden") && setModel.value !== "",
      "settings to reload again");
    assert(
      "settings: switching the web tools off survives the round-trip",
      setWebTools.checked === false,
    );
    setWebTools.checked = true;
    d.getElementById("settings-form").dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await waitFor(() => modal.classList.contains("hidden"), "the switch-on to save");
    // Restore the seeded shape: empty endpoint, tools on.
    d.getElementById("open-settings").click();
    await waitFor(() => !modal.classList.contains("hidden") && setModel.value !== "",
      "settings to reload a third time");
    setSearchUrl.value = "";
    setSearchKey.value = "";
    d.getElementById("settings-form").dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await waitFor(() => modal.classList.contains("hidden"), "settings to close clean");

    /* ---- trash: delete, see it named in settings, restore -------------- */
    window.confirm = () => true;
    const victim = rowByTitle("Empty notebook");
    assert("trash: the seeded empty chat is in the sidebar", !!victim);
    // Delete lives in the row menu now, where the reference puts it.
    victim.querySelector(".sess-menu").click();
    d.querySelector('.row-menu [data-run="delete"]').click();
    await waitFor(() => LIVE.called("delete_session").length === 1, "delete_session to fire");
    await waitFor(() => !rowByTitle("Empty notebook"), "the row to leave the sidebar");
    assert(
      "trash: deleting moves the chat out of the sidebar",
      !rowByTitle("Empty notebook") && LIVE.lastArgs("delete_session").id.startsWith("live-"),
    );
    d.getElementById("open-settings").click();
    await waitFor(() => !modal.classList.contains("hidden"), "settings to open for the trash");
    const trashHost = d.getElementById("trash-list") || d.querySelector(".trash-row")?.parentElement;
    await waitFor(() => /Empty notebook/.test((trashHost || d.body).textContent),
      "the trash to render its row");
    assert(
      `trash: the trashed chat is named in settings (${trashHost.textContent.replace(/\s+/g, " ").slice(0, 40)})`,
      /Empty notebook/.test(trashHost.textContent),
    );
    trashHost.querySelector(".trash-restore").click();
    await waitFor(() => LIVE.called("restore_session").length === 1, "restore_session to fire");
    await waitFor(() => rowByTitle("Empty notebook"), "the chat to come back");
    assert(
      `trash: restore brings the chat back by name (toast-args=${JSON.stringify(
        LIVE.lastArgs("restore_session"))})`,
      !!rowByTitle("Empty notebook"),
    );
    d.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await waitFor(() => modal.classList.contains("hidden"), "settings to close after trash");

    /* ---- search: content search answers with snippets ------------------ */
    const searchBox = d.getElementById("session-search");
    searchBox.value = "harness";
    searchBox.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => LIVE.called("search_sessions").length >= 1, "search_sessions to fire");
    await waitFor(
      () => d.querySelectorAll("#session-list .hit-row").length > 0,
      "hit rows to render",
    );
    const hit = d.querySelector("#session-list .hit-row");
    assert(
      `search: a content hit renders with its snippet (${hit.textContent.replace(/\s+/g, " ").slice(0, 60)})`,
      /match(es)? inside chats/.test(d.getElementById("session-list").textContent) &&
        /^Design the streaming/.test(hit.querySelector(".title").textContent) &&
        /harness/.test(hit.querySelector(".hit-snippet").textContent),
    );
    searchBox.value = "";
    searchBox.dispatchEvent(new Event("input", { bubbles: true }));

    /* ---- the Files section walks the workspace, and opens what it finds - */
    // The tree moved from the sidebar into the Settings rail (rev 110), and this
    // is the only guard that sees the whole loop: select the section, expand a
    // folder (a listing keyed by path, so the root's rows cannot pass for the
    // folder's), then open a file into the document panel.
    openPane("files-section");
    await waitFor(
      () => d.querySelectorAll("#file-tree .file-row").length === 2,
      "the workspace root to list",
    );
    const treeRows = () => [...d.querySelectorAll("#file-tree .file-row")];
    const rowNames = () =>
      treeRows().map((row) => row.querySelector(".file-name").textContent);
    assert(
      `Files: selecting the section lists the workspace root (${rowNames().join(
        " | ",
      )} asked=${JSON.stringify(LIVE.lastArgs("list_files"))})`,
      rowNames().join(" | ") === "src | README.md" &&
        LIVE.lastArgs("list_files").path === "" &&
        /21 B/.test(treeRows()[1].textContent),
    );
    treeRows()[0].click();
    await waitFor(
      () => d.querySelectorAll("#file-tree .file-row").length === 3,
      "the folder to expand",
    );
    assert(
      `Files: expanding a folder asks for that folder (${rowNames().join(
        " | ",
      )} asked=${JSON.stringify(LIVE.lastArgs("list_files"))})`,
      rowNames().join(" | ") === "src | main.rs | README.md" &&
        LIVE.lastArgs("list_files").path === "src" &&
        // Depth is what says `main.rs` is inside `src` rather than beside it.
        parseFloat(treeRows()[1].style.paddingLeft) >
          parseFloat(treeRows()[0].style.paddingLeft),
    );
    treeRows()[1].click();
    await waitFor(
      () => /fn main/.test(d.getElementById("doc-body").textContent),
      "the file to open in the document panel",
    );
    assert(
      `Files: opening a file fills the document panel (${
        d.getElementById("doc-path").textContent
      })`,
      LIVE.lastArgs("read_file").path === "src/main.rs" &&
        d.getElementById("doc-path").textContent === "src/main.rs",
    );

    /* ---- a finished turn renders the store's text, not the draft ------- */
    // The stream wrote one reply; the store holds another. After the turn ends
    // the UI re-reads the session, so the stored text is what must be on screen.
    // The stream writes one reply; the store holds another. The UI adopts the
    // store's version when the turn ends, so that text must be what is drawn —
    // waiting on a re-read count alone can catch a straggler from an earlier
    // turn and read the transcript mid-flight.
    type("finalize this answer");
    sendBtn.click();
    await waitFor(
      () =>
        (state.cache[state.activeId] || []).some((m) =>
          (m.text || "").includes("finalized by the store"),
        ),
      "the turn to end and adopt the store's text",
    );
    // rAF starves under virtual time: run the frame that would follow by hand.
    repaint();
    const afterFinalize = d.getElementById("messages").textContent;
    assert(
      `a finished turn renders the store's text, not the streamed draft (${
        afterFinalize.includes("finalized by the store") ? "store" : "draft"})`,
      afterFinalize.includes("finalized by the store") &&
        !afterFinalize.includes(
          "Hello, world — from the live harness. (you said: finalize this answer)",
        ),
    );

    // ---- Schedule: a reminder is added, delivered late, and cancelled ------
    // The store takes its clock from the caller, so a reminder can be made due
    // without waiting: one added through the pane's own form lands in the
    // future, and one added already past due is delivered by the next tick —
    // late, which is what the pane has to say instead of "now".
    openPane("schedule-section");
    await waitFor(() => LIVE.called("schedule_list").length >= 1, "the reminders to be read");
    const fill = (id, value) => {
      d.getElementById(id).value = value;
    };
    fill("sched-text", "water the plants");
    fill("sched-every", "5");
    fill("sched-mode", "every");
    d.getElementById("sched-add").click();
    await waitFor(() => LIVE.called("schedule_add").length === 1, "the reminder to be added");
    await waitFor(
      () => /water the plants/.test(d.getElementById("sched-items").textContent),
      "the reminder to be drawn",
    );
    const schedItems = () => d.getElementById("sched-items").textContent.replace(/\s+/g, " ");
    assert(
      `a reminder added in the pane is listed with its cadence and next due (${
        schedItems().slice(0, 96)})`,
      /water the plants · every 5 min · in 5 min/.test(schedItems()) &&
        /1 of 50 reminders/.test(d.getElementById("sched-note").textContent) &&
        /a repeat needs at least 1 min/.test(d.getElementById("sched-note").textContent) &&
        /1 scheduled/.test(d.getElementById("sched-count").textContent),
    );

    await window.__TAURI__.core.invoke("schedule_add", {
      text: "drink water",
      atMs: Date.now() - 5 * 60000,
      nowMs: Date.now(),
    });
    const ticksBefore = LIVE.called("schedule_tick").length;
    await window.tickSchedule();
    await waitFor(
      () => LIVE.called("schedule_tick").length > ticksBefore,
      "the tick to reach the store",
    );
    const schedHistory = () => d.getElementById("sched-history").textContent.replace(/\s+/g, " ");
    assert(
      `a due reminder is delivered and says how late it was (${schedHistory().slice(0, 88)})`,
      /drink water/.test(schedHistory()) &&
        /5 min late/.test(schedHistory()) &&
        LIVE.called("schedule_tick").length > ticksBefore &&
        // A one-shot has no next time, so it leaves the list; its delivery is
        // the record that it happened.
        !/drink water/.test(schedItems()),
    );

    d.querySelector("#sched-items .rule-row button").click();
    await waitFor(() => LIVE.called("schedule_cancel").length === 1, "the reminder to be cancelled");
    await waitFor(
      () => /Nothing scheduled yet/.test(schedItems()),
      "the list to drop the cancelled reminder",
    );
    assert(
      `cancelling removes the reminder and keeps what it delivered (${
        schedItems().slice(0, 60)})`,
      /Nothing scheduled yet/.test(schedItems()) && /drink water/.test(schedHistory()),
    );

    d.getElementById("sched-clear").click();
    await waitFor(
      () => LIVE.called("schedule_clear_history").length === 1,
      "the history to be cleared",
    );
    await waitFor(
      () => /Nothing delivered yet/.test(schedHistory()),
      "the history to empty",
    );
    assert(
      "clearing the history empties it and leaves the list empty too",
      /Nothing delivered yet/.test(schedHistory()) &&
        /Nothing scheduled yet/.test(schedItems()) &&
        /0 of 200 deliveries kept/.test(d.getElementById("sched-note").textContent),
    );

    // ---- Knowledge: curate a note, search it, edit it, delete it ----------
    // The pane is the curator for the store the agent searches, so every path a
    // person has (add, find, rewrite, remove) has to reach the backend with the
    // values on screen — and a blank title has to be refused before the round
    // trip rather than after it.
    openPane("knowledge-section");
    await waitFor(() => LIVE.called("knowledge_list").length >= 1, "the notes to be read");
    const kbHost = () => d.getElementById("kb-items");
    const kbText = () => kbHost().textContent.replace(/\s+/g, " ");
    const kbRows = () => [...d.querySelectorAll("#kb-items .kb-row")];
    const kbFill = (title, tags, body) => {
      d.getElementById("kb-title").value = title;
      d.getElementById("kb-tags").value = tags;
      d.getElementById("kb-body").value = body;
    };
    await waitFor(() => /No knowledge yet/.test(kbText()), "the empty pane to say so");

    kbFill("Deploy ritual", "Ops, ops, release", "Tag the commit, then push main.");
    d.getElementById("kb-save").click();
    await waitFor(() => LIVE.called("knowledge_add").length === 1, "the note to be saved");
    await waitFor(() => /Deploy ritual/.test(kbText()), "the note to be drawn");
    assert(
      `a note added in the pane is listed with its tags and age (${
        kbText().slice(0, 92)})`,
      /Deploy ritual/.test(kbText()) &&
        /ops · release/.test(kbText()) &&
        /edited just now/.test(kbText()) &&
        /1 of 500 entries/.test(d.getElementById("kb-note").textContent) &&
        d.getElementById("kb-title").value === "",
    );

    kbFill("House style", "style", "Two-space indent, no semicolons.");
    d.getElementById("kb-save").click();
    await waitFor(() => LIVE.called("knowledge_add").length === 2, "the second note to be saved");
    d.getElementById("kb-title").value = "";
    d.getElementById("kb-save").click();
    await waitFor(
      () => /needs a title/.test(d.getElementById("kb-note").textContent),
      "the blank title to be refused",
    );
    assert(
      "a blank title is refused here, not by the backend",
      LIVE.called("knowledge_add").length === 2 &&
        d.getElementById("kb-save").textContent !== "Save" &&
        /needs a title/.test(d.getElementById("kb-note").textContent),
    );

    d.getElementById("kb-query").value = "ritual";
    d.getElementById("kb-search").click();
    await waitFor(() => LIVE.called("knowledge_search").length === 1, "the search to reach the store");
    await waitFor(() => kbRows().length === 1, "the list to narrow");
    assert(
      `a search draws only the matches, and still says how many notes exist (${
        d.getElementById("kb-note").textContent.slice(0, 74)})`,
      /Deploy ritual/.test(kbText()) &&
        !/House style/.test(kbText()) &&
        kbRows().length === 1 &&
        // The bounds line counts every entry, not the matches: a filtered view
        // has to say the corpus is bigger than what it is showing.
        /2 of 500 entries/.test(d.getElementById("kb-note").textContent) &&
        LIVE.lastArgs("knowledge_search").query === "ritual",
    );

    kbRows()[0].querySelector(".kb-edit").click();
    await waitFor(
      () => d.getElementById("kb-title").value === "Deploy ritual",
      "the form to fill from the row",
    );
    assert(
      `editing fills the form from the row and offers Save (${
        d.getElementById("kb-save").textContent})`,
      d.getElementById("kb-save").textContent === "Save" &&
        d.getElementById("kb-tags").value === "ops, release" &&
        d.getElementById("kb-body").value === "Tag the commit, then push main.",
    );
    kbFill("Deploy ritual, edited", "ops", "Copy the release notes first.");
    d.getElementById("kb-save").click();
    await waitFor(() => LIVE.called("knowledge_update").length === 1, "the edit to be saved");
    await waitFor(() => /Copy the release notes first/.test(kbText()), "the edit to be drawn");
    assert(
      "the edited note is rewritten in place, and the form goes back to Add",
      /Deploy ritual, edited/.test(kbText()) &&
        d.getElementById("kb-save").textContent === "Add" &&
        d.getElementById("kb-cancel").hidden === true &&
        LIVE.lastArgs("knowledge_update").id === "kb-" + LIVE.lastArgs("knowledge_update").nowMs + "-1",
    );

    d.getElementById("kb-all").click();
    await waitFor(() => kbRows().length === 2, "the whole list again");
    kbRows()[1].querySelector(".kb-del").click();
    await waitFor(() => LIVE.called("knowledge_delete").length === 1, "the delete to reach the store");
    await waitFor(() => kbRows().length === 1, "the row to go");
    assert(
      `deleting removes that note and only it (${kbText().slice(0, 60)})`,
      /Deploy ritual, edited/.test(kbText()) &&
        !/House style/.test(kbText()) &&
        /1 of 500 entries/.test(d.getElementById("kb-note").textContent),
    );

    /* ---- M9: the sidebar's row menu, against the double --------------- */
    {
      const rows = () => [...d.querySelectorAll("#session-list .session-row")];
      // Which workspace group a row is drawn under, so the pin can be checked
      // against the list it orders (a pin sorts chats *inside* a workspace).
      const groupOf = (row) => {
        let n = row.previousElementSibling;
        while (n && !n.classList.contains("ws-head")) n = n.previousElementSibling;
        return n ? n.textContent : "";
      };
      // A group with at least two chat rows, so "it floated" is observable: the
      // pinned row must be the last of its group, and the list must move it
      // ahead of the row that was first there.
      const before = rows();
      const target = [...before].reverse().find((r) => groupOf(r) === groupOf(before[0]) && r !== before[0]);
      assert(`M9: the sidebar offers a second chat to pin (${before.length} rows)`, !!target);
      const targetId = target.dataset.id;
      const wasFirstInGroup = before.find((r) => groupOf(r) === groupOf(target));
      const unstarred = target.querySelector(".sess-pin").textContent;
      target.querySelector(".sess-pin").click();
      await waitFor(
        () => LIVE.called("set_session_pinned").length === 1,
        "the pin to reach the backend",
      );
      await waitFor(
        () => rows()[0].dataset.id === targetId || rows().findIndex((r) => r.dataset.id === targetId) <
          rows().findIndex((r) => r.dataset.id === wasFirstInGroup.dataset.id),
        "the pinned chat to float above its group",
      );
      const pinned = d.querySelector(`#session-list .session-row[data-id="${targetId}"]`);
      assert(
        `M9: the star pins a chat, marks it and floats it above the group's first row (${
          wasFirstInGroup.dataset.id} → ${targetId}, was "${unstarred}")`,
        LIVE.lastArgs("set_session_pinned").id === targetId &&
          LIVE.lastArgs("set_session_pinned").pinned === true &&
          pinned.querySelector(".sess-pin").getAttribute("aria-pressed") === "true" &&
          pinned.querySelector(".sess-pin").textContent === "★" &&
          rows().findIndex((r) => r.dataset.id === targetId) <
            rows().findIndex((r) => r.dataset.id === wasFirstInGroup.dataset.id) &&
          LIVE.sessions().find((x) => x.id === targetId).pinned === true,
      );

      // ⌘G renames the open chat in place. Dispatched on the document, the way
      // a real chord arrives when focus is not in a field.
      const openId = activeRow().dataset.id;
      d.dispatchEvent(new KeyboardEvent("keydown", { key: "g", metaKey: true, bubbles: true }));
      const field = d.querySelector(`#session-list .session-row[data-id="${openId}"] .sess-rename`);
      assert(
        `M9: Cmd/Ctrl+G turns the open chat's row into a field (${field && field.value})`,
        !!field,
      );
      const renamed = `renamed ${Math.floor(Date.now() / 1000) % 100000}`;
      field.value = `  ${renamed}  `;
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      await waitFor(
        () => LIVE.called("rename_session").length === 1,
        "the rename to reach the backend",
      );
      await waitFor(
        () => !d.querySelector(".sess-rename") && rowByTitle(renamed),
        "the row to redraw under its new name",
      );
      assert(
        `M9: the rename round-trips — the row and the store both carry "${renamed}" (${
          JSON.stringify(LIVE.lastArgs("rename_session"))
        })`,
        LIVE.lastArgs("rename_session").id === openId &&
          LIVE.lastArgs("rename_session").title === `  ${renamed}  ` &&
          LIVE.sessions().find((x) => x.id === openId).title === renamed &&
          !!rowByTitle(renamed),
      );

      // Archive the open chat: it leaves the list, the head's menu counts it, and
      // restoring — through the filter that menu sets — puts it back with the
      // name it had.
      d.dispatchEvent(new KeyboardEvent("keydown", { key: "a", metaKey: true, shiftKey: true, bubbles: true }));
      await waitFor(
        () => LIVE.called("set_session_archived").length === 1,
        "the archive to reach the backend",
      );
      await waitFor(() => !rowByTitle(renamed), "the chat to leave the list");
      d.getElementById("session-view").click();
      await waitFor(() => !!d.querySelector(".row-menu"), "the view menu to open");
      const viewItems = () => [...d.querySelectorAll(".row-menu .row-menu-item")];
      const hideItem = viewItems().find((b) => /^Hide archived/.test(b.textContent));
      assert(
        `M9: archiving hides the chat and the view menu counts it (${hideItem && hideItem.textContent.trim()})`,
        !!hideItem &&
          /Hide archived \(1\)/.test(hideItem.textContent) &&
          hideItem.getAttribute("aria-checked") === "true" &&
          LIVE.lastArgs("set_session_archived").id === openId &&
          LIVE.lastArgs("set_session_archived").archived === true &&
          state.activeId !== openId,
      );
      // "All conversations" is the way back to an archived chat now: the row it
      // reveals is the same choice read back, not a second disclosure's state.
      const allItem = viewItems().find((b) => /^All conversations/.test(b.textContent));
      allItem.click();
      await waitFor(
        () => !!d.querySelector(`#session-list .session-row.archived-row[data-id="${openId}"]`),
        "the archived row to appear",
      );
      const back = d.querySelector(`#session-list .session-row.archived-row[data-id="${openId}"]`);
      back.querySelector(".sess-restore").click();
      await waitFor(
        () => LIVE.called("set_session_archived").length === 2,
        "the restore to reach the backend",
      );
      await waitFor(() => !!rowByTitle(renamed), "the chat to come back to the list");
      assert(
        `M9: Restore puts the chat back in the list, whole and named "${renamed}"`,
        !!rowByTitle(renamed) &&
          LIVE.lastArgs("set_session_archived").archived === false &&
          LIVE.sessions().find((x) => x.id === openId).archived === false &&
          !!activeRow(),
      );

      // Fork from the row menu: the whole conversation, as a chat of its own.
      // The name comes from the *store*, not the row: the row draws a clipped
      // title, so a long chat's branch would be searched for under a string the
      // backend never used.
      const forkTarget = rows().find((r) => LIVE.messages(r.dataset.id).length > 0) || rows()[0];
      const forkId = forkTarget.dataset.id;
      const sourceTitle = LIVE.sessions().find((x) => x.id === forkId).title;
      await d.querySelector(`#session-list .session-row[data-id="${forkId}"] .sess-menu`).click();
      const entries = [...d.querySelectorAll(".row-menu .row-menu-item")];
      const forkEntry = entries.find((e) => e.dataset.run === "fork");
      if (forkEntry) forkEntry.click();
      const branchRow = () => {
        const b = LIVE.sessions().find((x) => x.title === `${sourceTitle} (branch)`);
        return b ? d.querySelector(`#session-list .session-row[data-id="${b.id}"]`) : null;
      };
      await waitFor(() => LIVE.called("fork_session").length === 1, "the fork to reach the backend");
      await sleep(120);
      if (!branchRow()) {
        assert(
          `M9: the branch appeared (store=${LIVE.sessions()
            .map((x) => x.title)
            .join(" | ")}; rows=${rows()
            .map((r) => r.dataset.id)
            .join(",")}; src=${forkId}:${LIVE.messages(forkId).length}; ${
            LIVE.lastArgs("fork_session") ? JSON.stringify(LIVE.lastArgs("fork_session")) : "no call"
          }; said=${JSON.stringify(state.error)};${
            LIVE.messages(forkId).length > 0 ? "has turns" : "no turns"
          })`,
          false,
        );
      }
      const branch = LIVE.sessions().find((x) => x.title === `${sourceTitle} (branch)`);
      assert(
        `M9: Fork copies the whole conversation into a chat named "${sourceTitle} (branch)" (${
          JSON.stringify(LIVE.lastArgs("fork_session"))
        })`,
        !!branchRow() &&
          LIVE.lastArgs("fork_session").sessionId === forkId &&
          LIVE.lastArgs("fork_session").index === LIVE.messages(forkId).length &&
          LIVE.messages(branch.id).length === LIVE.messages(forkId).length &&
          !branch.archived &&
          !branch.pinned,
      );
      // The backend refuses a branch at index 0, and a chat with no turns has
      // nothing else: the page must refuse it too, in words, without asking.
      const empty = rows().find((r) => LIVE.messages(r.dataset.id).length === 0);
      if (empty) {
        const beforeFork = LIVE.called("fork_session").length;
        d.querySelector(`#session-list .session-row[data-id="${empty.dataset.id}"] .sess-menu`).click();
        d.querySelector('.row-menu [data-run="fork"]').click();
        await settle();
        await settle();
        assert(
          `M9: forking a chat with no turns is refused in words, with no request (${
            LIVE.called("fork_session").length
          } forks sent)`,
          LIVE.called("fork_session").length === beforeFork &&
            /no turns to fork/.test(d.getElementById("status-left") ? d.body.textContent : ""),
        );
      }

      // "Branch" in a message's own actions had no guard, and its index was one
      // short: the double added the one back, so only the real contract shows it.
      const branchSource = LIVE.sessions()
        .filter((x) => LIVE.messages(x.id).length >= 2 && !x.archived)
        .map((x) => x.id)[0];
      if (branchSource) {
        await window.switchSession(branchSource);
        await sleep(60);
        repaint();
        const buttons = [...d.querySelectorAll("#messages .mini-btn")].filter(
          (b) => b.textContent === "Branch",
        );
        // The tail window always ends at the newest message, so the last Branch
        // button belongs to the last one — the one absolute index the transcript
        // states without being asked. Branching there must keep all of it.
        const wantIndex = LIVE.messages(branchSource).length;
        if (buttons[buttons.length - 1]) buttons[buttons.length - 1].click();
        await waitFor(
          () => LIVE.called("fork_session").length === 2,
          "the transcript branch to reach the backend",
        );
        await sleep(80);
        const made = LIVE.sessions().find(
          (x) => x.title === `${LIVE.sessions().find((y) => y.id === branchSource).title} (branch)`,
        );
        assert(
          `M9: Branch in a message's actions keeps that message and every one before it (${
            JSON.stringify(LIVE.lastArgs("fork_session"))
          } of ${LIVE.messages(branchSource).length}; wanted ${wantIndex})`,
          !!made &&
            LIVE.lastArgs("fork_session").index === wantIndex &&
            LIVE.messages(made.id).length === wantIndex &&
            LIVE.messages(branchSource).length >= wantIndex,
        );
      }

      // No menu may outlive the click that used it.
      assert(
        `M9: the row menu closes itself once an entry is picked (${d.querySelectorAll(".row-menu").length} left)`,
        d.querySelectorAll(".row-menu").length === 0,
      );
    }

    /* ---- the sidebar's prominent action asks the system panel ---------- */
    // It opened a chat before rev 111, and a form before rev 114. The gesture is
    // the folder, so the panel comes first and its answer *is* the workspace.
    {
      const before = LIVE.settings().workspaces.length;
      const createsBefore = LIVE.called("create_workspace").length;
      closeSettings();
      closeAddWorkspace();
      await sleep(20);
      LIVE.answerNext("choose_folder", "/home/e2e/bos");
      d.getElementById("workspace-add-primary").click();
      await waitFor(
        () => LIVE.called("create_workspace").length === createsBefore + 1,
        "the pick to reach the backend as a workspace",
      );
      const sent = LIVE.lastArgs("create_workspace");
      assert(
        `M9: the folder-plus asks the system panel first and the pick becomes the workspace (${JSON.stringify({ asked: LIVE.lastArgs("choose_folder").start, root: sent.root, name: sent.name, config: sent.config })})`,
        sent.root === "/home/e2e/bos" &&
          sent.name === "bos" &&
          sent.config === null &&
          d.getElementById("ws-add").classList.contains("hidden") &&
          d.getElementById("settings-modal").classList.contains("hidden"),
      );
      const made = LIVE.settings().workspaces.find((w) => w.root === "/home/e2e/bos");
      assert(
        `M9: the folder's workspace is the active one and is in the sidebar (${[...d.querySelectorAll("#session-list .ws-name")].map((n) => n.textContent).join(",")})`,
        !!made &&
          LIVE.settings().active_workspace === made.id &&
          [...d.querySelectorAll("#session-list .ws-name")].some((n) => n.textContent === "bos"),
      );
      // A folder that is already a workspace — the one that is *not* active here —
      // is focused rather than recorded twice: with a panel, one click on a folder
      // you already have is the easy mistake, and two records for one folder is a
      // state nobody asked for.
      LIVE.answerNext("choose_folder", "/tmp/second");
      d.getElementById("workspace-add-primary").click();
      await waitFor(
        () => LIVE.settings().active_workspace === "w-second",
        "the folder that already is a workspace to be focused",
      );
      assert(
        `M9: picking a folder that already is a workspace focuses it instead of adding it again (${LIVE.settings().workspaces.length} workspaces, ${LIVE.called("create_workspace").length} creations)`,
        LIVE.settings().workspaces.length === before + 1 &&
          LIVE.called("create_workspace").length === createsBefore + 1,
      );
      // Settings lists and edits, it does not add: its "New workspace" is a doorway
      // to this same flow, so there is one answer to what adding a workspace does.
      d.getElementById("open-settings").click();
      await waitFor(() => !d.getElementById("settings-modal").classList.contains("hidden"), "settings to open");
      d.getElementById("set-tab-ws-list").click();
      await sleep(20);
      LIVE.answerNext("choose_folder", "/home/e2e/docs");
      d.getElementById("ws-manage-add").click();
      await waitFor(
        () => LIVE.called("create_workspace").length === createsBefore + 2,
        "the Settings doorway to reach the panel",
      );
      assert(
        `M9: the Settings doorway runs the same panel-first flow (${LIVE.lastArgs("create_workspace").root})`,
        LIVE.lastArgs("create_workspace").root === "/home/e2e/docs" &&
          d.getElementById("settings-modal").classList.contains("hidden") &&
          d.getElementById("ws-add").classList.contains("hidden"),
      );
      // A dismissal is not an error and not a change.
      const afterDoors = LIVE.settings().workspaces.length;
      LIVE.answerNext("choose_folder", null);
      d.getElementById("workspace-add-primary").click();
      await sleep(80);
      assert(
        `M9: dismissing the panel adds no workspace and opens no form (${afterDoors} to ${LIVE.settings().workspaces.length}, ${d.getElementById("ws-add").className.trim()})`,
        LIVE.settings().workspaces.length === afterDoors &&
          d.getElementById("ws-add").classList.contains("hidden"),
      );
    }

    /* ---- the form the flow falls back to, end to end -------------------- */
    // A machine with no folder panel is the way to the form, and there the form is
    // the whole answer: the app's own browser walks the tree, the MCP switches are
    // that workspace's own, and one call carries all of it. Read back from the
    // bridge's store, because what the form rendered is not evidence that the
    // backend was told.
    {
      const workspacesBefore = LIVE.settings().workspaces.length;
      // Whatever dialog an earlier block left open is closed first, so "this
      // gesture did not open Settings" is a statement about this gesture.
      closeSettings();
      closeAddWorkspace();
      await sleep(20);
      LIVE.answerNext("choose_folder", new Error("no folder dialog here: osascript is not installed"));
      d.getElementById("workspace-add-primary").click();
      await waitFor(() => !d.getElementById("ws-add").classList.contains("hidden"), "the fallback form to open");
      const modal = d.getElementById("ws-add");
      assert(
        `M9: with no panel the folder-plus opens the form, its browser and the reason (${d.getElementById("ws-add-browse-hint").textContent})`,
        !modal.classList.contains("hidden") &&
          !d.getElementById("ws-add-browser").classList.contains("hidden") &&
          d.getElementById("ws-add-browse-hint").textContent.includes("osascript") &&
          d.getElementById("settings-modal").classList.contains("hidden"),
      );
      assert(
        `M9: the dialog lists the MCP servers with their own switches (${[...d.querySelectorAll(".ws-add-mcp-on")].map((b) => b.value + "=" + b.checked).join(",")})`,
        [...d.querySelectorAll(".ws-add-mcp-on")].map((b) => b.value).join(",") === "filesystem,notes" &&
          d.querySelector('.ws-add-mcp-on[value="notes"]').checked === false,
      );
      // The system panel: modal, un-drivable, and therefore asked for through the
      // backend, whose answer this page only has to act on. A path lands.
      d.getElementById("ws-add-root").value = "/home/e2e";
      d.getElementById("ws-add-browse").click();
      await waitFor(
        () => d.getElementById("ws-add-root").value === "/home/e2e/bos",
        "the panel's folder to land in the field",
      );
      assert(
        `M9: the system panel's answer lands in the root field (${JSON.stringify(LIVE.lastArgs("choose_folder"))} → ${d.getElementById("ws-add-root").value})`,
        d.getElementById("ws-add-root").value === "/home/e2e/bos" &&
          d.getElementById("ws-add-browser").classList.contains("hidden") &&
          d.getElementById("ws-add-name").value === "bos",
      );
      // Dismissing it is not an error and not a change.
      d.getElementById("ws-add-root").value = "";
      LIVE.answerNext("choose_folder", null);
      d.getElementById("ws-add-browse").click();
      await sleep(60);
      assert(
        `M9: dismissing the panel leaves the field alone and writes no error (${JSON.stringify(d.getElementById("ws-add-root").value)} / ${JSON.stringify(d.getElementById("ws-add-err").textContent)})`,
        d.getElementById("ws-add-root").value === "" &&
          d.getElementById("ws-add-err").textContent === "" &&
          d.getElementById("ws-add-browser").classList.contains("hidden"),
      );
      // No panel on this machine: the app opens its own browser and says why,
      // and the field keeps whatever it had.
      d.getElementById("ws-add-root").value = "/home/e2e";
      LIVE.answerNext("choose_folder", new Error("no folder dialog here: osascript is not installed"));
      d.getElementById("ws-add-browse").click();
      await waitFor(
        () => !d.getElementById("ws-add-browser").classList.contains("hidden"),
        "the fallback browser to open",
      );
      assert(
        `M9: an absent panel opens the in-app browser and says why (${d.getElementById("ws-add-browse-hint").textContent})`,
        d.getElementById("ws-add-browse-hint").textContent.includes("osascript") &&
          d.getElementById("ws-add-root").value === "/home/e2e",
      );
      // The browser walks the filesystem one level at a time and hands the
      // resolved path back to the same field.
      await waitFor(() => d.querySelectorAll("#ws-browse-list .ws-browse-row").length === 2, "the folder level to render");
      d.querySelector('#ws-browse-list .ws-browse-row[data-path="/home/e2e/bos"]').click();
      await waitFor(
        () => d.getElementById("ws-browse-path").textContent === "/home/e2e/bos",
        "the descent into bos",
      );
      d.getElementById("ws-browse-use").click();
      await sleep(20);
      assert(
        `M9: Use this folder fills the field with the resolved path (${d.getElementById("ws-add-root").value})`,
        d.getElementById("ws-add-root").value === "/home/e2e/bos" &&
          d.getElementById("ws-add-browser").classList.contains("hidden"),
      );
      // A folder the app cannot list says so beside the browser instead of
      // leaving an empty list that looks like an empty folder.
      LIVE.failNext("list_folder", "/home/e2e/gone: no such file or directory");
      d.getElementById("ws-add-browse-manual").click();
      await waitFor(
        () => d.getElementById("ws-browse-note").textContent.includes("no such file"),
        "the folder failure to be reported",
      );
      assert(
        `M9: a folder that will not list says why beside it (${d.getElementById("ws-browse-note").textContent})`,
        d.getElementById("ws-add-root").value === "/home/e2e/bos",
      );
      d.getElementById("ws-add-browse-manual").click();
      // The switch the dialog carries is the workspace's own: the server that
      // starts on is ticked off here and the one that starts off is ticked on,
      // so the payload cannot be "whatever was inherited" and pass.
      d.querySelector('.ws-add-mcp-on[value="filesystem"]').checked = false;
      d.querySelector('.ws-add-mcp-on[value="notes"]').checked = true;
      d.getElementById("ws-add-name").value = "bos build";
      d.getElementById("ws-add-model").value = "glm-4.7";
      d.getElementById("ws-add-create").click();
      await waitFor(
        () => LIVE.called("create_workspace").length > 0,
        "the creation to reach the backend",
      );
      await waitFor(() => modal.classList.contains("hidden"), "the dialog to close");
      const made = LIVE.settings().workspaces.find((w) => w.name === "bos build");
      const sent = LIVE.lastArgs("create_workspace");
      assert(
        `M9: Add-workspace creates the workspace with the folder, the model and the MCP choice it was given (${JSON.stringify({ root: sent.root, model: sent.config && sent.config.model, mcp: sent.config && sent.config.mcp_enabled })})`,
        !!made &&
          made.root === "/home/e2e/bos" &&
          made.model === "glm-4.7" &&
          (made.mcp_servers || []).find((m) => m.name === "notes").enabled === true &&
          (made.mcp_servers || []).find((m) => m.name === "filesystem").enabled === false &&
          LIVE.settings().active_workspace === made.id,
      );
      assert(
        `M9: the new workspace appears in the sidebar and is the active one (${[...d.querySelectorAll("#session-list .ws-name")].map((n) => n.textContent).join(",")})`,
        LIVE.settings().workspaces.length === workspacesBefore + 1 &&
          [...d.querySelectorAll("#session-list .ws-name")].some((n) => n.textContent === "bos build"),
      );
      // A backend refusal keeps the form open with the folder still filled in, so
      // the next attempt is one field away rather than one panel away.
      LIVE.answerNext("choose_folder", new Error("no folder dialog here: osascript is not installed"));
      d.getElementById("workspace-add-primary").click();
      await waitFor(() => !modal.classList.contains("hidden"), "the form to open again");
      d.getElementById("ws-add-root").value = "/home/e2e/bos";
      d.getElementById("ws-add-name").value = "twice";
      LIVE.failNext("create_workspace", "no MCP server named \"ghost\"");
      d.getElementById("ws-add-create").click();
      await waitFor(
        () => d.getElementById("ws-add-err").textContent.length > 0,
        "the refusal to be shown in the dialog",
      );
      assert(
        `M9: a refused creation keeps the dialog open with its reason (${d.getElementById("ws-add-err").textContent})`,
        !modal.classList.contains("hidden") &&
          d.getElementById("ws-add-err").textContent.includes("ghost") &&
          d.getElementById("ws-add-root").value === "/home/e2e/bos" &&
          LIVE.settings().workspaces.length === workspacesBefore + 1,
      );
      d.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await sleep(20);
      assert(
        `M9: Escape closes Add-workspace (${modal.className.trim()})`,
        modal.classList.contains("hidden"),
      );
    }

    /* ---- attachments: paste, stage, send, store, render, queue --------
       A turn may carry images. The paste path is the one a user's hand makes:
       a clipboard image becomes a chip, the chip rides the send as a data
       URL, the stored turn re-renders it, and a click zooms it in place. */
    await waitFor(
      () => sendBtn.textContent.trim() === "Send",
      "idle before the attachment scenario",
    );
    const sendsBeforeAttach = LIVE.called("send_message").length;
    const PNG_1PX =
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    const pasteImage = () => {
      const file = new File(
        [Uint8Array.from(atob(PNG_1PX), (c) => c.charCodeAt(0))],
        "dot.png",
        { type: "image/png" },
      );
      const dt = new DataTransfer();
      dt.items.add(file);
      const ev = new Event("paste", { bubbles: true });
      Object.defineProperty(ev, "clipboardData", { value: dt });
      input.dispatchEvent(ev);
    };
    const strip = d.getElementById("attach-strip");
    pasteImage();
    await waitFor(() => !strip.classList.contains("hidden"), "the attach chip to render");
    assert(
      `attach: a pasted image becomes a named, removable chip (chips=${
        strip.querySelectorAll(".attach-chip").length})`,
      strip.querySelectorAll(".attach-chip").length === 1 &&
        strip.querySelector(".attach-x").getAttribute("aria-label") ===
          "Remove attached image 1",
    );
    assert(
      `attach: staged images alone enable Send (text=${JSON.stringify(input.value)}, disabled=${
        sendBtn.disabled})`,
      sendBtn.disabled === false && input.value === "",
    );
    strip.querySelector(".attach-x").click();
    assert(
      "attach: the chip's ✕ removes it and hides the strip",
      strip.classList.contains("hidden") && sendBtn.disabled === true,
    );
    // The image-only turn: no text at all, and the backend still gets it.
    pasteImage();
    await waitFor(() => !strip.classList.contains("hidden"), "the chip to come back");
    sendBtn.click();
    await waitFor(
      () => LIVE.called("send_message").length === sendsBeforeAttach + 1,
      "the image turn to send",
    );
    const attachArgs = LIVE.lastArgs("send_message");
    assert(
      `attach: the data URL crosses to the backend (text=${JSON.stringify(attachArgs.text)}, images=${
        (attachArgs.images || []).length})`,
      attachArgs.text === "" &&
        (attachArgs.images || []).length === 1 &&
        /^data:image\/png;base64,/.test(attachArgs.images[0]),
    );
    await waitFor(() => sendBtn.textContent.trim() === "Send", "the image turn to finish");
    // A finished turn re-reads the store: the picture must still be there.
    await waitFor(
      () => d.querySelector("#messages .msg.user .msg-img"),
      "the image bubble to render from the store",
    );
    assert(
      "attach: the stored turn re-renders its image in the user bubble",
      !!d.querySelector("#messages .msg.user .msg-img"),
    );
    const msgImg = d.querySelector("#messages .msg.user .msg-img");
    msgImg.click();
    assert(
      "attach: clicking the image zooms it in place (no navigation)",
      msgImg.classList.contains("zoomed"),
    );
    msgImg.click();
    assert(
      "attach: a second click returns the image to its thumbnail",
      !msgImg.classList.contains("zoomed"),
    );
    // And a queued follow-up keeps its attachment through the wait.
    type("slow with image");
    sendBtn.click();
    await waitFor(() => sendBtn.classList.contains("stop"), "the slow turn to start");
    pasteImage();
    await waitFor(() => !strip.classList.contains("hidden"), "the queued image's chip");
    type("queued with image");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await waitFor(
      () => /1 image/.test(d.getElementById("queue-list").textContent),
      "the queue row to name the image",
    );
    const queuedSends = LIVE.called("send_message").length;
    await waitFor(
      () => LIVE.called("send_message").length === queuedSends + 1,
      "the queued image turn to flush",
    );
    assert(
      `attach: a queued follow-up carries its images (text=${
        LIVE.lastArgs("send_message").text})`,
      LIVE.lastArgs("send_message").text === "queued with image" &&
        (LIVE.lastArgs("send_message").images || []).length === 1,
    );
    await waitFor(
      () => sendBtn.textContent.trim() === "Send",
      "the queued image turn to finish",
    );

    /* ---- hygiene: nothing crashed on the way through ------------------- */
    assert(
      `harness: the bridge answered every call with a real handler (${LIVE.calls.length} calls, ${
        LIVE.errors.length} errors)`,
      LIVE.errors.length === 0,
    );
    assert(
      `harness: the run exercised the whole chat surface (${LIVE.calls.length} bridge calls from ${
        callsAtBoot} at boot)`,
      LIVE.calls.length > callsAtBoot + 20,
    );
  } catch (err) {
    // Fail with the page's own state at the point of failure, so a red run
    // says what the transcript showed and what the bridge had recorded.
    assert(
      `the live harness ran to completion (${err}; errors=${JSON.stringify(
        LIVE.errors,
      )}; rows=${d.querySelectorAll("#messages .msg").length}; dbg=${JSON.stringify(
        window.__DBG,
      )})`,
      false,
    );
  }
  const pre = d.createElement("pre");
  pre.id = "result";
  pre.textContent = JSON.stringify(R);
  d.body.appendChild(pre);
})();
