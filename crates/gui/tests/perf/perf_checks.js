/* What the shipped page costs, counted rather than timed.
 *
 * Every other guard answers "does it work"; none answered "at what price", so
 * the two properties that make this UI feel native were claims in a comment
 * rather than facts: a burst of streamed events costs one frame instead of one
 * repaint per event, and a chat with thousands of messages still draws only a
 * window of them. The runner loads the shipped page with a bridge holding
 * `window.__PERF_SCALE__` chats (one of them very long) and appends this file.
 *
 * Why counts: the runner needs `--virtual-time-budget` to let the bridge's
 * asynchronous pushes land, and under virtual time the clock only advances on
 * timers — a synchronous repaint of 201 rows measures 0.00 ms, which would be a
 * lie about cost. So this guard counts work (frames requested, repaints run,
 * DOM mutations, rows drawn, round trips) and leaves wall-clock milliseconds to
 * `ui_selftest.sh`, which runs a static fixture without virtual time and
 * therefore reports real ones.
 *
 * rAF is replaced with a queue, so "one frame per burst" is counted exactly
 * instead of inferred from wall time.
 */
(() => {
  const checks = {};
  let failures = 0;
  const check = (desc, ok, detail) => {
    checks[detail ? `${desc} — ${detail}` : desc] = ok ? "PASS" : "FAIL";
    if (!ok) failures++;
  };
  const waitFor = (fn, what, ms = 8000) =>
    new Promise((res, rej) => {
      const t0 = Date.now();
      (function tick() {
        let v = null;
        try {
          v = fn();
        } catch (err) {
          v = null;
        }
        if (v) return res(v);
        if (Date.now() - t0 > ms) return rej(new Error(`timed out waiting for ${what}`));
        setTimeout(tick, 25);
      })();
    });
  const emit = (name, payload) => window.__TAURI__.event.emit(name, payload);

  (async () => {
    try {
      const seeded = window.__LIVE__.seeded || { sessions: 0, messages: 0 };
      const activeRow = await waitFor(
        () => document.querySelector("#session-list .session-row.active"),
        "the boot-active chat row"
      );
      await waitFor(() => {
        const input = document.getElementById("input");
        return input && !input.disabled;
      }, "an enabled composer");
      check(
        "boot into a heavy workspace completes and lists it",
        !!activeRow.dataset.id,
        `${seeded.sessions} chats seeded, ${document.querySelectorAll("#session-list .session-row").length} rows drawn`
      );
      check(
        "the burst below lands on the active chat, not a background one",
        activeRow.dataset.id === seeded.active,
        `${activeRow.dataset.id} is active, ${seeded.active} was seeded long`
      );

      const cost = { messages: 0, sidebar: 0, transcript: 0, term: 0 };
      const repaint = window.renderMessages;
      window.renderMessages = function () {
        cost.messages++;
        return repaint.apply(this, arguments);
      };
      // Records are delivered on a microtask, so the counter is drained
      // synchronously with takeRecords() instead of read whenever the browser
      // gets around to the callback (which is how this first measured 0).
      const observers = {};
      const observe = (node, key) => {
        // Both paths must count. Records delivered to the callback are gone from
        // the queue by the time anyone reads it, and records still queued are
        // taken here — so counting only one path silently drops everything that
        // happens across an `await`, which is everything the soak does.
        const mo = new MutationObserver((records) => {
          cost[key] += records.length;
        });
        mo.observe(node, { childList: true, subtree: true, characterData: true, attributes: true });
        observers[key] = mo;
      };
      const mutations = (key) => {
        cost[key] += observers[key].takeRecords().length;
        return cost[key];
      };
      const transcript = document.getElementById("messages");
      observe(transcript, "transcript");
      observe(document.getElementById("session-list"), "sidebar");

      const realRaf = window.requestAnimationFrame.bind(window);
      let frames = [];
      window.requestAnimationFrame = (cb) => {
        frames.push(cb);
        return frames.length;
      };
      const flush = (n) => {
        for (let i = 0; i < n; i++) {
          const due = frames;
          frames = [];
          for (const cb of due) cb();
        }
      };

      /* ---- a burst of streamed deltas costs a frame, not a repaint each ---- */
      const BURST = 400;
      const repaintsBefore = cost.messages;
      const sidebarBefore = mutations("sidebar");
      const mutationsBefore = mutations("transcript");
      for (let i = 0; i < BURST; i++) {
        emit("agent-event", { session_id: seeded.active, kind: "text", text: ` t${i}` });
      }
      // The bridge delivers an event one macrotask later, like a real IPC push,
      // so the burst has to be allowed to land before it means anything.
      await new Promise((res) => setTimeout(res, 60));
      const during = cost.messages - repaintsBefore;
      const queued = frames.length;
      flush(1);
      // Drained after the frame, not before it: a mutation count taken before
      // the repaint measures the burst, which is expected to touch nothing.
      const mutationsDuring = mutations("transcript") - mutationsBefore;
      const afterFrame = cost.messages - repaintsBefore - during;
      check(
        "a 400-event stream burst queues one frame instead of one repaint each",
        during === 0 && queued === 1 && afterFrame === 1,
        `${BURST} events → ${during} rendered during the burst, ${queued} frame, ${afterFrame} repaint on it`
      );
      check(
        "streamed text reaches the drawn row, and cheaply",
        transcript.textContent.includes(" t399") && mutationsDuring > 0 && mutationsDuring <= 60,
        `${mutationsDuring} DOM mutations across ${transcript.children.length} rows, tail visible=${transcript.textContent.includes(" t399")}`
      );
      check(
        "streaming does not rebuild the chat list",
        mutations("sidebar") === sidebarBefore,
        `${mutations("sidebar") - sidebarBefore} mutations of #session-list`
      );

      /* ---- a chatty shell catches up once per frame, and stays bounded - */
      const runPanel = document.getElementById("run-panel");
      const runOutput = document.getElementById("run-output");
      observe(runOutput, "term");
      const runPaint = window.renderRunPanel;
      let runPaints = 0;
      window.renderRunPanel = function () {
        runPaints++;
        return runPaint.apply(this, arguments);
      };
      // Only the panel's own key: an unrelated writer must not turn a claim
      // about shell output into a claim about the whole app.
      let runWrites = 0;
      const realSet = localStorage.setItem.bind(localStorage);
      localStorage.setItem = (k, v) => {
        if (String(k).startsWith("bos.runs.")) runWrites++;
        return realSet(k, v);
      };
      const termId = "bos-pty-" + (state.workspaceRoot || "default");
      document.getElementById("term-open").click();
      await new Promise((res) => setTimeout(res, 60));
      flush(1);
      check(
        "opening a shell attaches to one this panel already owns",
        runPanel.hidden === false && runOutput.textContent.includes("harness shell"),
        `chip="${document.getElementById("run-chip").textContent}", ${state.runPanel.lines.length} lines`
      );
      const FLOOD = 2000;
      const linesBefore = state.runPanel.lines.length;
      const paintsBefore = runPaints;
      const writesBefore = runWrites;
      window.__LIVE__.ptyFlood(termId, FLOOD);
      // The bridge pushes on a macrotask, so the flood has to land first.
      await new Promise((res) => setTimeout(res, 60));
      const duringFlood = runPaints - paintsBefore;
      const queuedTerm = frames.length;
      flush(1);
      const afterFlood = runPaints - paintsBefore - duringFlood;
      check(
        `a ${FLOOD}-line shell flood repaints once, not once per line`,
        duringFlood === 0 && queuedTerm === 1 && afterFlood === 1,
        `${FLOOD} chunks → ${duringFlood} painted during the flood, ${queuedTerm} frame, ${afterFlood} paint on it`
      );
      const drawn = runOutput.querySelectorAll(".run-line").length;
      check(
        "a chatty shell stays inside its line cap, in state and in the DOM",
        state.runPanel.lines.length === 400 && drawn === 400,
        `${state.runPanel.lines.length} lines kept of ${linesBefore + FLOOD}, ${drawn} drawn`
      );
      const termMutations = mutations("term");
      check(
        "the flood's paint costs the same whether it draws 3 lines or 400",
        termMutations > 0 && termMutations <= 8,
        `${termMutations} mutations to draw ${drawn} lines after ${FLOOD} chunks`
      );
      check(
        "shell output never touches localStorage",
        runWrites === writesBefore,
        `${runWrites - writesBefore} bos.runs writes during ${FLOOD} chunks`
      );
      check(
        "the tail the user sees is the newest output, not the oldest",
        runOutput.textContent.includes(`flood ${FLOOD - 1}`) &&
          !runOutput.textContent.includes("flood 0\n"),
        `keeps flood ${FLOOD - 400}..${FLOOD - 1}; line 0 dropped=${
          !runOutput.textContent.includes("flood 0\n")}`
      );

      /* ---- a full repaint still refuses to draw the whole chat ---- */
      const rows = transcript.children.length;
      const fullBefore = mutations("transcript");
      window.markDirtyAll();
      window.renderMessages();
      const fullMutations = mutations("transcript") - fullBefore;
      check(
        "a full repaint keeps the window, and costs nothing for unchanged rows",
        rows > 0 && rows <= 600 && fullMutations <= rows * 4,
        `${rows} rows drawn of ${seeded.messages}, ${fullMutations} DOM mutations to repaint them`
      );

      window.requestAnimationFrame = realRaf;

      /* ---- a soak: a long session must not get slower per turn ---------- */
      // The seeded chat already holds far more messages than the drawn window
      // holds, so the window is saturated before this starts: what is measured
      // is what ten more turns cost, not what the first ones did.
      const SOAK = 10;
      const WINDOW_ROWS = 200;
      // An absolute work budget, not a ratio: a turn into a saturated window
      // costs ~7 mutations (one row in, one out, the caret row, the busy class),
      // so 24 leaves room for a detail or two and still fails loudly on a
      // regression to the ~400 that rebuilding the window cost.
      const TURN_BUDGET = 24;
      const composer = document.getElementById("input");
      const sendBtn = document.getElementById("send");
      const rowsBefore = transcript.children.length;
      // `#messages`' first child is a scroll spacer, not a row, which is why the
      // drawn-message count is the child count minus one.
      const oldestBefore = (transcript.children[1] || { textContent: "" }).textContent;
      const sendsBefore = window.__LIVE__.called("send_message").length;
      const soakCosts = [];
      for (let i = 1; i <= SOAK; i += 1) {
        // The cost is incurred *while the turn streams*, not by the frame after
        // it: by then the rows are already drawn and a repaint finds nothing to
        // do, which is how this first measured a flat zero.
        const before = mutations("transcript");
        composer.value = `soak ${i}`;
        composer.dispatchEvent(new Event("input", { bubbles: true }));
        sendBtn.click();
        await waitFor(
          () => window.__LIVE__.called("send_message").length === sendsBefore + i,
          `soak turn ${i} to reach the bridge`,
        );
        await waitFor(() => !sendBtn.textContent.includes("Stop"), `soak turn ${i} to end`);
        // rAF starves under virtual time, so the frame the app scheduled is run
        // by hand: exactly the work that frame would have done.
        repaint();
        soakCosts.push(mutations("transcript") - before);
      }
      const rowsAfter = transcript.children.length;
      check(
        `a ${SOAK}-turn soak grows the session without growing the drawn window`,
        rowsBefore - 1 === WINDOW_ROWS && rowsAfter - 1 === WINDOW_ROWS && rowsBefore === rowsAfter,
        `${seeded.messages} seeded + ${SOAK * 2} streamed messages → ${rowsBefore - 1} then ${
          rowsAfter - 1} messages drawn (cap ${WINDOW_ROWS}, plus the scroll spacer)`,
      );
      check(
        `the soak's newest turn is drawn while the row that was on top is gone (top was "${
          oldestBefore.trim().slice(0, 24)}")`,
        transcript.textContent.includes(`soak ${SOAK}`) &&
          !transcript.textContent.includes(oldestBefore.trim().slice(0, 24)),
        `newest turn drawn=${transcript.textContent.includes(`soak ${SOAK}`)}`,
      );
      const minCost = Math.min(...soakCosts);
      const maxCost = Math.max(...soakCosts);
      check(
        `a turn into a saturated window stays inside its work budget (${
          soakCosts.join(",")} of ${TURN_BUDGET})`,
        minCost > 0 && maxCost <= TURN_BUDGET,
        `${minCost}..${maxCost} DOM mutations per turn over a ${seeded.messages}-message session`,
      );
      const soakNodes = transcript.querySelectorAll("*").length;
      check(
        "the drawn window stays bounded in nodes as well as in rows",
        // Bounded by the *cap*, not by the rows drawn: a ratio would pass even
        // if the window grew to hold the whole chat, since nodes grow with rows.
        soakNodes <= WINDOW_ROWS * 12,
        `${soakNodes} nodes for ${rowsAfter - 1} messages drawn of ${seeded.messages} (cap ${
          WINDOW_ROWS * 12} nodes)`,
      );
      check(
        `a soaked send costs one round trip per turn, not per chunk (${
          window.__LIVE__.called("send_message").length - sendsBefore} for ${SOAK})`,
        window.__LIVE__.called("send_message").length === sendsBefore + SOAK,
        `${window.__LIVE__.called("send_message").length - sendsBefore} send_message calls for ${SOAK} turns`,
      );

      /* ---- a query costs one round trip, not one per keystroke ---- */
      const reveal = document.getElementById("sessions-search");
      if (reveal && reveal.getAttribute("aria-expanded") === "false") reveal.click();
      const field = document.getElementById("session-search");
      field.value = "budgets";
      field.dispatchEvent(new Event("input", { bubbles: true }));
      await waitFor(() => document.querySelector("#session-list .hit-row"), "search hits");
      const calls = window.__LIVE__.called("search_sessions").length;
      check(
        "one query costs one round trip across every seeded chat",
        calls === 1,
        `${calls} search_sessions call(s) for one input event over ${seeded.sessions} chats`
      );
      check(
        "no harness or page errors were recorded",
        window.__LIVE__.errors.length === 0,
        window.__LIVE__.errors.slice(0, 3).join(" | ") || "none"
      );
    } catch (err) {
      failures++;
      checks[`perf harness ran to the end — ${err && err.message}`] = "FAIL";
    }
    const pre = document.createElement("pre");
    pre.id = "result";
    pre.textContent = JSON.stringify({ checks, failures });
    document.body.appendChild(pre);
  })();
})();
