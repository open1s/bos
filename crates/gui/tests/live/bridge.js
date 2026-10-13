/* The live backend: a stateful, asynchronous double for the Tauri bridge.
 *
 * page_selftest replaces the bridge with settled fixtures, so it proves the
 * shipped page wires handlers to `invoke` — and nothing more. The behaviors
 * that only exist against a *live* backend cannot be seen that way: a turn
 * that streams in over time and ends, a queued follow-up flushing on finish,
 * an approval that must be answered before a tool runs, a failed send rolling
 * back, a save that round-trips. This file is that backend: it holds state
 * across commands, delivers events on timers (real async, deterministic under
 * --virtual-time-budget), writes the transcript store like the Rust side does,
 * and can be told to fail a named command once.
 *
 * Injected ahead of app.js, exactly where page_selftest puts its dead stub.
 * Everything the checks can drive lives on window.__LIVE__.
 */
(() => {
  "use strict";

  const LIVE = {
    calls: [],            // every {cmd, args} that crossed the bridge
    errors: [],           // listener throws, missing handlers, uncaught errors
    failures: [],         // {cmd, message, left} — failNext consumes one each
  };
  window.__LIVE__ = LIVE;

  LIVE.failNext = (cmd, message, count) => {
    LIVE.failures.push({ cmd, message, left: count || 1 });
  };
  /* The system folder panel is a modal: a headless page cannot open one, and the
     double cannot pretend to have shown one either. What it can do is answer the
     way the platform's program answers — a path, a dismissal, or the absence of
     any panel — which is exactly what the app's decisions are about. */
  LIVE.answers = {};
  LIVE.answerNext = (cmd, value) => {
    LIVE.answers[cmd] = value;
  };
  LIVE.called = (cmd) => LIVE.calls.filter((c) => c.cmd === cmd);
  LIVE.sessions = () => copy(store.sessions);
  LIVE.settings = () => copy(store.settings);
  LIVE.messages = (id) => copy(store.cache[id] || []);
  LIVE.lastArgs = (cmd) => {
    const hits = LIVE.called(cmd);
    return hits.length ? hits[hits.length - 1].args : null;
  };

  window.addEventListener("error", (e) => LIVE.errors.push(`error: ${e.message}`));
  window.addEventListener("unhandledrejection", (e) =>
    LIVE.errors.push(`unhandledrejection: ${e.reason}`));

  /* ---------- event delivery ---------- */

  const listeners = {};
  const listen = (name, fn) => {
    (listeners[name] = listeners[name] || new Set()).add(fn);
    return Promise.resolve(() => listeners[name].delete(fn));
  };
  // One macrotask later, like a real IPC push. Timers keep ordering, and
  // --virtual-time-budget fast-forwards them deterministically.
  const emit = (name, payload, delay) => {
    setTimeout(() => {
      for (const fn of listeners[name] || []) {
        try {
          fn({ event: name, payload });
        } catch (err) {
          LIVE.errors.push(`listener ${name}: ${err}`);
        }
      }
    }, delay || 0);
  };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  /* ---------- store ---------- */

  const STEP = 40; // one scripted beat, in virtual milliseconds
  let nextId = 1;
  const nowSec = () => Math.floor(Date.now() / 1000);

  const store = {
    sessions: [],        // {id, title, updated_at, workspace}
    cache: {},           // id -> [ChatMessage]
    streams: {},         // id -> {session_id, gen, started_at}
    cancelled: {},       // id -> true while a stop was requested
    approvals: {},       // approval id -> {session_id, resolve}
    gen: {},
    goal: {},            // id -> goal object
    plan: {},            // id -> [{text, status}]
    pty: {},             // sessionId -> {shell, tail} — a shell that outlives the panel
    jobs: [],            // [{id,label,kind,state,started_at,finished_at,bytes,dropped_bytes,error,body}]
    spills: [],          // [{id,file,bytes,preview,body}]
    compact: {},         // sessionId -> the live/folded split the Context pane reports
    memories: [],
    knowledge: [],          // curated notes, as the Knowledge pane holds them
    skills: [],             // SKILL.md folders, as the skills pane holds them
    kbSeq: 0,
    schedules: [],          // durable reminders, as the Schedule pane holds them
    scheduleHistory: [],    // deliveries, newest first
    scheduleSeq: 0,
    settings: null,
    trash: [],
    titles: {},          // id -> title, kept through delete so the trash can restore
  };

  // One source of truth for the scoped rules: the Rules pane, the agent's
  // self-inspection, and the approval path must not be able to disagree about
  // what is in force or which rule decided a call.
  const rulesState = { rules: [], warnings: [], audit_cap: 200, audit: [] };

  const rulesReport = () => ({
    rules: copy(rulesState.rules),
    warnings: copy(rulesState.warnings),
    audit_cap: rulesState.audit_cap,
  });

  const recordDecision = (tool, args, decision, rule) => {
    rulesState.audit.unshift({
      at: Math.floor(Date.now() / 1000),
      tool,
      args: String(args ?? "").slice(0, 120),
      decision,
      ...(rule ? { rule_id: rule.id, scope: rule.scope } : {}),
      session: "",
    });
    if (rulesState.audit.length > rulesState.audit_cap) {
      rulesState.audit.length = rulesState.audit_cap;
    }
  };

  const globMatch = (pattern, name) =>
    new RegExp(
      `^${
        String(pattern)
          .replace(/[.+^${}()|[\]\\]/g, "\\$&")
          .replace(/\*/g, ".*")
          .replace(/\?/g, ".")
      }$`,
    ).test(name);

  const argMatches = (pred, args) => {
    let parsed = {};
    try {
      parsed = typeof args === "string" ? JSON.parse(args) : args || {};
    } catch {
      return false;
    }
    const value = String(parsed[pred.key] ?? "");
    if (pred.match_kind === "exact") return value === pred.value;
    if (pred.match_kind === "prefix") return value.startsWith(pred.value);
    if (pred.match_kind === "contains") return value.includes(pred.value);
    return false;
  };

  // The rule book consults rules before a user is asked, exactly as the backend
  // does, and every decision lands in the ring whether or not a rule matched.
  const decideCall = (tool, args) => {
    for (const rule of rulesState.rules) {
      if (!globMatch(rule.tool, tool)) continue;
      if (rule.arg && !argMatches(rule.arg, args)) continue;
      recordDecision(tool, args, rule.effect, rule);
      return rule.effect;
    }
    recordDecision(tool, args, "ask", null);
    return "ask";
  };

  const jobRecord = (j) => ({
    id: j.id,
    label: j.label,
    kind: j.kind,
    state: j.state,
    started_at: j.started_at,
    finished_at: j.finished_at ?? null,
    bytes: j.body.length,
    dropped_bytes: j.dropped_bytes ?? 0,
    error: j.error ?? null,
  });

  const compactFor = (sessionId) =>
    store.compact[sessionId] ||
    (store.compact[sessionId] = {
      session_id: sessionId,
      title: "Design the streaming harness",
      live_messages: 3,
      archived_messages: 1,
      budget_tokens: 8192,
      compacting: false,
      can_compact: true,
    });

  const compactStatus = (sessionId) => {
    const c = compactFor(sessionId);
    const estimated_tokens = c.live_messages * 30;
    return {
      ...c,
      messages: c.live_messages + c.archived_messages,
      estimated_tokens,
      over_budget: estimated_tokens > c.budget_tokens,
      refusal: null,
    };
  };

  const mkSession = (title, msgs, workspace = "w-main") => {
    const s = {
      id: `live-${nextId++}`,
      title,
      updated_at: nowSec(),
      workspace,
      pinned: false,
      archived: false,
    };
    store.sessions.unshift(s);
    store.cache[s.id] = msgs;
    return s;
  };

  // The list the sidebar reads: the record minus its messages, which is what
  // the Rust `SessionSummary` carries.
  const summaryOf = (s) => ({
    id: s.id,
    title: s.title,
    updated_at: s.updated_at,
    workspace: s.workspace,
    pinned: !!s.pinned,
    archived: !!s.archived,
  });

  const toolRow = (name, path, output, ms) => ({
    name,
    args: JSON.stringify({ path }),
    output,
    ms,
  });

  /* A scale hook for the performance guard: the runner sets
     `window.__PERF_SCALE__` before this file loads, so the app boots into a
     heavy workspace — many chats, one very long transcript — and the budget is
     measured on the shipped page instead of on a fixture. The newest chat is
     unshifted, so it is list[0] and takes the active spot at boot, which is how
     the checks know the burst below reaches the hot path. */
  const seedScale = ({ sessions = 0, messages = 0 } = {}) => {
    const long = [];
    for (let i = 0; i < messages; i++) {
      const assistant = i % 2 === 1;
      long.push({
        role: assistant ? "Assistant" : "User",
        text: `${assistant ? "Answer" : "Question"} ${i}: ` +
          "reasoning about the harness and its budgets. ".repeat(3),
        reasoning: assistant && i % 6 === 1 ? "weigh the options" : "",
        tools: assistant && i % 10 === 1
          ? [toolRow("read_file", "crates/gui/ui/app.js", "…", 4)]
          : [],
        error: null,
      });
    }
    for (let i = 1; i < sessions; i++) mkSession(`Perf chat ${i}`, []);
    const big = mkSession("Perf baseline", long); // last, so it is list[0]
    return { active: big.id, sessions: store.sessions.length, messages: long.length };
  };
  LIVE.seedScale = seedScale;

  // A chatty shell: n output events pushed like the real backend's, so the
  // panel's per-frame catch-up and its line cap can be counted, not inferred.
  LIVE.ptyFlood = (sessionId, n) => {
    for (let i = 0; i < n; i++) {
      emit("pty-output", { session_id: sessionId, text: `flood ${i}\n` });
    }
  };

  // mkSession unshifts (a new chat is the newest), so seed newest-last: the
  // design session must be list[0] and take the active spot at boot.
  const sEmpty = mkSession("Empty notebook", []);
  const s1 = mkSession("Design the streaming harness", [
    { role: "User", text: "Design the streaming harness", reasoning: "", tools: [], error: null },
    {
      role: "Assistant",
      text: "Drafting the plan now.",
      reasoning: "reading the plan first",
      tools: [toolRow("read_file", "plan.md", "read 120 bytes", 42)],
      error: null,
    },
    { role: "User", text: "Go ahead", reasoning: "", tools: [], error: null },
    { role: "Assistant", text: "The harness design is ready.", reasoning: "", tools: [], error: null },
  ]);

  // A second session with its own plan, so a session switch has to restore
  // *that* chat's plan rather than keep showing the one on screen.
  store.plan[sEmpty.id] = [{ text: "empty-chat step one", status: "pending" }];
  // M7/M6: a running job whose output is far larger than one read window (so
  // the tail window and its dropped bytes are real), a job that already failed,
  // and a spill larger than the pane's own cap.
  store.jobs = [
    {
      id: "job-1",
      label: "cargo test --workspace",
      kind: "run",
      state: "running",
      started_at: 1700000000,
      finished_at: null,
      body: Array.from({ length: 2000 }, (_, i) => `test line ${i}`).join("\n"),
    },
    {
      id: "job-2",
      label: "npm run lint",
      kind: "shell",
      state: "failed",
      started_at: 1700000000,
      finished_at: 1700000005,
      error: "exit 1",
      body: "boom",
    },
  ];
  store.spills = [
    {
      id: "spill-1",
      file: "/tmp/live/spill/spill-1.txt",
      bytes: 20000,
      preview: "lots of output",
      body: Array.from({ length: 1200 }, (_, i) => `spill line ${i}`).join("\n"),
    },
  ];
  store.plan[s1.id] = [
    { text: "wire the live harness", status: "completed" },
    { text: "prove the stream end to end", status: "pending" },
  ];
  store.goal[s1.id] = {
    objective: "ship the streaming harness",
    state: "active",
    rounds: 3,
    max_rounds: 12,
    blocked_reason: null,
  };
  const DEFAULT_MEMORY_FILE = "/home/live/.bos/gui/memory.jsonl";
  store.memories = [
    { id: "m1", content: "remembers the streaming harness", created_at_ms: Date.now() - 60000 },
  ];
  store.settings = {
    model: "glm-4.6",
    base_url: "https://api.example.com/v1",
    api_key: "sk-live-harness",
    system_prompt: "",
    project_instructions: true,
    temperature: 0.7,
    reasoning_effort: "medium",
    bash_enabled: true,
    file_tools_enabled: true,
    require_approval: true,
    allowed_tools: [],
    bash_workspace: "/tmp/live",
    skills_dir: "~/.bos/skills",
    memory_enabled: true,
    memory_path: "",
    context_budget: 32768,
    web_tools_enabled: true,
    search_api_url: "",
    search_api_key: "",
    mcp_servers: [
      { name: "filesystem", transport: "stdio", command: "mcp-filesystem", url: "", enabled: true },
    ],
    providers: [],
    workspaces: [
      {
        id: "w-main", name: "Main", root: "/tmp/live", model: "",
        mcp_servers: [
          { name: "filesystem", transport: "stdio", command: "mcp-filesystem", url: "", enabled: true },
          { name: "notes", transport: "stdio", command: "mcp-notes", url: "", enabled: false },
        ],
      },
      { id: "w-second", name: "Second", root: "/tmp/second", model: "", mcp_servers: [] },
    ],
    active_workspace: "w-main",
    presets: [],
  };

  const overviewOf = (id) => {
    const msgs = store.cache[id] || [];
    let tools = 0;
    let chars = 0;
    for (const m of msgs) {
      tools += (m.tools || []).length;
      chars += (m.text || "").length;
    }
    return { messages: msgs.length, tool_calls: tools, characters: chars };
  };

  /* ---------- streamed turns ---------- */

  // Scenario choice rides on the message text so a check reads like its intent:
  // "write …" raises an approval, "slow …" streams long enough to queue or stop.
  const scenarioOf = (text) =>
    /\bwrite\b/.test(text)
      ? "approval"
      : /\bslow\b/.test(text)
        ? "long"
        : /\bplan\b/.test(text)
          ? "plan"
          : "plain";

  const replyFor = (text) =>
    `Hello, world — from the live harness. (you said: ${text})`;

  function scriptTurn(sessionId, userText) {
    const gen = (store.gen[sessionId] = (store.gen[sessionId] || 0) + 1);
    const stream = { session_id: sessionId, gen, started_at: Date.now() };
    store.streams[sessionId] = stream;
    delete store.cancelled[sessionId];

    const acc = { text: "", reasoning: "", tools: [], error: null };
    const ev = (payload, delay) =>
      emit("agent-event", { session_id: sessionId, ...payload }, delay);

    const finish = (extra) => {
      delete store.streams[sessionId];
      const msgs = store.cache[sessionId] || (store.cache[sessionId] = []);
      // A real backend finalizes the reply — the provider's own text, artifacts
      // stripped — so the stored turn can differ from the streamed draft. The UI
      // must render what the store handed back, not what it happened to stream.
      const stored = /finalize/.test(userText)
        ? `Final answer, finalized by the store. (you said: ${userText})`
        : acc.text;
      msgs.push({
        role: "Assistant",
        text: stored,
        reasoning: acc.reasoning,
        tools: acc.tools,
        error: acc.error,
      });
      emit("stream-finished", {
        session_id: sessionId,
        gen,
        prompt_tokens: 120,
        completion_tokens: 45,
        // 80% of the prompt served from the provider's cache; the foot states it.
        cache_read_tokens: 96,
        cancelled: !!(extra && extra.cancelled),
        error: acc.error || null,
      }, STEP);
    };

    // 5.3: a turn the backend cancelled on its own, with no press for the client to
    // mistake for the source. The foot has to say Stopped from the payload alone.
    // (isolation: the servercancel branch is removed)

    (async () => {
      if (scenarioOf(userText) === "approval") {
        await sleep(STEP);
        if (store.cancelled[sessionId]) return finish({ cancelled: true });
        const tool = toolRow("write_file", "notes/a.txt", null, null);
        ev({ kind: "tool", name: tool.name, args: tool.args });
        acc.tools.push(tool);
        const verdict = decideCall("write_file", tool.args);
        if (verdict !== "ask") {
          tool.output = verdict === "allow" ? "wrote 2 bytes" : "denied by a rule";
          tool.ms = 7;
          await sleep(STEP);
          ev({ kind: "tool_result", name: tool.name, output: tool.output, ms: tool.ms });
          acc.text =
            verdict === "allow"
              ? "Done — notes/a.txt is written."
              : "Refused — a rule denies write_file.";
          await sleep(STEP);
          ev({ kind: "text", text: acc.text });
          await sleep(STEP);
          ev({ kind: "usage", prompt: 120, completion: 45 });
          return finish({});
        }
        const aprId = `apr-${nextId++}`;
        await sleep(STEP);
        if (store.cancelled[sessionId]) return finish({ cancelled: true });
        emit(
          "approval-request",
          {
            id: aprId,
            session_id: sessionId,
            tool: "write_file",
            args: tool.args,
            diff: "--- /dev/null\n+++ notes/a.txt\n+live harness was here\n",
          },
          STEP,
        );
        const decision = await new Promise((resolve) => {
          store.approvals[aprId] = { session_id: sessionId, resolve };
          // An unanswered approval dies with its stream, like the backend's.
          setTimeout(() => {
            if (store.approvals[aprId]) {
              delete store.approvals[aprId];
              resolve({ approved: false, timeout: true });
            }
          }, 8000);
        });
        delete store.approvals[aprId];
        tool.output = decision.approved ? "wrote 2 bytes" : "denied by the user";
        tool.ms = 12;
        ev({ kind: "tool_result", name: tool.name, output: tool.output, ms: tool.ms });
        acc.text = decision.approved
          ? "Done — notes/a.txt is written."
          : "Understood — I left the file alone.";
        await sleep(STEP);
        ev({ kind: "text", text: acc.text });
        await sleep(STEP);
        ev({ kind: "usage", prompt: 120, completion: 45 });
        return finish({});
      }

      /* The agent calls update_plan mid-turn. The UI never receives the plan
         itself, only a tool event, so what this exercises is the re-pull. */
      if (scenarioOf(userText) === "plan") {
        await sleep(STEP);
        ev({ kind: "text", text: "Working out the steps. " });
        acc.text += "Working out the steps. ";
        const steps = [
          { text: "Read the harness", status: "completed" },
          { text: "Seed the long chat", status: "in_progress" },
          { text: "Measure the burst", status: "pending" },
        ];
        store.plan[sessionId] = steps;
        await sleep(STEP);
        if (store.cancelled[sessionId]) return finish({ cancelled: true });
        ev({ kind: "tool", name: "update_plan", args: JSON.stringify({ items: steps }) });
        acc.tools.push(toolRow("update_plan", "items", "3 steps", 7));
        await sleep(STEP);
        ev({ kind: "text", text: "Plan updated." });
        acc.text += "Plan updated.";
        await sleep(STEP);
        ev({ kind: "usage", prompt: 120, completion: 45 });
        return finish({});
      }

      if (scenarioOf(userText) === "long") {
        ev({ kind: "reasoning", text: "settling in for a long turn" }, STEP);
        acc.reasoning = "settling in for a long turn";
        for (let i = 0; i < 12; i++) {
          await sleep(60);
          if (store.cancelled[sessionId]) {
            acc.text += ` chunk ${i}`;
            return finish({ cancelled: true });
          }
          ev({ kind: "text", text: ` chunk ${i}` });
          acc.text += ` chunk ${i}`;
        }
        await sleep(STEP);
        ev({ kind: "usage", prompt: 120, completion: 45 });
        return finish({});
      }

      // plain: reasoning → text in two chunks → usage → finish.
      const reply = replyFor(userText);
      const mid = Math.ceil(reply.length / 2);
      await sleep(STEP);
      ev({ kind: "reasoning", text: "composing the reply" });
      acc.reasoning = "composing the reply";
      await sleep(STEP);
      ev({ kind: "text", text: reply.slice(0, mid) });
      acc.text += reply.slice(0, mid);
      await sleep(STEP);
      ev({ kind: "text", text: reply.slice(mid) });
      acc.text += reply.slice(mid);
      await sleep(STEP);
      ev({ kind: "usage", prompt: 120, completion: 45 });
      return finish({});
    })();

    return stream;
  }

  /* ---------- authored skills ----------
     The double writes into a directory it holds in memory and reads back
     through the same rules the loader applies (name and description are the
     agent's, the body and the path are the GUI's), so a check that reads the
     pane is reading the round trip the backend performs. */
  store.skillsDir = "/home/e2e/.bos/skills";
  store.settingsDisabledSkills = [];
  /* Which chat switched which skill off. A chat with no key here follows the
     workspace, which is what makes the per-chat list a scope and not a copy. */
  store.skillActivation = {};
  store.skills = [
    { name: "code-review", description: "Review a diff", body: "Read the diff, then list findings." },
  ];
  const SKILL_MAX_BODY = 12000;
  const SKILL_MAX_NAME = 128;
  const SKILL_MAX_DESC = 1024;
  const skillSlug = (name) => {
    const trimmed = String(name || "").trim();
    if (!trimmed || trimmed === "." || trimmed === "..") return null;
    if (trimmed.includes("/") || trimmed.includes("\\") || trimmed.includes("\0")) return null;
    return trimmed;
  };
  const skillErrors = (name, description, body) => {
    const errors = [];
    const trimmedName = String(name || "").trim();
    const trimmedDesc = String(description || "").trim();
    const trimmedBody = String(body || "").trim();
    if (!trimmedName) errors.push("name: must not be empty");
    else if (trimmedName.length > SKILL_MAX_NAME) {
      errors.push(`name: must be at most ${SKILL_MAX_NAME} characters`);
    } else if (/\s/.test(trimmedName)) errors.push("name: must not contain whitespace");
    if (!trimmedDesc) errors.push("description: must not be empty");
    else if (trimmedDesc.length > SKILL_MAX_DESC) {
      errors.push(`description: must be at most ${SKILL_MAX_DESC} characters`);
    }
    if (!trimmedBody) errors.push("body: must not be empty");
    else if (trimmedBody.length > SKILL_MAX_BODY) {
      errors.push(`body: must be at most ${SKILL_MAX_BODY} characters`);
    }
    if (!skillSlug(trimmedName)) {
      errors.push("name: must be one path segment (no / or \\, and not . or ..)");
    }
    return errors;
  };
  const skillDenied = (name) => store.settingsDisabledSkills.includes(name);
  const skillReport = (nowMs) => ({
    now_ms: nowMs,
    dir: store.skillsDir,
    exists: true,
    skills: store.skills
      .map((skill) => ({
        name: skill.name,
        description: skill.description,
        path: `${store.skillsDir}/${skill.name}/SKILL.md`,
        instructions_chars: skill.body.trim().length,
        denied: skillDenied(skill.name),
      }))
      .sort((a, b) => (a.name < b.name ? -1 : 1)),
    max_body_chars: SKILL_MAX_BODY,
    max_name_chars: SKILL_MAX_NAME,
    max_description_chars: SKILL_MAX_DESC,
  });

  const skillAvailable = () =>
    store.skills
      .filter((skill) => !skillDenied(skill.name))
      .map((skill) => skill.name)
      .sort();
  const skillScope = (session) => {
    const available = skillAvailable();
    const record = store.skillActivation[String(session || "")];
    if (!record) return { explicit: false, active: available };
    return { explicit: true, active: available.filter((name) => record.includes(name)) };
  };
  const skillActivationReport = (session, nowMs) => {
    const scope = skillScope(session);
    return {
      now_ms: nowMs,
      session: String(session || ""),
      explicit: scope.explicit,
      active: scope.active,
      available: skillAvailable(),
      denied: store.skills.map((s) => s.name).filter(skillDenied).sort(),
      path: "/home/e2e/.bos/gui/skill-activation.json",
      sessions_known: Object.keys(store.skillActivation).length,
      max_sessions: 500,
    };
  };

  /* ---------- what the MCP servers publish ----------
     The double answers the catalog the way the backend does: a capability the
     server declares as `false` is never listed (and its total is zero, not the
     count of rows it happens to hold), a body longer than the pane's cap comes
     back cut with both numbers, and a URI it does not hold is an error like any
     other. */
  const VIEW_MAX_TEXT = 20000;
  const viewRows = (list) => (Array.isArray(list) ? list : []);
  const viewResource = (row) => ({
    uri: row.uri,
    name: row.name || row.uri,
    description: row.description || "",
    mime_type: row.mime_type || null,
    clipped: false,
  });
  const viewPrompt = (row) => {
    const list = Array.isArray(row.arguments) ? row.arguments : [];
    return {
      name: row.name,
      description: row.description || "",
      arguments: list.length,
      required: list.filter((a) => a && a.required === true).length,
      clipped: false,
    };
  };
  const viewServer = (server) => {
    const resources_supported = !!server.resources;
    const prompts_supported = !!server.prompts;
    return {
      server: server.name,
      connected: !!server.connected,
      resources_supported,
      prompts_supported,
      resources: resources_supported ? viewRows(server.resources).map(viewResource) : [],
      prompts: prompts_supported ? viewRows(server.prompts).map(viewPrompt) : [],
      resources_total: resources_supported ? viewRows(server.resources).length : 0,
      prompts_total: prompts_supported ? viewRows(server.prompts).length : 0,
      error: null,
    };
  };
  const catalogReport = (nowMs) => ({
    now_ms: nowMs || 0,
    servers: store.mcpServers.map(viewServer),
    max_rows: 200,
    max_text_chars: VIEW_MAX_TEXT,
    max_desc_chars: 240,
  });
  const readReport = (server, uri, nowMs) => {
    if (!store.mcpServers.some((s) => s.name === server && s.connected)) {
      throw new Error(`no connected MCP server is named ${JSON.stringify(server)}`);
    }
    if (!Object.prototype.hasOwnProperty.call(store.mcpBodies, uri)) {
      throw new Error(`resources/read: no resource at ${uri}`);
    }
    const body = store.mcpBodies[uri];
    const full = body.text || "";
    const text = full.slice(0, VIEW_MAX_TEXT);
    return {
      now_ms: nowMs || 0,
      server,
      uri,
      mime_type: body.mime_type || null,
      text,
      chars: text.length,
      source_chars: full.length,
      truncated: full.length > text.length,
      parts: 1,
      empty: full.length === 0,
    };
  };

  /* ---------- command handlers ---------- */

  const copy = (x) => JSON.parse(JSON.stringify(x));

  /* ---------- the MCP per-tool policy ----------
     The double keeps the same precedence the backend enforces (deny > ask >
     allow, `*` on either half, the strictest rule wins, then the default) so a
     check that reads the pane is reading a real decision. */
  store.mcpServers = [
    {
      name: "filesystem", transport: "stdio", enabled: true, connected: true,
      tools: ["read", "write", "list"],
      resources: [
        { uri: "file:///repo/schema.json", name: "schema", mime_type: "application/json" },
        { uri: "file:///repo/big.log", name: "big", mime_type: "text/plain" },
      ],
      prompts: [
        {
          name: "review",
          description: "Review a diff",
          arguments: [{ name: "diff", required: true }, { name: "style" }],
        },
      ],
    },
  ];
  // Bodies are held per URI, and one URI is deliberately longer than the pane
  // is allowed to draw, so the truncation a read reports is the backend's own.
  store.mcpBodies = {
    "file:///repo/schema.json": { text: '{"type":"object"}', mime_type: "application/json" },
    "file:///repo/big.log": { text: "x".repeat(30000), mime_type: "text/plain" },
    "file:///repo/empty.bin": { text: null, mime_type: "application/octet-stream" },
  };
  store.mcpPolicy = { default: "allow", rules: [] };
  const POLICY_MAX_RULES = 200;
  const policyRank = (e) => (e === "deny" ? 3 : e === "ask" ? 2 : 1);
  const policyHit = (pattern, name) => pattern === "*" || pattern === name;
  const policyDecide = (server, tool) => {
    let best = null;
    for (const rule of store.mcpPolicy.rules) {
      if (!policyHit(rule.server, server) || !policyHit(rule.tool, tool)) continue;
      if (!best || policyRank(rule.effect) > policyRank(best.effect)) best = rule;
    }
    return best
      ? { effect: best.effect, by: `${best.server}/${best.tool}` }
      : { effect: store.mcpPolicy.default, by: "default" };
  };
  const policyAttached = (server) =>
    server.tools.filter((tool) => policyDecide(server.name, tool).effect !== "deny");
  const policyReport = (nowMs) => {
    const servers = store.mcpServers.map((server) => {
      const listed = server.tools.map((tool) => {
        const decision = policyDecide(server.name, tool);
        return { name: tool, effect: decision.effect, by: decision.by };
      });
      const attached = listed.filter((t) => t.effect !== "deny").length;
      return { name: server.name, tools: listed, attached, denied: listed.length - attached };
    });
    return {
      now_ms: nowMs,
      default: store.mcpPolicy.default,
      rules: copy(store.mcpPolicy.rules),
      max_rules: POLICY_MAX_RULES,
      max_name_chars: 64,
      servers,
      attached: servers.reduce((n, s) => n + s.attached, 0),
      denied: servers.reduce((n, s) => n + s.denied, 0),
      asked: servers.reduce(
        (n, s) => n + s.tools.filter((t) => t.effect === "ask").length,
        0,
      ),
      file_version: 1,
      expected_version: 1,
    };
  };
  const policySet = (server, tool, effect) => {
    const hit = store.mcpPolicy.rules.find((r) => r.server === server && r.tool === tool);
    if (hit) hit.effect = effect;
    else store.mcpPolicy.rules.push({ server, tool, effect });
    return policyReport(Date.now());
  };

  /* Applied here, after the bridge's own seeds, so the long chat is the newest
     of all and takes the active spot at boot. */
  if (window.__PERF_SCALE__) LIVE.seeded = seedScale(window.__PERF_SCALE__);

  /* ---------- reminders ----------
     The scheduler's rules, mirrored from crates/gui/src/schedule.rs so the
     checks below are about the UI rather than about this double: a repeat needs
     at least a minute, one delivery per reminder per tick however long the gap,
     and a one-shot leaves the list once it has been delivered. */
  const SCHED_MIN_EVERY = 1;
  const SCHED_MAX = 50;
  const SCHED_HISTORY = 200;
  const SCHED_MAX_TEXT = 500;

  const scheduleDue = (cadence) => (cadence.kind === "once" ? cadence.at_ms : cadence.next_ms);

  const scheduleReport = (nowMs, fired) => ({
    now_ms: nowMs,
    schedules: copy(store.schedules),
    history: copy(store.scheduleHistory),
    fired: copy(fired || []),
    caps: {
      min_every_minutes: SCHED_MIN_EVERY,
      max_schedules: SCHED_MAX,
      max_history: SCHED_HISTORY,
      max_text_chars: SCHED_MAX_TEXT,
    },
  });

  const tickSchedules = (nowMs) => {
    const fired = [];
    const done = [];
    for (const schedule of store.schedules) {
      const due = scheduleDue(schedule.cadence);
      if (due > nowMs) continue;
      fired.push({
        schedule_id: schedule.id,
        text: schedule.text,
        at_ms: due,
        noticed_ms: nowMs,
      });
      schedule.last_fired_ms = due;
      schedule.fired += 1;
      if (schedule.cadence.kind === "once") {
        done.push(schedule);
      } else {
        const step = schedule.cadence.every_minutes * 60000;
        const missed = Math.floor((nowMs - schedule.cadence.next_ms) / step) + 1;
        schedule.cadence.next_ms += missed * step;
      }
    }
    store.schedules = store.schedules.filter((s) => !done.includes(s));
    for (const delivery of fired.slice().reverse()) store.scheduleHistory.unshift(delivery);
    if (store.scheduleHistory.length > SCHED_HISTORY) {
      store.scheduleHistory.length = SCHED_HISTORY;
    }
    return fired;
  };

  /* ---------- knowledge ----------
     The store's rules, mirrored from crates/gui/src/knowledge.rs: a title is
     required and bounded, tags are normalized, a title hit outranks a tag hit
     which outranks a body hit, and an empty query lists everything newest
     first. */
  const KB_MAX = 500;
  const KB_MAX_TITLE = 120;
  const KB_MAX_BODY = 4000;
  const KB_MAX_TAGS = 8;

  const kbTags = (raw) => {
    const tags = [];
    for (const piece of String(raw || "").split(",")) {
      const tag = piece.trim().toLowerCase();
      if (!tag || tags.includes(tag)) continue;
      if (tag.length > 32) throw new Error("a tag is at most 32 characters");
      tags.push(tag);
    }
    if (tags.length > KB_MAX_TAGS) throw new Error(`at most ${KB_MAX_TAGS} tags per entry`);
    return tags;
  };

  const kbClean = (title, body) => {
    const cleanTitle = String(title || "").trim();
    if (!cleanTitle) throw new Error("a knowledge entry needs a title");
    if (cleanTitle.length > KB_MAX_TITLE) {
      throw new Error(`a title is at most ${KB_MAX_TITLE} characters`);
    }
    const cleanBody = String(body || "").trim();
    if (cleanBody.length > KB_MAX_BODY) {
      throw new Error(`a body is at most ${KB_MAX_BODY} characters`);
    }
    return { title: cleanTitle, body: cleanBody };
  };

  const kbScore = (entry, query) => {
    const tokens = String(query || "")
      .toLowerCase()
      .split(/[^a-z0-9_\-/]+/)
      .filter(Boolean);
    if (!tokens.length) return 0;
    const title = entry.title.toLowerCase();
    const tags = entry.tags.join(" ");
    const body = entry.body.toLowerCase();
    let score = 0;
    for (const token of tokens) {
      if (title.includes(token)) score += 4;
      if (tags.includes(token)) score += 3;
      if (body.includes(token)) score += 1;
    }
    return score;
  };

  const kbMatches = (query, limit) => {
    const hits = store.knowledge
      .map((entry) => ({ entry, score: kbScore(entry, query) }))
      .filter(({ score }) => !String(query || "").trim() || score > 0)
      .sort(
        (a, b) =>
          b.score - a.score ||
          b.entry.updated_ms - a.entry.updated_ms ||
          a.entry.id.localeCompare(b.entry.id),
      )
      .map(({ entry }) => entry);
    return limit == null ? hits : hits.slice(0, limit);
  };

  const knowledgeReport = (query, limit, nowMs) => ({
    now_ms: nowMs || 0,
    query: String(query || ""),
    total: store.knowledge.length,
    matches: copy(kbMatches(query, limit)),
    caps: {
      max_entries: KB_MAX,
      max_title_chars: KB_MAX_TITLE,
      max_body_chars: KB_MAX_BODY,
      max_tags: KB_MAX_TAGS,
      max_tag_chars: 32,
      tool_body_chars: 1200,
      tool_max_limit: 20,
    },
  });

  /* The workspace the Files section walks: one folder holding one file, keyed by
     path, so expanding the folder answers with *that* folder's children rather
     than the root's again — a fixture with one level cannot tell the two apart. */
  const workspaceFiles = {
    "": {
      entries: [
        { name: "src", path: "src", is_dir: true, bytes: 0 },
        { name: "README.md", path: "README.md", is_dir: false, bytes: 21 },
      ],
      truncated: false,
    },
    src: {
      entries: [{ name: "main.rs", path: "src/main.rs", is_dir: false, bytes: 10 }],
      truncated: false,
    },
  };
  const workspaceText = {
    "README.md": "# bos notes\n",
    "src/main.rs": "fn main()\n",
  };

  const handlers = {
    set_message_feedback: () => null,
    list_sessions: () =>
      copy(store.sessions)
        .sort((a, b) => b.updated_at - a.updated_at)
        .map(summaryOf),
    // The chat lands in the workspace the user is in, because the real command does.
    // A create that hard-codes the main workspace is a chat the page will not show
    // while another one is active - correctly, a transcript of one workspace must not
    // sit under another - and the reader is left with an empty panel beside a turn
    // that did run.
    create_session: () =>
      summaryOf(mkSession("New chat", [], store.settings.active_workspace || "w-main")),
    // The three the sidebar's row menu reaches. They answer with the summary the
    // Rust command answers with, so the page redraws from the backend's word
    // rather than from what it hoped happened.
    rename_session: ({ id, title }) => {
      const s = store.sessions.find((x) => x.id === id);
      if (!s) throw new Error("no such chat");
      const name = String(title == null ? "" : title).trim();
      if (!name) throw new Error("a chat needs a name");
      if ([...name].length > 80) throw new Error("a chat name is at most 80 characters");
      s.title = name;
      s.updated_at = nowSec();
      store.titles[id] = name;
      return summaryOf(s);
    },
    set_session_pinned: ({ id, pinned }) => {
      const s = store.sessions.find((x) => x.id === id);
      if (!s) throw new Error("no such chat");
      s.pinned = !!pinned;
      return summaryOf(s);
    },
    set_session_archived: ({ id, archived }) => {
      const s = store.sessions.find((x) => x.id === id);
      if (!s) throw new Error("no such chat");
      s.archived = !!archived;
      if (!s.archived) s.updated_at = nowSec();
      return summaryOf(s);
    },
    load_session: ({ id }) => copy(store.cache[id] || []),
    // The page logs through a command of its own (logf → ui_log → ~/.bos/log).
    // There is no file to write here, but a command the page calls with no
    // handler is recorded as a hygiene error, so this one has to answer.
    ui_log: () => null,
    delete_session: ({ id }) => {
      // The trash names the chat back on restore, so keep its title.
      const s = store.sessions.find((x) => x.id === id);
      if (s) store.titles[id] = s.title;
      store.sessions = store.sessions.filter((x) => x.id !== id);
      store.trash.unshift({ id, title: store.titles[id] || "" });
      delete store.cache[id];
      delete store.gen[id];
      return { id };
    },
    trashed_chats: () => copy(store.trash),
    restore_session: ({ id }) => {
      const i = store.trash.findIndex((t) => t.id === id);
      if (i < 0) throw new Error("not in the trash");
      const { title } = store.trash[i];
      store.trash.splice(i, 1);
      store.sessions.unshift({
        id,
        title: title || "Restored chat",
        updated_at: nowSec(),
        workspace: "w-main",
      });
      store.cache[id] = store.cache[id] || [];
      return title || "Restored chat";
    },
    purge_trash: () => {
      const n = store.trash.length;
      store.trash = [];
      return n;
    },
    send_message: ({ sessionId, text, images, truncateFrom }) => {
      if (store.streams[sessionId]) throw new Error("already streaming");
      const msgs = store.cache[sessionId] || (store.cache[sessionId] = []);
      if (typeof truncateFrom === "number") msgs.splice(truncateFrom);
      msgs.push({
        role: "User",
        text,
        reasoning: "",
        tools: [],
        images: images || [],
        error: null,
      });
      return scriptTurn(sessionId, text);
    },
    retry_last: ({ sessionId }) => {
      const msgs = store.cache[sessionId] || [];
      const lastUser = [...msgs].reverse().find((m) => m.role === "User");
      msgs.splice(Math.max(0, msgs.length - 2));
      msgs.push({
        role: "User",
        text: lastUser ? lastUser.text : "again",
        reasoning: "",
        tools: [],
        images: lastUser ? lastUser.images || [] : [],
        error: null,
      });
      return scriptTurn(sessionId, lastUser ? lastUser.text : "again");
    },
    stop_streaming: ({ sessionId }) => {
      store.cancelled[sessionId] = true;
      // A stop denies this chat's pending approvals server-side; the script
      // notices the cancellation at its next beat and emits the finish.
      for (const [id, apr] of Object.entries(store.approvals)) {
        if (apr.session_id === sessionId) {
          delete store.approvals[id];
          apr.resolve({ approved: false, cancelled: true });
        }
      }
      return { stopped: true };
    },
    current_stream: () => Object.values(store.streams).map(copy),
    respond_approval: ({ id, approved }) => {
      const apr = store.approvals[id];
      if (apr) apr.resolve({ approved: !!approved });
      return { id, approved: !!approved };
    },
    allow_tool: ({ tool }) => {
      if (!store.settings.allowed_tools.includes(tool)) store.settings.allowed_tools.push(tool);
      return copy(store.settings.allowed_tools);
    },
    session_overview: ({ sessionId }) => overviewOf(sessionId),
    compacted_archive: ({ sessionId }) =>
      store.compact[sessionId] ? store.compact[sessionId].archived_messages : 0,
    archived_messages: ({ sessionId }) => [],
    // Takes `session_id`, like the command it doubles: the old stub took
    // `sessionId` and returned 0 without touching state, so restoring folded
    // turns could only ever look like it worked.
    restore_compacted: ({ session_id }) => {
      const c = store.compact[session_id];
      if (!c) return 0;
      const restored = c.archived_messages;
      c.live_messages += c.archived_messages;
      c.archived_messages = 0;
      c.can_compact = true;
      return restored;
    },
    get_plan: ({ sessionId }) => copy(store.plan[sessionId] || []),
    restore_plan: ({ id }) => copy(store.plan[id] || []),
    set_plan: ({ sessionId, items }) => {
      store.plan[sessionId] = copy(items || []);
      return copy(store.plan[sessionId]);
    },
    get_goal: ({ sessionId }) => copy(store.goal[sessionId] || null),
    set_goal: ({ sessionId, objective, maxRounds }) => {
      if (!objective) {
        delete store.goal[sessionId];
        return null;
      }
      store.goal[sessionId] = {
        objective,
        state: "active",
        rounds: 0,
        max_rounds: maxRounds === undefined || maxRounds === null ? 12 : maxRounds,
        blocked_reason: null,
      };
      return copy(store.goal[sessionId]);
    },
    goal_action: ({ sessionId, action }) => {
      const g = store.goal[sessionId];
      if (!g) throw new Error("no active goal");
      if (action === "advance") {
        if (g.max_rounds && g.rounds >= g.max_rounds) throw new Error("round cap reached");
        g.rounds += 1;
      } else if (action === "pause") {
        g.state = g.state === "paused" ? "active" : "paused";
      } else if (action === "complete") {
        g.state = "completed";
      } else if (action === "block") {
        g.state = "blocked";
        g.blocked_reason = "blocked by the harness";
      }
      return copy(g);
    },
    get_settings: () => copy(store.settings),
    save_settings: (patch) => {
      const map = {
        model: "model",
        baseUrl: "base_url",
        apiKey: "api_key",
        systemPrompt: "system_prompt",
        projectInstructions: "project_instructions",
        temperature: "temperature",
        reasoningEffort: "reasoning_effort",
        bashEnabled: "bash_enabled",
        fileToolsEnabled: "file_tools_enabled",
        bashWorkspace: "bash_workspace",
        skillsDir: "skills_dir",
        memoryEnabled: "memory_enabled",
        memoryPath: "memory_path",
        requireApproval: "require_approval",
        contextBudget: "context_budget",
        webToolsEnabled: "web_tools_enabled",
        searchApiUrl: "search_api_url",
        searchApiKey: "search_api_key",
        mcpServers: "mcp_servers",
        providers: "providers",
      };
      for (const [from, to] of Object.entries(map)) {
        if (Object.prototype.hasOwnProperty.call(patch, from)) store.settings[to] = patch[from];
      }
      return copy(store.settings);
    },
    list_profiles: () => [
      { name: "glm-4.6", model: "glm-4.6", base_url: "https://api.example.com/v1", has_key: true },
      { name: "claude-sonnet", model: "claude-sonnet-4", base_url: "https://api.anthropic.com", has_key: false },
    ],
    list_capabilities: ({ session } = {}) => {
      const carried = skillScope(session).active;
      return {
        tools: [],
        async_tools: [],
        skills: store.skills
          .filter((s) => carried.includes(s.name))
          .map((s) => ({ name: s.name, description: s.description, category: "skill" })),
        plugins: [],
        hook_events: ["pre_tool_use"],
      };
    },
    memory_status: () => {
      const settings = store.settings;
      const workspaces = settings.workspaces || [];
      const active =
        workspaces.find((w) => w.id === settings.active_workspace) || workspaces[0] || {};
      const file = (w) => (w.memory_path || "").trim() || DEFAULT_MEMORY_FILE;
      const path = file(active);
      return {
        enabled: settings.memory_enabled !== false,
        path,
        shared_workspaces: workspaces.filter((w) => file(w) === path).length,
        cap: 500,
        max_chars: 2000,
      };
    },
    list_memories: () => copy(store.memories),
    remember_memory: ({ text }) => {
      store.memories.unshift({ id: `m-${nextId++}`, content: text, created_at_ms: Date.now() });
      return copy(store.memories);
    },
    forget_memory: ({ id }) => {
      store.memories = store.memories.filter((m) => m.id !== id);
      return copy(store.memories);
    },
    skill_list: ({ nowMs }) => skillReport(nowMs || 0),
    skill_activation: ({ session, nowMs }) => skillActivationReport(session, nowMs || 0),
    skill_set_active: ({ session, name, active, nowMs }) => {
      const available = skillAvailable();
      if (active && !available.includes(name)) {
        throw new Error(
          store.skills.some((skill) => skill.name === name)
            ? `this workspace has switched ${JSON.stringify(name)} off`
            : `this workspace has no skill named ${JSON.stringify(name)}`,
        );
      }
      const key = String(session || "");
      const record = store.skillActivation[key] || available.slice();
      const next = record.filter((held) => held !== name);
      if (active) next.push(name);
      store.skillActivation[key] = next.sort();
      return skillActivationReport(session, nowMs || 0);
    },
    skill_inherit: ({ session, nowMs }) => {
      delete store.skillActivation[String(session || "")];
      return skillActivationReport(session, nowMs || 0);
    },
    skill_author: ({ name, description, body, overwrite, nowMs }) => {
      const errors = skillErrors(name, description, body);
      if (errors.length) throw new Error(errors.join("; "));
      const slug = skillSlug(name);
      const existing = store.skills.find((s) => s.name === slug);
      if (existing && !overwrite) {
        throw new Error(
          `${store.skillsDir}/${slug}/SKILL.md already exists — replace it to overwrite`,
        );
      }
      const row = {
        name: slug,
        description: String(description).trim(),
        body: String(body).trim(),
      };
      if (existing) store.skills[store.skills.indexOf(existing)] = row;
      else store.skills.push(row);
      return skillReport(nowMs || 0);
    },
    knowledge_list: ({ nowMs }) => knowledgeReport("", null, nowMs),
    knowledge_search: ({ query, limit, nowMs }) =>
      knowledgeReport(query, limit == null ? null : Number(limit), nowMs),
    knowledge_add: ({ title, tags, body, nowMs }) => {
      if (store.knowledge.length >= KB_MAX) {
        throw new Error(`at the cap of ${KB_MAX} entries; delete one first`);
      }
      const clean = kbClean(title, body);
      store.kbSeq += 1;
      store.knowledge.push({
        id: `kb-${nowMs}-${store.kbSeq}`,
        title: clean.title,
        tags: kbTags(tags),
        body: clean.body,
        created_ms: nowMs || 0,
        updated_ms: nowMs || 0,
      });
      return knowledgeReport("", null, nowMs);
    },
    knowledge_update: ({ id, title, tags, body, nowMs }) => {
      const entry = store.knowledge.find((e) => e.id === id);
      if (!entry) throw new Error(`no knowledge entry with id ${id}`);
      const clean = kbClean(title, body);
      entry.title = clean.title;
      entry.tags = kbTags(tags);
      entry.body = clean.body;
      entry.updated_ms = nowMs || 0;
      return knowledgeReport("", null, nowMs);
    },
    knowledge_delete: ({ id, nowMs }) => {
      const at = store.knowledge.findIndex((e) => e.id === id);
      if (at === -1) throw new Error(`no knowledge entry with id ${id}`);
      store.knowledge.splice(at, 1);
      return knowledgeReport("", null, nowMs);
    },
    schedule_list: ({ nowMs }) => scheduleReport(nowMs || 0, []),
    schedule_tick: ({ nowMs }) => {
      const now = nowMs || 0;
      return scheduleReport(now, tickSchedules(now));
    },
    schedule_add: ({ text, everyMinutes, atMs, nowMs }) => {
      const trimmed = (text || "").trim();
      if (!trimmed) throw new Error("a reminder needs some text");
      if (trimmed.length > SCHED_MAX_TEXT) {
        throw new Error(`a reminder is at most ${SCHED_MAX_TEXT} characters`);
      }
      if (everyMinutes != null && everyMinutes < SCHED_MIN_EVERY) {
        throw new Error(
          `a repeating reminder needs at least ${SCHED_MIN_EVERY} minute between deliveries`,
        );
      }
      if (store.schedules.length >= SCHED_MAX) {
        throw new Error(`at the cap of ${SCHED_MAX} reminders; cancel one first`);
      }
      store.scheduleSeq += 1;
      store.schedules.push({
        id: `sc-${nowMs}-${store.scheduleSeq}`,
        text: trimmed,
        cadence:
          everyMinutes != null
            ? {
                kind: "every",
                every_minutes: everyMinutes,
                next_ms: (nowMs || 0) + everyMinutes * 60000,
              }
            : { kind: "once", at_ms: atMs || 0 },
        created_ms: nowMs || 0,
        last_fired_ms: null,
        fired: 0,
      });
      return scheduleReport(nowMs || 0, []);
    },
    schedule_cancel: ({ id, nowMs }) => {
      const before = store.schedules.length;
      store.schedules = store.schedules.filter((s) => s.id !== id);
      if (store.schedules.length === before) throw new Error(`no reminder with id ${id}`);
      return scheduleReport(nowMs || 0, []);
    },
    schedule_clear_history: ({ nowMs }) => {
      store.scheduleHistory = [];
      return scheduleReport(nowMs || 0, []);
    },
    search_sessions: ({ query }) => {
      const q = (query || "").toLowerCase();
      const hits = [];
      for (const s of store.sessions) {
        for (const m of store.cache[s.id] || []) {
          const hay = `${s.title}\n${m.text || ""}`.toLowerCase();
          const at = hay.indexOf(q);
          if (at >= 0) {
            hits.push({
              id: s.id,
              title: s.title,
              workspace: s.workspace,
              snippet: `…${hay.slice(Math.max(0, at - 12), at + 40).replace(/\s+/g, " ")}…`,
            });
            break;
          }
        }
      }
      return hits.slice(0, 20);
    },
    mcp_status: () => ({
      connecting: false,
      servers: store.mcpServers.map((server) => ({
        name: server.name,
        transport: server.transport,
        enabled: server.enabled,
        connected: server.connected,
        tools: policyAttached(server).length,
        tool_names: copy(server.tools),
        error: null,
      })),
    }),
    mcp_catalog: ({ nowMs }) => catalogReport(nowMs),
    mcp_read_resource: ({ server, uri, nowMs }) => readReport(server, uri, nowMs),
    mcp_policy_status: ({ nowMs }) => policyReport(nowMs),
    mcp_policy_set: ({ server, tool, effect }) => policySet(server, tool, effect),
    mcp_policy_clear: ({ server, tool }) => {
      store.mcpPolicy.rules = store.mcpPolicy.rules.filter(
        (r) => !(r.server === server && r.tool === tool),
      );
      return policyReport(Date.now());
    },
    mcp_policy_default: ({ effect }) => {
      store.mcpPolicy.default = effect;
      return policyReport(Date.now());
    },
    reconnect_mcp: () => ({ connecting: false, servers: [] }),
    export_session: () => "/tmp/live-export.md",
    compact_session: () => ({ ok: true }),
    init_agents: () => ({ ok: true }),
    open_url: () => true,
    truncate_session: ({ sessionId, index }) => {
      const msgs = store.cache[sessionId] || [];
      msgs.splice(index);
      return copy(msgs);
    },
    // The real command keeps the first `index` messages and names the branch
    // after its parent: the page reads `fork.title` and `fork.messages` back.
    fork_session: ({ sessionId, index }) => {
      const owner = store.sessions.find((x) => x.id === sessionId);
      if (!owner) throw new Error("no such chat");
      const src = copy(store.cache[sessionId] || []);
      if (!Number.isInteger(index) || index < 1 || index > src.length) {
        throw new Error(`cannot branch at ${index} of ${src.length}`);
      }
      const prefix = src.slice(0, index);
      const fork = mkSession(`${owner.title} (branch)`, prefix);
      // The command answers with the whole record, messages included: the page
      // primes its cache from `fork.messages` rather than re-reading it.
      return { ...copy(fork), messages: copy(prefix) };
    },
    move_session: ({ id, workspaceId }) => {
      const s = store.sessions.find((x) => x.id === id);
      if (s) s.workspace = workspaceId;
      return s ? copy(s) : null;
    },
    // The real command answers with the whole settings, and the page takes it as
    // a snapshot: a partial one would blank the workspace list the sidebar is
    // grouped by.
    set_active_workspace: ({ id }) => {
      if (!store.settings.workspaces.some((w) => w.id === id)) {
        throw new Error(`unknown workspace '${id}'`);
      }
      store.settings.active_workspace = id;
      return copy(store.settings);
    },
    search_files: () => [],
    list_files: ({ path }) => copy(workspaceFiles[path] || { entries: [], truncated: false }),
    read_file: ({ path }) => ({
      path,
      text: workspaceText[path] || "",
      bytes: (workspaceText[path] || "").length,
      truncated: false,
      lines: [],
    }),
    // M4/M6/M7/M10/M11/M14 panels: not driven here (page_selftest owns them
    // with settled fixtures), but answer honestly if a refresh fires.
    jobs_list: () => store.jobs.map(jobRecord),
    job_output: ({ id, max_bytes }) => {
      const job = store.jobs.find((j) => j.id === id);
      if (!job) throw new Error(`no such job: ${id}`);
      const cap = max_bytes || 16384;
      // The backend keeps the *tail*: the newest output is what a user wants.
      const text = job.body.length > cap ? job.body.slice(job.body.length - cap) : job.body;
      return {
        id,
        state: job.state,
        text,
        bytes: text.length,
        total_bytes: job.body.length,
        truncated: job.body.length > cap,
        dropped_bytes: Math.max(0, job.body.length - cap),
      };
    },
    job_interrupt: ({ id }) => {
      const job = store.jobs.find((j) => j.id === id);
      if (!job) throw new Error(`no such job: ${id}`);
      job.state = "interrupted";
      job.finished_at = job.started_at + 7;
      return jobRecord(job);
    },
    token_meter: ({ session_id, provider_tokens }) => {
      const c = compactFor(session_id);
      const estimated = c.live_messages * 30;
      const provider =
        provider_tokens === null || provider_tokens === undefined ? null : provider_tokens;
      const delta = provider === null ? null : provider - estimated;
      return {
        session_id,
        messages: c.live_messages + c.archived_messages,
        estimated,
        archived_estimated: c.archived_messages * 30,
        provider,
        delta,
        within_tolerance: delta === null ? null : Math.abs(delta) <= 64,
        per_message: [],
      };
    },
    compaction_status: ({ session_id }) => compactStatus(session_id),
    compact_now: ({ session_id }) => {
      const c = compactFor(session_id);
      if (!c.can_compact) {
        return { ...compactStatus(session_id), refusal: "not enough live history to fold" };
      }
      c.archived_messages += c.live_messages;
      c.live_messages = 0;
      c.can_compact = false;
      return compactStatus(session_id);
    },
    spill_list: () => store.spills.map((sp) => ({ id: sp.id, file: sp.file, bytes: sp.bytes, preview: sp.preview })),
    spill_read: ({ id, max_bytes }) => {
      const sp = store.spills.find((x) => x.id === id);
      if (!sp) throw new Error(`no such spill: ${id}`);
      const cap = max_bytes || 8192;
      return {
        id,
        file: sp.file,
        bytes: sp.bytes,
        truncated: sp.body.length > cap,
        text: sp.body.slice(0, cap),
      };
    },
    approval_rules_list: () => rulesReport(),
    approval_rule_add: ({ tool, scope, effect, arg_key, arg_match, arg_value, workspace, session_id }) => {
      if ((arg_key && (!arg_match || !arg_value)) || (!arg_key && (arg_match || arg_value))) {
        throw new Error("an argument predicate needs arg_key, arg_match, and arg_value together");
      }
      if (scope === "session" && !session_id) {
        throw new Error("a session-scoped rule needs a session_id");
      }
      const rule = {
        id: `rule-${rulesState.rules.length + 1}`,
        tool,
        scope,
        effect,
        created_at: Math.floor(Date.now() / 1000),
      };
      if (arg_key) rule.arg = { key: arg_key, match_kind: arg_match, value: arg_value };
      if (scope === "workspace") rule.workspace = workspace || "/tmp/live";
      if (scope === "session") rule.session = session_id;
      rulesState.rules.push(rule);
      return rulesReport();
    },
    approval_rule_revoke: ({ id }) => {
      const index = rulesState.rules.findIndex((r) => r.id === id);
      if (index < 0) throw new Error(`no rule with id '${id}'`);
      rulesState.rules.splice(index, 1);
      return rulesReport();
    },
    approval_audit: ({ limit }) =>
      copy(rulesState.audit).slice(0, Math.min(limit || 50, rulesState.audit_cap)),

    // The agent reading its own composition: counts derived from the harness's
    // real store (memories, the active goal), so the pane cannot agree with the
    // Goal card by accident.
    self_inspect: ({ session_id }) => {
      const goal = store.goal[session_id] || null;
      return {
        tools: 11,
        async_tools: 4,
        skills: 2,
        plugins: 1,
        hook_events: 5,
        mcp_servers: 1,
        mcp_tools: store.mcpServers.reduce((n, s) => n + policyAttached(s).length, 0),
        memories: store.memories.length,
        approval_rules: rulesState.rules.length,
        active_goal: goal ? goal.objective : null,
        goal_state: goal ? goal.state : null,
        goal_rounds: goal ? goal.rounds : null,
        goal_remaining:
          goal && goal.max_rounds != null ? Math.max(0, goal.max_rounds - goal.rounds) : null,
        model: "harness-model",
        workspace: "/tmp/live",
        bash_enabled: true,
        file_tools_enabled: true,
        skills_dir: "/tmp/live/skills",
      };
    },
    evaluate_transcript: ({ session_id }) => {
      const msgs = store.cache[session_id] || [];
      const blank = msgs.filter(
        (m) => m.role === "Assistant" && !String(m.text || "").trim(),
      ).length;
      const failures = msgs.reduce(
        (n, m) => n + (m.tools || []).filter((t) => t.failed).length,
        0,
      );
      const score = Math.max(0, 100 - blank * 20 - failures * 10);
      const reasons = [];
      if (blank) reasons.push(`${blank} blank assistant turn(s) (-${blank * 20})`);
      if (failures) reasons.push(`${failures} failed tool call(s) (-${failures * 10})`);
      if (!reasons.length) reasons.push("No problems were found in this transcript.");
      return {
        session_id,
        score,
        grade: score >= 90 ? "excellent" : score >= 75 ? "good" : score >= 60 ? "fair" : "poor",
        reasons,
        signals: { turns: msgs.length, blank_turns: blank, tool_failures: failures },
      };
    },
    replay_session: ({ session_id }) => {
      const msgs = store.cache[session_id] || [];
      const entries = msgs.map((m, i) => ({
        order: i,
        index: i,
        role: String(m.role || "").toLowerCase(),
        text: m.text || "",
        reasoning: m.reasoning || "",
        tools: (m.tools || []).map((t) => ({ name: t.name, args: t.args, failed: !!t.failed, ms: t.ms ?? 0 })),
        error: m.error || null,
        archived: false,
      }));
      const tools = entries.flatMap((e) => e.tools);
      return {
        session_id,
        title: (store.sessions.find((x) => x.id === session_id) || {}).title || "",
        workspace: "/tmp/live",
        entries,
        metrics: {
          turns: entries.length,
          user_turns: entries.filter((e) => e.role === "user").length,
          assistant_turns: entries.filter((e) => e.role === "assistant").length,
          archived_turns: 0,
          tool_calls: tools.length,
          tool_failures: tools.filter((t) => t.failed).length,
          tool_ms_total: tools.reduce((n, t) => n + (t.ms || 0), 0),
          tool_ms_missing: tools.filter((t) => !t.ms).length,
          errors: entries.filter((e) => e.error).length,
          blank_turns: entries.filter((e) => e.role === "assistant" && !e.text.trim()).length,
          estimated_tokens: entries.reduce((n, e) => n + Math.ceil((e.text.length + e.reasoning.length) / 4), 0),
        },
      };
    },
    export_bundle: ({ session_id }) => {
      const msgs = store.cache[session_id] || [];
      const bytes = 2048 + msgs.length * 512;
      return {
        path: `/tmp/live/bundles/${session_id}.zip`,
        bytes,
        schema_version: 1,
        entries: msgs.length,
        files: [
          { name: "transcript.json", size: bytes - 900, crc32: "0f1e2d3c" },
          { name: "metrics.json", size: 400, crc32: "a1b2c3d4" },
          { name: "manifest.json", size: 500, crc32: "9f8e7d6c" },
        ],
      };
    },
    // A workspace is created from what the dialog collected, and the store keeps
    // it: the checks read the workspace back out of here rather than out of the
    // dialog that sent it.
    create_workspace: ({ name, root, config }) => {
      const path = String(root == null ? "" : root).trim();
      if (!path) throw new Error("choose a folder for this workspace");
      if (!path.startsWith("/")) throw new Error(`${path}: no such file or directory`);
      const seed = store.settings.workspaces[0].mcp_servers || [];
      for (const wanted of (config && config.mcp_enabled) || []) {
        if (!seed.some((m) => m.name === wanted)) {
          throw new Error(`no MCP server named "${wanted}"`);
        }
      }
      const id = `w-new-${store.settings.workspaces.length}`;
      const workspace = {
        id,
        name: String(name || "").trim() || path.split("/").filter(Boolean).pop() || "Default",
        root: path,
        model: (config && config.model) || "",
        mcp_servers: seed.map((m) => ({
          ...m,
          enabled: (config && config.mcp_enabled ? config.mcp_enabled : seed.filter((s) => s.enabled).map((s) => s.name)).includes(m.name),
        })),
        disabled_tools: (config && config.disabled_tools) || [],
        disabled_skills: (config && config.disabled_skills) || [],
        disabled_plugins: (config && config.disabled_plugins) || [],
      };
      store.settings.workspaces = [...store.settings.workspaces, workspace];
      store.settings.active_workspace = id;
      return copy({ ...store.settings, active_workspace: id });
    },
    // The folder chooser's own tree, one level at a time.
    choose_folder: ({ start }) => {
      const next = LIVE.answers.choose_folder;
      if (next !== undefined) {
        LIVE.answers.choose_folder = undefined;
        if (next instanceof Error) throw next;
        return next;
      }
      return start ? `${start}/bos` : "/home/e2e";
    },
    list_folder: ({ path }) => {
      const tree = {
        "/home/e2e": { parent: "/home", dirs: ["bos", "Documents"] },
        "/home/e2e/bos": { parent: "/home/e2e", dirs: ["crates", "docs"] },
        "/home/e2e/Documents": { parent: "/home/e2e", dirs: [] },
      };
      const at = path || "/home/e2e";
      const level = tree[at];
      if (!level) throw new Error(`${at}: no such file or directory`);
      return {
        path: at,
        parent: level.parent,
        entries: level.dirs.map((d) => ({ name: d, path: `${at}/${d}` })),
        truncated: 0,
        unreadable: 0,
      };
    },
    update_workspace: () => null,
    remove_workspace: () => null,
    save_preset: () => null,
    apply_preset: () => null,
    // A shell that outlives the panel, like the real one: the tail is replayed
    // on re-attach, writes echo through an event rather than synchronously, and
    // a kill ends it. The old `() => null` stub made the shipped path throw on
    // `status.tail`, so nothing had ever exercised the terminal for real.
    pty_start: ({ sessionId, shell }) => {
      const live = store.pty[sessionId];
      if (live) return { shell: live.shell, fresh: false, tail: copy(live.tail) };
      store.pty[sessionId] = {
        shell: shell || "/bin/zsh",
        tail: ["bos harness shell\r\n$ "],
      };
      return { shell: store.pty[sessionId].shell, fresh: true, tail: copy(store.pty[sessionId].tail) };
    },
    pty_write: ({ sessionId, data }) => {
      const live = store.pty[sessionId];
      if (!live) throw new Error("no such shell");
      const echoed = `${data}$ ${data.trim() || "echo"}: live harness\r\n$ `;
      live.tail.push(echoed);
      emit("pty-output", { session_id: sessionId, text: echoed }, 5);
      return true;
    },
    pty_kill: ({ sessionId }) => {
      const live = store.pty[sessionId];
      if (!live) throw new Error("no such shell");
      delete store.pty[sessionId];
      emit("pty-exit", { session_id: sessionId, exit_code: null }, 5);
      return true;
    },
    pty_resize: () => true,
    run_command: () => true,
  };

  window.__TAURI__ = {
    core: {
      invoke: (cmd, args) => {
        LIVE.calls.push({ cmd, args: args || {} });
        const f = LIVE.failures.find((x) => x.cmd === cmd && x.left > 0);
        if (f) {
          f.left -= 1;
          return Promise.reject(new Error(f.message));
        }
        const h = handlers[cmd];
        if (!h) {
          LIVE.errors.push(`no backend handler for ${cmd}`);
          return Promise.reject(new Error(`live backend: no handler for ${cmd}`));
        }
        try {
          return Promise.resolve(h(args || {}));
        } catch (err) {
          return Promise.reject(err);
        }
      },
      transformCallback: () => 0,
    },
    event: { listen, emit: (name, payload) => emit(name, payload, 0) },
  };
})();
