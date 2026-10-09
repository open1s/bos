/* BOS chat frontend: bridges Tauri IPC (invoke + events) to the DOM. */
"use strict";

// Late-bound on purpose: the runtime injects `__TAURI__`, and reading it per
// call means a harness (or a test double) can observe what a form sends.
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const { listen } = window.__TAURI__.event;

/* ---------- state ---------- */

/* Only the tail of a long transcript is drawn: a 10 000-message chat should cost
   what a 200-message one does. "Load earlier" grows the window explicitly, so the
   transcript never grows sideways under a scroll position the user did not pick. */
const WINDOW = 200;
let lastWindowLen = -1;
let lastWindowStart = -1;
let loadEarlierEl = null;
let topSpacerEl = null;    // stands in for the un-rendered rows above the window
let spacerCredit = 0;      // px the spacer gave up, so a grown window can stay put
let lastSpacerPx = 0;      // what the spacer was last rendered at
let avgRowPx = 64;         // measured from rendered rows, off the streaming path
let scrollGrowBound = false;
let lastChildCount = -1;   // messagesEl.childElementCount after the last pass

const state = {
  sessions: [],          // [{id, title, updated_at}] newest first
  activeId: null,
  cache: {},             // sessionId -> [ChatMessage]
  streams: {},           // sessionId -> {session_id, gen, started_at} (many may stream)
  usage: null,           // {prompt, completion} | null
  toolCount: 0,
  error: null,
  model: "",
  workspaces: [],       // [{id,name,root,model}] from the server (§24)
  presets: [],          // [{id,name}] reusable workspace configurations (§24.9)
  activeWorkspace: "",
  wsEdit: null,         // {mode:'create'|'edit'|'remove', id} | null — inline form
  wsBusy: false,        // a workspace command is in flight
  windowExtra: 0,       // extra rows kept above the tail window
  collapsed: {},        // workspace groups whose chats are hidden
  hits: [],             // content matches for the current search
  usage: { prompt: 0, completion: 0 }, // what the provider said the last turn cost
  memories: [],         // what the agent remembers across chats
  budget: 32768,        // context-budget meter ceiling (tokens; 0 = off)
  approvals: [],         // queued approval-request payloads
  allowedTools: [],      // tools allowed permanently ("always allow")
  profiles: [],          // discovered [llm.<name>] profiles, key-free
  queue: {},             // sessionId -> [text] follow-ups waiting for the stream
  compacting: {},
  archived: 0,          // turns the last compaction folded, restorable
  trashed: [],          // chats in the trash, newest first
  overview: null,       // counts and outline for the active chat
  overviewId: null,     // which chat that overview describes        // sessionId -> true while /compact summarizes in the background
  initializing: false,   // true while /init drafts AGENTS.md in the background
  editing: null,         // {sessionId, index} | null — inline message editor open
  plan: [],              // last server-side plan of the active session (draft source)
  planEditing: false,    // plan panel in edit mode (human-in-the-loop steering)
  planDraft: null,       // [{text, status}] | null — unsaved plan edits
  mcpServers: [],        // edit buffer from the settings dialog
  providers: [],         // fallback-provider edit buffer from the dialog
  mcpStatus: null,       // {connecting, servers} from the mcp_status command
  mcpPolling: false,     // true while a status follow-up loop is running
  search: "",            // sidebar filter (session-title substring)
  palette: null,         // {query, entries, index} | null — slash palette
  skills: [],            // cached skills for the palette [{name, description}]
  mention: null,         // {query, atStart, entries, index, fresh} | null — @file picker
  mentionDismissed: "",  // token query dismissed with Esc (reopens on change)
  promptNav: null,       // {pos, draft} | null — ↑/↓ prompt-history walk
  runPanel: { open: false, activeId: null, lines: [] }, // run panel: streamed command output
};

/* ---------- multi-session streaming helpers ---------- */

/** The stream record for `id`, or null when that chat is idle. */
const streamOf = (id) => (id ? state.streams[id] || null : null);

/** Streams belonging to chats other than the active one. */
const otherStreams = () =>
  Object.values(state.streams).filter((s) => s.session_id !== state.activeId);

/* ---------- DOM ---------- */

/* Cap on follow-ups one chat may hold while streaming: past this the
   composer gives the text back instead of silently swallowing input. */
const MAX_QUEUE = 8;

const $ = (id) => document.getElementById(id);
const sessionList = $("session-list");
const messagesEl = $("messages");
const chatTitle = $("chat-title");
const chip = $("stream-chip");
const input = $("input");
const queueList = $("queue-list");
const sendBtn = $("send");
const statusLeft = $("status-left");
const statusRight = $("status-right");
const modal = $("settings-modal");
const settingsForm = $("settings-form");
const setModel = $("set-model");
const profilesHost = $("profiles-list");
const setBaseUrl = $("set-base-url");
const setApiKey = $("set-api-key");
const setSystem = $("set-system");
const setInstructions = $("set-instructions");
const setTemp = $("set-temperature");
const setEffort = $("set-effort");
const setBash = $("set-bash");
const setFiles = $("set-files");
const setApproval = $("set-approval");
const setWorkspace = $("set-workspace");
const setSkillsDir = $("set-skills-dir");
const setMemory = $("set-memory");
const setMemoryPath = $("set-memory-path");
const setContextBudget = $("set-context-budget");
const capList = $("cap-list");
const mcpList = $("mcp-list");
const mcpName = $("mcp-name");
const mcpTransport = $("mcp-transport");
const mcpCmd = $("mcp-cmd");
const mcpUrl = $("mcp-url");
const mcpAddBtn = $("mcp-add-btn");
const mcpMsg = $("mcp-msg");
const mcpConn = $("mcp-conn");
const provList = $("prov-list");
const provName = $("prov-name");
const provModel = $("prov-model");
const provUrl = $("prov-url");
const provKey = $("prov-key");
const provAddBtn = $("prov-add-btn");
const provMsg = $("prov-msg");
const approvalModal = $("approval-modal");
const approvalTool = $("approval-tool");
const approvalArgs = $("approval-args");
const approvalDiff = $("approval-diff");
const allowedList = $("allowed-list");
const trashList = $("trash-list");
const trashPurge = $("trash-purge");
trashPurge.addEventListener("click", () => void purgeTrash());
const sessionSearch = $("session-search");
const paletteEl = $("cmd-palette");
const paletteList = $("cmd-list");
const mentionEl = $("mention-pop");
const mentionList = $("mention-list");

function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

const clip = (s, n) => (s.length > n ? s.slice(0, n) + "…" : s);

function fmtTime(unix) {
  const d = new Date(unix * 1000);
  const p = (v) => String(v).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/* Session ages read better as relative labels; the exact stamp stays available on
   hover. `now` is injectable so the shape is testable without a clock. */
function fmtAge(unix, now = Date.now() / 1000) {
  const secs = Math.max(0, Math.round(now - unix));
  if (secs < 45) return "now";
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  if (secs < 86400) return `${Math.round(secs / 3600)}h`;
  if (secs < 7 * 86400) return `${Math.round(secs / 86400)}d`;
  return fmtTime(unix).slice(0, 5);
}

/* ---------- rendering ---------- */

function activeMsgs() {
  return state.cache[state.activeId] || [];
}

/* Mirror of the Rust-side token estimate behind send-side compaction
   (ascii ÷ 4 rounded up + one token per non-ascii code point), restricted to
   the messages seed_session actually forwards: user + assistant text, plus
   four tokens of per-message overhead. Keeping both sides identical means the
   meter and the compactor always agree. */
function estimateTokens(text) {
  if (!text) return 0;
  let ascii = 0;
  let other = 0;
  for (const ch of text) {
    if (ch.codePointAt(0) <= 0x7f) ascii += 1;
    else other += 1;
  }
  return Math.ceil(ascii / 4) + other;
}

/* Estimated outgoing context of the active transcript, in tokens. */
function estimateContext() {
  let tokens = 0;
  for (const m of activeMsgs()) {
    if ((m.role === "user" || m.role === "assistant") && m.text) {
      tokens += estimateTokens(m.text) + 4;
    }
  }
  return tokens;
}

/* Compact token counts for the statusbar: 950 → "950", 32768 → "32.8k". */
function fmtTokens(v) {
  return v >= 1000 ? `${(v / 1000).toFixed(1)}k` : String(v);
}

/* Sessions belong to a workspace, so the panel shows them that way: one header per
   workspace (name, root, the model bound to it) with its chats underneath. Pure, so
   the grouping can be tested without a DOM. */
function groupSessions(sessions, workspaces, activeId) {
  const byId = new Map((workspaces || []).map((w) => [w.id, w]));
  const groups = new Map();
  for (const s of sessions) {
    const id = s.workspace || activeId || "";
    if (!groups.has(id)) groups.set(id, []);
    groups.get(id).push(s);
  }
  return [...groups.entries()].map(([id, items]) => {
    const w = byId.get(id);
    return {
      id,
      name: (w && (w.name || w.root)) || id || "Default",
      root: (w && w.root) || "",
      model: (w && w.model) || "",
      sessions: items,
    };
  });
}

/* Every workspace command returns the newly effective settings, because the
   server bumps its settings generation when the choice changes (each cached agent
   is then rebuilt from the new model and tool set). One applier keeps that
   invariant in a single place. */
async function applyWorkspaceSnapshot(s) {
  state.workspaces = s.workspaces || [];
  state.presets = s.presets || [];
  state.activeWorkspace = s.active_workspace || "";
  state.model = s.model || state.model;
  state.budget = s.context_budget ?? state.budget;
  const list = await invoke("list_sessions");
  state.sessions = Array.isArray(list) ? list : [];
  renderSidebar();
  renderTitle();
}

/* A workspace command, with the failure reported the same way the transcript
   reports one: in the status line, not silently swallowed. */
async function workspaceCommand(promise) {
  try {
    state.wsBusy = true;
    await applyWorkspaceSnapshot(await promise);
    state.wsEdit = null;
  } catch (e) {
    state.error = String(e);
  } finally {
    state.wsBusy = false;
    renderSidebar();
    scheduleRepaint();
  }
}

async function switchWorkspace(id) {
  if (!id || id === state.activeWorkspace) return;
  await workspaceCommand(invoke("set_active_workspace", { id }));
}

/* Skills the panel can offer: the ones the capability listing knows about, plus any
   the workspace already denies — so a denied skill stays visible (and can be turned
   back on) even before that listing has been loaded. */
function skillNamesFor(workspace) {
  const names = (state.skills || []).map((s) => s && s.name).filter(Boolean);
  for (const denied of (workspace && workspace.disabled_skills) || []) {
    if (!names.includes(denied)) names.push(denied);
  }
  return names;
}

/* Which skills the form has switched off, same deny-list shape as the tools. */
function disabledSkillsFromForm(form) {
  return [...form.querySelectorAll(".ws-skill")]
    .filter((box) => !box.checked)
    .map((box) => box.value);
}

/* The MCP servers a workspace allows. Servers are opt-in: an entry is always
   configured by hand with its own transport, so the panel sends the entries back
   untouched apart from `enabled`, and a server nobody ticked simply does not
   attach. */
function mcpSelectionFromForm(form, known) {
  const boxes = [...form.querySelectorAll(".ws-mcp")];
  const ticked = new Map(boxes.map((box) => [box.value, box.checked]));
  return (known.mcp_servers || []).map((server) => ({
    ...server,
    enabled: ticked.has(server.name) ? ticked.get(server.name) : server.enabled,
  }));
}

/* The built-in tools a workspace may switch off. These are the names the model
   sees, which is what `caps::build_agent` gates on, so the panel and the server
   agree on the vocabulary by construction. */
const BUILTIN_TOOLS = ["bash", "read_file", "write_file", "list_dir", "plan"];

/* Which tools the form has switched off: the ones the workspace denies, so the
   caller sends a deny-list rather than a snapshot of every tool that exists. A
   tool added to the build later therefore appears for people with no opinion. */
function disabledToolsFromForm(form) {
  return [...form.querySelectorAll(".ws-tool")]
    .filter((box) => !box.checked)
    .map((box) => box.value);
}

/* Workspace editing is an inline form rather than a native dialog: a webview may
   not implement prompt(), and an inline form is also the version the harness can
   exercise. */
function openWorkspaceForm(mode, id) {
  state.wsEdit = { mode, id };
  renderSidebar();
  const first = sessionList.querySelector(".ws-form input");
  if (first) first.focus();
}

function closeWorkspaceForm() {
  state.wsEdit = null;
  renderSidebar();
}

/* Save the workspace being edited as a reusable preset: a copy of its
   configuration, offered next to "New workspace" from then on (§24.9). */
function submitWorkspaceAsPreset(form) {
  const id = state.wsEdit ? state.wsEdit.id : "";
  const name = (form.querySelector(".ws-name") || {}).value || "";
  return workspaceCommand(invoke("save_preset", { workspaceId: id, name }));
}

function submitWorkspaceForm(form) {
  const mode = state.wsEdit ? state.wsEdit.mode : "create";
  const id = state.wsEdit ? state.wsEdit.id : "";
  const name = (form.querySelector(".ws-name") || {}).value || "";
  const root = (form.querySelector(".ws-root") || {}).value || "";
  if (mode === "remove") {
    const del = form.querySelector(".ws-del-chats");
    return workspaceCommand(invoke("remove_workspace", { id, deleteSessions: !!(del && del.checked) }));
  }
  if (mode === "create") {
    return workspaceCommand(invoke("create_workspace", { name, root }));
  }
  const known = state.workspaces.find((w) => w.id === id) || {};
  // A field the open form does not show is carried through rather than cleared,
  // the same rule the skills follow: a rename must not repoint the model.
  const shown = (cls, fallback) => {
    const field = form.querySelector(cls);
    return field ? field.value : fallback;
  };
  const tempField = form.querySelector(".ws-temperature");
  const tempValue = tempField ? Number(tempField.value) : NaN;
  const temperature = tempField
    ? tempField.value.trim() !== "" && Number.isFinite(tempValue)
      ? tempValue
      : null
    : known.temperature ?? null;
  return workspaceCommand(
    invoke("update_workspace", {
      id,
      name,
      root,
      model: shown(".ws-model", known.model || ""),
      baseUrl: shown(".ws-base-url", known.base_url || ""),
      // Never rendered, so always carried: the server keeps the key it has.
      apiKey: known.api_key || "",
      systemPrompt: shown(".ws-system-prompt", known.system_prompt || ""),
      memoryPath: shown(".ws-memory", known.memory_path || ""),
      skillsDir: shown(".ws-skills", known.skills_dir || ""),
      // A form without the box leaves the switch as it was, rather than reading
      // an absent checkbox as "off" — the one field here that can be unstated.
      memoryEnabled: (() => {
        const box = form.querySelector(".ws-memory-on");
        return box ? box.checked : known.memory_enabled !== false;
      })(),
      bashEnabled: workspaceFlag(form, known, "ws-bash", "bash_enabled"),
      fileToolsEnabled: workspaceFlag(form, known, "ws-files", "file_tools_enabled"),
      requireApproval: workspaceFlag(form, known, "ws-approval", "require_approval"),
      projectInstructions: workspaceFlag(form, known, "ws-instructions", "project_instructions"),
      contextBudget: workspaceBudget(form, known),
      temperature,
      reasoningEffort: shown(".ws-reasoning", known.reasoning_effort || ""),
      disabledTools: disabledToolsFromForm(form),
      // Not exposed in the panel yet, so it is carried through rather than
      // cleared: saving a rename must not silently re-enable a skill.
      // Only a form that actually showed the skills may rewrite them; otherwise
      // the stored deny-list is carried through rather than cleared.
      disabledSkills: form.querySelectorAll(".ws-skill").length
        ? disabledSkillsFromForm(form)
        : known.disabled_skills || [],
      mcpServers: mcpSelectionFromForm(form, known),
    }),
  );
}

/* Re-home a chat into another workspace. The command returns the session list,
   so the sidebar re-groups from the server's answer rather than guessing. */
async function moveSession(id, workspaceId) {
  const list = await invoke("move_session", { id, workspaceId });
  state.sessions = list;
  renderSidebar();
}

/* The form is rebuilt from `state.wsEdit`, so it survives a re-render (a stream
   tick, a search keystroke) without losing what the user is looking at. */
function workspaceFormEl(workspace) {
  const mode = state.wsEdit.mode;
  const form = el("div", "ws-form");
  if (mode === "remove") {
    form.appendChild(el("div", "ws-form-title", `Remove “${workspace.name}”?`));
    const label = el("label", "ws-form-row");
    const del = el("input", "ws-del-chats");
    del.type = "checkbox";
    label.appendChild(del);
    label.appendChild(el("span", "", "delete its chats too"));
    form.appendChild(label);
    const remove = el("button", "ws-form-go danger", state.wsBusy ? "…" : "Remove");
    remove.addEventListener("click", () => submitWorkspaceForm(form));
    form.appendChild(remove);
  } else {
    if (mode === "edit" && workspace) {
      form.appendChild(el("div", "ws-form-title", "Edit workspace"));
    }
    const name = el("input", "ws-name");
    name.placeholder = "Name";
    name.value = (mode === "edit" && workspace && workspace.name) || "";
    form.appendChild(name);
    const root = el("input", "ws-root");
    root.placeholder = "Root folder (empty = current folder)";
    root.value = (mode === "edit" && workspace && workspace.root) || "";
    form.appendChild(root);
    if (mode === "edit" || mode === "create") {
      const tools = el("div", "ws-tools");
      tools.appendChild(el("div", "ws-form-hint", "Tools this workspace may use"));
      const off = new Set((workspace && workspace.disabled_tools) || []);
      for (const tool of BUILTIN_TOOLS) {
        const row = el("label", "ws-tool-row");
        const box = el("input", "ws-tool");
        box.type = "checkbox";
        box.value = tool;
        box.checked = !off.has(tool);
        row.appendChild(box);
        row.appendChild(el("span", "", tool));
        tools.appendChild(row);
      }
      form.appendChild(tools);
      const skillNames = mode === "edit" ? skillNamesFor(workspace) : [];
      if (skillNames.length) {
        const skills = el("div", "ws-tools");
        skills.appendChild(el("div", "ws-form-hint", "Skills this workspace may use"));
        const denied = new Set((workspace && workspace.disabled_skills) || []);
        for (const name of skillNames) {
          const row = el("label", "ws-tool-row");
          const box = el("input", "ws-skill");
          box.type = "checkbox";
          box.value = name;
          box.checked = !denied.has(name);
          row.appendChild(box);
          row.appendChild(el("span", "", name));
          skills.appendChild(row);
        }
        form.appendChild(skills);
      }
      // Servers are listed on an existing workspace: a new one is seeded from the
      // defaults and can switch them off right after.
      const servers = (workspace && workspace.mcp_servers) || [];
      if (mode === "edit" && servers.length) {
        const mcp = el("div", "ws-tools");
        mcp.appendChild(el("div", "ws-form-hint", "MCP servers this workspace may use"));
        for (const server of servers) {
          const row = el("label", "ws-tool-row");
          const box = el("input", "ws-mcp");
          box.type = "checkbox";
          box.value = server.name;
          box.checked = !!server.enabled;
          row.appendChild(box);
          row.appendChild(el("span", "", server.name));
          mcp.appendChild(row);
        }
        form.appendChild(mcp);
      }
    }
    if (mode === "edit" && workspace) {
      const runtime = el("div", "ws-tools");
      runtime.appendChild(
        el("div", "ws-form-hint", "Model for this workspace (empty = inherit the defaults)"),
      );
      const runtimeInput = (cls, placeholder, value) => {
        // Styled like the name field and read by its own class: the key is
        // deliberately absent, because a secret re-rendered into the DOM is a
        // secret in a screenshot, and the server already has it.
        const input = el("input", `ws-name ws-runtime ${cls}`);
        input.placeholder = placeholder;
        // `value || ""` would turn the legitimate temperature 0 into blank.
        input.value = value === null || value === undefined ? "" : String(value);
        return input;
      };
      runtime.appendChild(runtimeInput("ws-model", "model", workspace.model));
      runtime.appendChild(runtimeInput("ws-base-url", "base url (empty = inherit)", workspace.base_url));
      runtime.appendChild(runtimeInput("ws-reasoning", "reasoning effort: low | medium | high", workspace.reasoning_effort));
      runtime.appendChild(runtimeInput("ws-temperature", "temperature (empty = inherit)", workspace.temperature === null || workspace.temperature === undefined ? "" : workspace.temperature));
      runtime.appendChild(runtimeInput("ws-system-prompt", "system prompt", workspace.system_prompt));
      runtime.appendChild(runtimeInput("ws-memory", "memory file (empty = inherit)", workspace.memory_path));
      runtime.appendChild(runtimeInput("ws-skills", "skills dir (empty = inherit)", workspace.skills_dir));
      const memoryBox = document.createElement("label");
      memoryBox.className = "row-check";
      const memoryInput = document.createElement("input");
      memoryInput.type = "checkbox";
      memoryInput.className = "ws-memory-on";
      memoryInput.checked = workspace.memory_enabled !== false;
      memoryBox.appendChild(memoryInput);
      memoryBox.appendChild(document.createTextNode(" memory"));
      runtime.appendChild(memoryBox);
      // The workspace owns its policy too, so the form covers every field it
      // owns rather than only the model half of them.
      runtime.appendChild(
        workspaceCheck(form, "ws-bash", "bash", workspace.bash_enabled !== false),
      );
      runtime.appendChild(
        workspaceCheck(form, "ws-files", "file tools", workspace.file_tools_enabled !== false),
      );
      runtime.appendChild(
        workspaceCheck(form, "ws-approval", "approval gate", workspace.require_approval !== false),
      );
      runtime.appendChild(
        workspaceCheck(
          form,
          "ws-instructions",
          "project instructions",
          workspace.project_instructions !== false,
        ),
      );
      runtime.appendChild(
        runtimeInput("ws-budget", "context budget (tokens)", workspace.context_budget),
      );
      form.appendChild(runtime);
    }
    if (mode === "edit") {
      const asPreset = el("button", "ws-form-go ws-preset", "Save as preset");
      asPreset.title = "Reuse this configuration when creating a workspace";
      asPreset.addEventListener("click", () => submitWorkspaceAsPreset(form));
      form.appendChild(asPreset);
    }
    const go = el("button", "ws-form-go", state.wsBusy ? "…" : mode === "edit" ? "Save" : "Create");
    go.addEventListener("click", () => submitWorkspaceForm(form));
    form.appendChild(go);
  }
  const cancel = el("button", "ws-form-cancel", "Cancel");
  cancel.addEventListener("click", closeWorkspaceForm);
  form.appendChild(cancel);
  return form;
}

function renderSidebar() {
  sessionList.textContent = "";
  // renderSidebar is the sidebar's single re-render point and every session or
  // workspace change already comes through it, so the Settings list is refreshed
  // here rather than in a second place that could drift out of step.
  renderWorkspaceManager();
  renderSearchHits();
  const q = state.search.trim().toLowerCase();
  const visible = state.sessions.filter(
    (s) => !q || (s.title || "New chat").toLowerCase().includes(q),
  );
  for (const group of groupSessions(visible, state.workspaces, state.activeWorkspace)) {
    // The header is the workspace switcher's anchor: root on hover, bound model
    // beside the name so the panel answers "which model am I about to use?".
    const head = el("div", "ws-head");
    head.title = group.root
      ? `${group.root} — click to switch`
      : `${group.name} — click to switch`;
    if (group.id === state.activeWorkspace) head.classList.add("active");
    head.addEventListener("click", () => switchWorkspace(group.id));
    const edit = el("button", "ws-act", "✎");
    edit.setAttribute("aria-label", "Rename workspace");
    edit.title = "Rename or move this workspace";
    edit.addEventListener("click", (e) => {
      e.stopPropagation();
      openWorkspaceForm("edit", group.id);
    });
    head.appendChild(edit);
    const remove = el("button", "ws-act", "✕");
    remove.setAttribute("aria-label", "Remove workspace");
    remove.title = "Remove this workspace";
    remove.addEventListener("click", (e) => {
      e.stopPropagation();
      openWorkspaceForm("remove", group.id);
    });
    head.appendChild(remove);
    const folded = !!state.collapsed[group.id];
    const fold = el("button", "ws-act ws-fold", folded ? "▸" : "▾");
    fold.title = folded ? "Show this workspace's chats" : "Hide them";
    fold.addEventListener("click", (e) => {
      // Otherwise the click would also switch the active workspace, which is the
      // header's job, not the fold's.
      e.stopPropagation();
      state.collapsed[group.id] = !folded;
      renderSidebar();
    });
    head.appendChild(fold);
    head.appendChild(el("span", "ws-name", clip(group.name, 18)));
    if (group.model) head.appendChild(el("span", "ws-model", clip(group.model, 16)));
    head.appendChild(el("span", "ws-count", String(group.sessions.length)));
    sessionList.appendChild(head);
    // A collapsed group still counts its chats in the header; it just hides them.
    if (folded) continue;
    for (const s of group.sessions) {
    const row = el("div", "session-row" + (s.id === state.activeId ? " active" : ""));
    row.appendChild(el("span", "title", clip(s.title, 24)));
    if (streamOf(s.id)) row.appendChild(el("span", "dot", "●"));
    const age = el("span", "time", fmtAge(s.updated_at));
    age.title = fmtTime(s.updated_at);
    row.appendChild(age);
    const del = el("button", "del", "✕");
    del.setAttribute("aria-label", "Move this chat to the trash");
    del.title = "Delete chat";
    del.addEventListener("click", (e) => {
      e.stopPropagation();
      deleteSession(s.id);
    });
    row.appendChild(del);
    // A chat's workspace decides the model and capabilities that run it, so it
    // can move house without being re-created. Shown only when there is
    // somewhere to move to.
    if (state.workspaces.length > 1) {
      const picker = el("select", "del sess-ws");
      picker.title = "Move this chat to another workspace";
      for (const w of state.workspaces) {
        const option = el("option", "", clip(w.name, 18));
        option.value = w.id;
        if (w.id === s.workspace) option.selected = true;
        picker.appendChild(option);
      }
      // Without this the click would also switch to the chat behind the menu.
      picker.addEventListener("click", (e) => e.stopPropagation());
      picker.addEventListener("change", (e) => {
        e.stopPropagation();
        moveSession(s.id, picker.value);
      });
      row.appendChild(picker);
    }
    row.addEventListener("click", () => switchSession(s.id));
    sessionList.appendChild(row);
    }
  }
  if (state.wsEdit) {
    const known = state.workspaces.find((w) => w.id === state.wsEdit.id);
    sessionList.appendChild(
      workspaceFormEl(known || { id: state.wsEdit.id, name: state.wsEdit.id, root: "" }),
    );
  } else {
    const add = el("button", "ws-add", "+ New workspace");
    add.addEventListener("click", () => openWorkspaceForm("create", ""));
    sessionList.appendChild(add);
    // Presets sit next to "New workspace" because applying one is a way of
    // creating a workspace — a copy of it, never a link to it (§24.9).
    for (const preset of state.presets) {
      const from = el("button", "ws-add ws-add-preset", `▸ ${preset.name || preset.id}`);
      from.title = "Create a workspace from this preset (a copy, not a link)";
      from.addEventListener("click", () =>
        workspaceCommand(invoke("apply_preset", { id: preset.id, name: "" })),
      );
      sessionList.appendChild(from);
    }
  }
}

function renderTitle() {
  const active = state.sessions.find((s) => s.id === state.activeId);
  const title = active ? clip(active.title || "New chat", 48) : "BOS";
  chatTitle.textContent = title;
  // The document panel's Start page shows the shell's view of the session.
  Shell.setContext({ session: active ? active.title || "New chat" : "" });
}

/* ---------- markdown (escape-first, then fixed-tag templating) ----------
   Untrusted model output is HTML-escaped before any tag is added; the only
   tags in the result come from these templates. Links are limited to
   http(s), so a javascript: URL can never become clickable. */

const MD_BLOCK = String.fromCharCode(1); // fenced-code placeholder marker
const MD_CODE = String.fromCharCode(2); // inline-code placeholder marker

function escapeMd(s) {
  // Drop control characters (so model output can't forge placeholders),
  // then escape the metacharacters that matter at block level. `>` stays
  // raw here so `^>` blockquote detection still sees it — a bare `>` is
  // inert in HTML; `inlineMd` finishes the escaping per line.
  let out = "";
  for (const ch of String(s)) {
    const c = ch.codePointAt(0);
    if (c >= 32 || ch === "\n" || ch === "\t") out += ch;
  }
  return out
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/"/g, "&quot;");
}

/* Inline: `code`, **bold**, *italic*, ~~strike~~, [text](https://…).
   Input arrives already escaped except `>`; this completes escaping before
   adding tags. Inline code is parked in placeholders so emphasis never
   rewrites it. `![alt](…)` is deliberately left as literal text — images
   would leak the reader's IP to third-party hosts. */
function inlineMd(src) {
  const codes = [];
  let t = String(src)
    .replace(/>/g, "&gt;")
    .replace(/`([^`\n]+)`/g, (_, code) => {
      codes.push(code);
      return MD_CODE + (codes.length - 1) + MD_CODE;
    });
  t = t.replace(/\*\*([^\n]+?)\*\*/g, "<b>$1</b>");
  t = t.replace(/(^|[^*\w])\*([^*\n]+)\*/g, "$1<i>$2</i>");
  t = t.replace(/~~([^~\n]+)~~/g, "<s>$1</s>");
  t = t.replace(
    /(^|[^!])\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g,
    '$1<a href="$3" rel="noreferrer">$2</a>'
  );
  return t.replace(
    new RegExp(MD_CODE + "(\\d+)" + MD_CODE, "g"),
    (_, i) => "<code>" + codes[+i] + "</code>"
  );
}

/* Input is already escaped by `escapeMd`, so this only finishes `>` and
   adds fixed tags. */
function codeBlockHtml(lang, code) {
  const esc = (s) => String(s).replace(/>/g, "&gt;");
  return (
    '<pre class="md-code"><div class="code-head"><span class="code-lang">' +
    esc(lang || "code") +
    '</span><button type="button" class="code-copy">Copy</button></div><code>' +
    esc(code.replace(/\n$/, "")) +
    "</code></pre>"
  );
}

/* Small block parser: fenced code, headings, lists, quotes, rules, paragraphs. */
function renderMarkdown(src) {
  const blocks = [];
  let t = escapeMd(src);
  t = t.replace(/```([\w+#.-]*)[ \t]*\n([\s\S]*?)```/g, (_, lang, code) => {
    blocks.push([lang, code]);
    return MD_BLOCK + (blocks.length - 1) + MD_BLOCK;
  });
  const restore = (s) =>
    s.replace(
      new RegExp(MD_BLOCK + "(\\d+)" + MD_BLOCK, "g"),
      (_, i) => codeBlockHtml(blocks[+i][0], blocks[+i][1])
    );

  let html = "";
  let para = [];
  let list = null; // "ul" | "ol"
  let quote = [];
  const flushText = () => {
    if (para.length) {
      html += "<p>" + para.map(inlineMd).join("<br>") + "</p>";
      para = [];
    }
    if (quote.length) {
      html += "<blockquote>" + quote.map(inlineMd).join("<br>") + "</blockquote>";
      quote = [];
    }
  };
  const flushList = () => {
    if (list) {
      html += "</" + list + ">";
      list = null;
    }
  };
  const flush = () => {
    flushText();
    flushList();
  };

  for (const raw of t.split("\n")) {
    const line = raw.replace(/[ \t]+$/, "");
    let m;
    if (
      line.startsWith(MD_BLOCK) &&
      line.endsWith(MD_BLOCK) &&
      /^\d+$/.test(line.slice(1, -1)) &&
      blocks[+line.slice(1, -1)]
    ) {
      flush();
      html += restore(line);
    } else if (line.trim() === "") {
      flush();
    } else if ((m = line.match(/^(#{1,4})\s+(.*)$/))) {
      flush();
      const lvl = m[1].length + 1; // # -> h2, keep h1 for page chrome
      html += "<h" + lvl + ">" + inlineMd(m[2]) + "</h" + lvl + ">";
    } else if (/^(-{3,}|\*{3,}|_{3,})$/.test(line.trim())) {
      flush();
      html += "<hr>";
    } else if ((m = line.match(/^>\s?(.*)$/))) {
      flushList();
      flushText();
      quote.push(m[1]);
    } else if ((m = line.match(/^\s*[-*+]\s+(.*)$/))) {
      flushText();
      if (list && list !== "ul") flushList();
      if (!list) {
        list = "ul";
        html += "<ul>";
      }
      html += "<li>" + inlineMd(m[1]) + "</li>";
    } else if ((m = line.match(/^\s*\d+[.)]\s+(.*)$/))) {
      flushText();
      if (list && list !== "ol") flushList();
      if (!list) {
        list = "ol";
        html += "<ol>";
      }
      html += "<li>" + inlineMd(m[1]) + "</li>";
    } else {
      flushList();
      para.push(line);
    }
  }
  flush();
  return restore(html);
}

/* ---------- clipboard ---------- */

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (_e) {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch (_e2) {
      ok = false;
    }
    ta.remove();
    return ok;
  }
}

function flash(btn, msg) {
  btn.textContent = msg;
  setTimeout(() => {
    btn.textContent = "Copy";
  }, 1200);
}

/* Tool timeline card: collapsible entry per tool call. Expand state lives
   on the tool object (`t.open`) because renderMessages rebuilds every bubble
   from the session cache on each repaint. */
function toolArgsSummary(args) {
  try {
    const obj = JSON.parse(args);
    for (const key of ["cmd", "command", "path", "file_path", "query", "pattern", "url", "question", "text"]) {
      if (typeof obj[key] === "string") {
        const v = obj[key].replace(/\s+/g, " ");
        return v.length > 72 ? v.slice(0, 72) + "…" : v;
      }
    }
    const compact = JSON.stringify(obj);
    return compact.length > 72 ? compact.slice(0, 72) + "…" : compact;
  } catch (_e) {
    const flat = String(args).replace(/\s+/g, " ");
    return flat.length > 72 ? flat.slice(0, 72) + "…" : flat;
  }
}

function toolArgsFull(args) {
  try {
    return JSON.stringify(JSON.parse(args), null, 2);
  } catch (_e) {
    return String(args);
  }
}

/* Render tool output inertly (textContent per line). Unified-diff-style
   output gets the same add/del/context coloring as approval diffs; everything
   else renders plain, capped so one huge file read can't stall repaints. */
function renderToolOutput(output) {
  const box = el("div", "tool-output");
  const lines = output.split("\n");
  const MAX_LINES = 400;
  const isDiff = /^\+\+\+ |^--- |^@@ /m.test(output);
  for (const line of lines.slice(0, MAX_LINES)) {
    let cls = "tool-line";
    if (isDiff) {
      if (line.startsWith("@@") || line.startsWith("+++") || line.startsWith("---")) cls += " d-ctx";
      else if (line.startsWith("+")) cls += " d-add";
      else if (line.startsWith("-")) cls += " d-del";
    }
    box.appendChild(el("div", cls, line));
  }
  if (lines.length > MAX_LINES) {
    box.appendChild(
      el("div", "tool-line d-ctx", `… ${lines.length - MAX_LINES} more lines`)
    );
  }
  return box;
}

function buildToolCard(t) {
  const card = el("div", "tool-card");
  const head = el("button", "tool-head");
  head.type = "button";
  head.setAttribute("aria-expanded", t.open ? "true" : "false");
  head.appendChild(el("span", "tool-name", t.name));
  head.appendChild(el("span", "tool-args", toolArgsSummary(t.args)));
  const done = t.output != null;
  head.appendChild(
    el("span", "tool-badge" + (done ? " done" : " running"),
      done ? (t.ms != null ? `${t.ms} ms` : "done") : "running…")
  );
  card.appendChild(head);
  if (t.open) {
    const body = el("div", "tool-body");
    body.appendChild(el("div", "tool-args-full", toolArgsFull(t.args)));
    if (done) {
      body.appendChild(renderToolOutput(t.output));
      // The card stays a summary; the right panel is where a full artifact is
      // read. Tool output is untrusted, so the panel renders it inert.
      const open = el("button", "mini-btn tool-open", "Open in panel");
      open.type = "button";
      open.title = "Open this result in the document panel";
      open.addEventListener("click", () => {
        Shell.openDocument({
          id: "tool:" + toolKey(t),
          title: t.name,
          path: toolPath(t),
          text:
            typeof t.output === "string"
              ? t.output
              : JSON.stringify(t.output, null, 2),
        });
      });
      body.appendChild(open);
    } else {
      body.appendChild(el("div", "tool-pending", "waiting for result…"));
    }
    card.appendChild(body);
  }
  head.addEventListener("click", () => {
    t.open = !t.open;
    // Expanding a card changes that row's signature and the row does not know
    // its own index, so a click invalidates the whole validation cursor.
    markDirtyAll();
    scheduleRepaint();
  });
  return card;
}

function buildBubble(m, showCaret, opts) {
  const bubble = el("div", "bubble");
  if (m.reasoning) bubble.appendChild(el("div", "reasoning", m.reasoning));
  for (const t of m.tools || []) {
    bubble.appendChild(buildToolCard(t));
  }
  if (m.text || showCaret) {
    const text = el("div", "text");
    if (m.text) {
      if (m.role === "User") {
        // Own words render verbatim; only assistant output gets markdown.
        text.appendChild(document.createTextNode(m.text));
      } else {
        text.innerHTML = renderMarkdown(m.text);
      }
    }
    if (showCaret) text.appendChild(el("span", "caret"));
    bubble.appendChild(text);
  } else if (!m.reasoning && !(m.tools || []).length && !m.error) {
    bubble.appendChild(el("div", "text", "…"));
  }
  if (m.error) bubble.appendChild(el("div", "err", "⚠ " + m.error));
  // Message actions (hover-revealed): copy anything with text; regenerate
  // the last reply; edit/delete only when opts gates them (idle chat).
  const acts = el("div", "bubble-actions");
  let hasActs = false;
  if (m.text) {
    const copyBtn = el("button", "mini-btn", "Copy");
    copyBtn.type = "button";
    copyBtn.addEventListener("click", async () => {
      const ok = await copyText(m.text);
      flash(copyBtn, ok ? "Copied" : "Copy failed");
    });
    acts.appendChild(copyBtn);
    hasActs = true;
  }
  if (opts && opts.regen) {
    const regenBtn = el("button", "mini-btn", "Regenerate");
    regenBtn.type = "button";
    regenBtn.addEventListener("click", () => regenerate());
    acts.appendChild(regenBtn);
    hasActs = true;
  }
  if (opts && opts.edit) {
    const editBtn = el("button", "mini-btn", "Edit");
    editBtn.type = "button";
    editBtn.addEventListener("click", () => {
      state.editing = { sessionId: state.activeId, index: opts.index };
      renderMessages();
    });
    acts.appendChild(editBtn);
    hasActs = true;
  }
  if (opts && opts.del) {
    const delBtn = el("button", "mini-btn", "Delete");
    delBtn.type = "button";
    delBtn.addEventListener("click", () => deleteFrom(opts.index));
    acts.appendChild(delBtn);
    hasActs = true;
  }
  if (opts && opts.branch) {
    const branchBtn = el("button", "mini-btn", "Branch");
    branchBtn.type = "button";
    branchBtn.title = "Fork this chat at this message into a new session";
    branchBtn.addEventListener("click", () => branchFrom(opts.index));
    acts.appendChild(branchBtn);
    hasActs = true;
  }
  if (hasActs) bubble.appendChild(acts);
  return bubble;
}

/* Edit-and-resend: the backend truncates the stored history at `index`,
   appends the edited prompt as a fresh turn, and the agent re-seeds from
   the surviving prefix. sendText rolls the tail back if the invoke fails. */
function resendEdited(index, text) {
  state.editing = null;
  sendText(text, state.activeId, index).catch(() => {});
}

/* "Delete from here": drop message `index` and everything after it from
   the stored transcript, then reload the shortened view. */
async function deleteFrom(index) {
  const id = state.activeId;
  if (!id || index == null) return;
  if (streamOf(id)) {
    toast("A turn is streaming — wait for it to finish before deleting.");
    return;
  }
  try {
    await invoke("truncate_session", { sessionId: id, index });
    const msgs = state.cache[id];
    if (msgs) msgs.splice(index);
    state.editing = null; // any open editor now points into the dropped tail
    renderAll();
    toast("Messages deleted");
  } catch (err) {
    toast(`Delete failed: ${err}`);
  }
}

/* "Branch here": fork the prefix before `index` into a brand-new session
   and switch to it — unlike delete, this chat's transcript is untouched,
   so both histories survive. The fork arrives with its messages and plan
   snapshot, so the sidebar row, cache, and plan panel are all primed. */
async function branchFrom(index) {
  const id = state.activeId;
  if (!id || index == null) return;
  if (streamOf(id)) {
    toast("A turn is streaming — wait for it to finish before branching.");
    return;
  }
  try {
    const fork = await invoke("fork_session", { sessionId: id, index });
    state.editing = null; // any open editor belongs to the old chat now
    state.cache[fork.id] = fork.messages;
    state.sessions.unshift(fork);
    state.activeId = fork.id;
    await restorePlan(fork.id);
    renderAll();
    toast(`Branched into "${fork.title}"`);
  } catch (err) {
    toast(`Branch failed: ${err}`);
  }
}

/* The inline editor that replaces a user bubble while editing it. */
function buildEditor(m, i) {
  const box = el("div", "bubble edit-box");
  const ta = document.createElement("textarea");
  ta.className = "edit-input";
  ta.value = m.text;
  ta.setAttribute("aria-label", "Edit message");
  box.appendChild(ta);
  const row = el("div", "bubble-actions");
  const save = el("button", "mini-btn", "Send");
  save.type = "button";
  save.addEventListener("click", () => {
    const v = ta.value.trim();
    if (!v) return;
    resendEdited(i, v);
  });
  const cancel = el("button", "mini-btn", "Cancel");
  cancel.type = "button";
  cancel.addEventListener("click", () => {
    state.editing = null;
    renderMessages();
  });
  ta.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      state.editing = null;
      renderMessages();
    } else if (ev.key === "Enter" && (ev.metaKey || ev.ctrlKey)) {
      ev.preventDefault();
      save.click();
    }
  });
  row.appendChild(save);
  row.appendChild(cancel);
  box.appendChild(row);
  setTimeout(() => {
    ta.focus();
    ta.setSelectionRange(ta.value.length, ta.value.length);
  }, 0);
  return box;
}

/* ---- incremental transcript ----

   Streaming delivers dozens of token events per frame. Rebuilding every row
   on each repaint is what made the transcript expensive; instead rows are
   cached per message identity and reused unless their content signature
   changed, so one token patches one row. */

const MSG_KEY = Symbol("msgKey");
let msgKeySeq = 0;

/* Stable identity for a message object (non-enumerable so it never leaks into
   payloads). Renamed/loaded objects simply get fresh keys and rebuild once. */
function msgKey(m) {
  let key = m[MSG_KEY];
  if (key === undefined) {
    key = ++msgKeySeq;
    Object.defineProperty(m, MSG_KEY, { value: key, enumerable: false });
  }
  return key;
}

/* Cheap content signature: length plus a short tail, so a same-length
   replacement (an edit, a tool result overwrite) still invalidates the row. */
function sigOf(s) {
  const t = s || "";
  return `${t.length}:${t.slice(-24)}`;
}

function messageSignature(m, opts, editingHere) {
  let tools = "";
  for (const t of m.tools || []) {
    tools += `${sigOf(t.name)}|${sigOf(t.args)}|${
      t.output == null ? "-" : sigOf(t.output)
    }|${t.ms == null ? "-" : t.ms}|${t.open ? "o" : "c"};`;
  }
  return [
    m.role,
    sigOf(m.text),
    sigOf(m.reasoning),
    sigOf(m.error),
    tools,
    opts.showCaret ? 1 : 0,
    opts.regen ? 1 : 0,
    opts.edit ? 1 : 0,
    opts.del ? 1 : 0,
    opts.branch ? 1 : 0,
    editingHere ? 1 : 0,
  ].join("~");
}

/* Stable identity for one tool entry, used as the document id when its result
   is opened in the right panel (same non-enumerable trick as messages). */
const TOOL_KEY = Symbol("toolKey");
let toolKeySeq = 0;

function toolKey(t) {
  let key = t[TOOL_KEY];
  if (key === undefined) {
    key = ++toolKeySeq;
    Object.defineProperty(t, TOOL_KEY, { value: key, enumerable: false });
  }
  return key;
}

/* Best-effort display path for a tool call. The value is agent-authored and is
   only ever shown as text; it is never used to read anything on its own. */
function toolPath(t) {
  const args = t.args && typeof t.args === "object" ? t.args : null;
  const p = args && (args.path || args.file || args.filename || args.target);
  return typeof p === "string" ? p : "";
}

/* Cached rows between repaints: message key -> { el, sig }. */
const rowCache = new Map();
let emptyRendered = false;

/* Validation cursor: rows above `dirtyFrom` are reused without recomputing their
   signatures. A streaming token only touches the tail, so the per-frame cost stops
   growing with the transcript. Any in-place mutation of an older row must call
   `markDirty` (which only ever moves the cursor backwards). */
let dirtyFrom = 0;
let lastEpoch = null;
/* The last rendered transcript identity, used to decide whether the leading
   rows may be trusted without looking at them. */
let lastMsgsRef = null;
let lastMsgsLen = -1;
let lastTailRef = null;

function markDirty(index) {
  if (index < dirtyFrom) dirtyFrom = index;
}

function markDirtyAll() {
  dirtyFrom = 0;
}

/* Tail-follow is maintained from scroll events instead of being measured on
   every repaint: asking "is the reader at the bottom?" with `scrollHeight`
   forces a layout, and the streaming repaint is the hot path. Sticking to the
   bottom writes an out-of-range value that the browser clamps — a write, so no
   read either. */
const TAIL_SLACK = 24;
let followTail = true;
let followSession = null;
let scrollFromUs = false;
let scrollClearTimer = 0;

messagesEl.addEventListener(
  "scroll",
  () => {
    // Our own stick-to-bottom write also lands here; ignore it rather than
    // measure the geometry we just moved.
    if (scrollFromUs) return;
    followTail =
      messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight <=
      TAIL_SLACK;
  },
  { passive: true },
);

function stickToBottom() {
  followTail = true;
  scrollFromUs = true;
  messagesEl.scrollTop = Number.MAX_SAFE_INTEGER;
  // Clear the mark even if the write produced no scroll event at all.
  if (!scrollClearTimer) {
    scrollClearTimer = setTimeout(() => {
      scrollClearTimer = 0;
      scrollFromUs = false;
    }, 120);
  }
}

function renderMessages() {
  const msgs = activeMsgs();
  // A session swap opens at the newest message, like a chat client should.
  if (followSession !== state.activeId) {
    followSession = state.activeId;
    followTail = true;
    markDirtyAll();
  }
  const prevTop = followTail ? 0 : messagesEl.scrollTop;

  if (msgs.length === 0) {
    if (!emptyRendered) {
      emptyRendered = true;
      rowCache.clear();
      messagesEl.textContent = "";
      const hint = el("div", "empty-hint");
      hint.appendChild(el("div", "", "New conversation"));
      hint.appendChild(el("div", "", "Type a message below to begin."));
      messagesEl.appendChild(hint);
    }
    return;
  }
  emptyRendered = false;

  const activeStream = streamOf(state.activeId);
  const streamingHere = !!activeStream;
  // Editing/deleting a stored message is only safe while this chat is idle:
  // no stream and no compaction rewriting the same transcript underneath.
  const idle = !activeStream && !state.compacting[state.activeId];
  const editing =
    state.editing && state.editing.sessionId === state.activeId ? state.editing : null;

  // Regenerate applies to the last assistant reply once this chat is idle
  // (a background chat streaming never blocks it).
  let lastAssistant = -1;
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === "Assistant") {
      lastAssistant = i;
      break;
    }
  }

  // Presentation flags decide per-row affordances (regenerate, edit, delete,
  // branch) and the caret, so a change in any of them invalidates every row.
  const epoch = `${idle ? 1 : 0}${streamingHere ? 1 : 0}${editing ? editing.index : -1}`;
  if (epoch !== lastEpoch) {
    lastEpoch = epoch;
    markDirtyAll();
  }
  // Shape check: same array, same length, same tail object, a cache entry and a
  // DOM row per message. Only then may the leading rows be trusted without being
  // looked at — a splice, a replacement, a load or compaction all fall back to
  // the full pass.
  const windowLen = Math.min(msgs.length, WINDOW + (state.windowExtra || 0));
  const windowStart = msgs.length - windowLen;
  const delta = lastWindowStart < 0 ? 0 : windowStart - lastWindowStart;
  const slid =
    msgs === lastMsgsRef &&
    lastWindowLen >= 0 &&
    delta >= 0 &&
    msgs.length - lastMsgsLen === delta &&
    rowCache.size === lastWindowLen &&
    messagesEl.childElementCount === lastChildCount &&
    (msgs.length === 0 || msgs[msgs.length - 1] === lastTailRef);
  if (slid && delta > 0) {
    for (let k = 0; k < delta; k++) {
      const key = msgKey(msgs[lastWindowStart + k]);
      const gone = rowCache.get(key);
      if (gone) {
        gone.el.remove();
        rowCache.delete(key);
      }
    }
  }
  const sameShape = slid && windowLen === lastWindowLen + delta;
  const from = sameShape ? Math.max(0, Math.min(dirtyFrom - windowStart, windowLen)) : 0;

  if (sameShape && dirtyFrom >= msgs.length) {
    // Nothing was marked and the shape is unchanged: the DOM already matches, so
    // the frame costs a scroll write and nothing else.
    if (followTail) stickToBottom();
    else messagesEl.scrollTop = prevTop;
    return;
  }

  // Reuse cached rows, rebuilding only the ones whose signature changed. With a
  // trusted prefix the walk starts at the cursor, so a streamed token costs one
  // row instead of one row per message in the transcript.
  const desired = [];
  const seen = from === 0 ? new Set() : null;
  for (let i = from; i < windowLen; i++) {
    const abs = windowStart + i;
    const m = msgs[abs];
    const showCaret = streamingHere && abs === msgs.length - 1;
    const editingHere = !!(editing && editing.index === abs);
    const opts = {
      index: abs,
      regen: idle && abs === lastAssistant && !!m.text,
      edit: idle && m.role === "User" && !!m.text,
      del: idle,
      branch: idle && abs > 0,
    };
    const key = msgKey(m);
    if (seen) seen.add(key);
    const sig = messageSignature(m, { ...opts, showCaret }, editingHere);
    let entry = rowCache.get(key);
    if (!entry || entry.sig !== sig) {
      const row = el("div", m.role === "User" ? "user" : "assistant");
      row.classList.add("msg");
      if (editingHere) {
        row.appendChild(buildEditor(m, abs));
      } else {
        row.appendChild(buildBubble(m, showCaret, opts));
      }
      // Swap the rebuilt row in place, so a change mid-transcript cannot
      // shift every row after it (a tool result landing in message 2 of 5000).
      if (entry && entry.el.parentNode === messagesEl) entry.el.replaceWith(row);
      entry = { el: row, sig };
      rowCache.set(key, entry);
    }
    desired.push(entry.el);
  }

  // Forget rows whose message left the transcript (edit/delete/fork/compaction
  // or a session swap); their DOM nodes go with them. A trusted prefix cannot
  // have lost a message, so this only runs on a full pass.
  if (seen && rowCache.size > seen.size) {
    for (const [key, entry] of rowCache) {
      if (!seen.has(key)) {
        entry.el.remove();
        rowCache.delete(key);
      }
    }
  }

  // Reconcile order with as few moves as possible: a streaming frame normally
  // moves nothing at all, because the growing row is already the last child.
  // With a trusted prefix the walk starts at the cursor instead of the top.
  let node = from > 0 ? desired[0] : messagesEl.firstChild;
  // The spacer lives inside the scroller (that is the point of it) but is not a
  // row, so the walk steps over it and the trim below never treats it as extra.
  if (topSpacerEl && node === topSpacerEl) node = node.nextSibling;
  for (const want of desired) {
    if (node === want) {
      node = want.nextSibling;
      continue;
    }
    messagesEl.insertBefore(want, node);
  }
  while (node) {
    const next = node.nextSibling;
    node.remove();
    node = next;
  }

  // Everything on screen is validated now; the next frame starts with an empty
  // dirty set, and only the tail is re-checked when a mutation marks it.
  lastMsgsRef = msgs;
  lastMsgsLen = msgs.length;
  lastWindowLen = windowLen;
  lastWindowStart = windowStart;
  lastTailRef = msgs.length ? msgs[msgs.length - 1] : null;
  dirtyFrom = msgs.length;

  renderLoadEarlier(windowStart);
  renderTopSpacer(windowStart);
  lastChildCount = messagesEl.childElementCount;
  if (followTail) stickToBottom();
  else {
    // Rows the spacer just handed back sit above the viewport, so add their
    // estimated height: without this, growing the window would jump the reader.
    messagesEl.scrollTop = prevTop + spacerCredit;
  }
  spacerCredit = 0;
  // Height sampling forces layout, so it never happens while a response is
  // streaming — that path is guarded at zero layout reads per frame.
  if (!streamingHere) sampleRowHeight();
}

/* The un-rendered rows above the window still have to occupy scroll space, or
   the scrollbar would describe only the window and the conversation above it
   would be unreachable. The height is an estimate from measured rows, so the
   bar is approximate while the window is partial and exact once it reaches the
   start of the chat. */
function renderTopSpacer(windowStart) {
  if (!(windowStart > 0)) {
    if (topSpacerEl) {
      topSpacerEl.remove();
      topSpacerEl = null;
    }
    return;
  }
  if (!topSpacerEl) {
    topSpacerEl = el("div", "spacer-top", "");
    topSpacerEl.setAttribute("aria-hidden", "true");
  }
  if (messagesEl.firstChild !== topSpacerEl) messagesEl.insertBefore(topSpacerEl, messagesEl.firstChild);
  const px = Math.round(windowStart * avgRowPx);
  const want = `${px}px`;
  // Credit the exact height the spacer changes by, not an estimate: the row
  // average is re-measured from what is on screen, so estimating here would
  // credit 200 rows at the new average while the spacer gave up 200 rows at the
  // old one, and the reader would jump by the difference.
  spacerCredit += lastSpacerPx - px;
  lastSpacerPx = px;
  if (topSpacerEl.style.height !== want) topSpacerEl.style.height = want;
}

/* Average a few rows to size the spacer. A sample rather than every row, because
   this is a layout read and the transcript is a hot path. */
function sampleRowHeight() {
  const rows = [...rowCache.values()].map((e) => e.el).filter((n) => n && n.isConnected);
  if (rows.length < 2) return;
  const step = Math.max(1, Math.floor(rows.length / 8));
  let sum = 0;
  let n = 0;
  for (let i = 0; i < rows.length; i += step) {
    const h = rows[i].offsetHeight;
    if (h > 0) {
      sum += h;
      n += 1;
    }
  }
  if (n) avgRowPx = sum / n;
}

/* Scrolling near the top grows the window, one window at a time, so reading
   backwards off the start of a long chat needs no button. */
function ensureScrollGrow() {
  if (scrollGrowBound || !messagesEl) return;
  scrollGrowBound = true;
  messagesEl.addEventListener("scroll", () => {
    if (messagesEl.scrollTop > 320 || lastWindowStart <= 0) return;
    state.windowExtra = (state.windowExtra || 0) + WINDOW;
    markDirtyAll();
    scheduleRepaint();
  });
}

function windowStartOfDebug() { return lastWindowStart; }

function renderLoadEarlier(windowStart) {
  if (!(windowStart > 0)) {
    if (loadEarlierEl) {
      loadEarlierEl.remove();
      loadEarlierEl = null;
    }
    return;
  }
  if (!loadEarlierEl) {
    loadEarlierEl = el("button", "load-earlier", "");
    loadEarlierEl.addEventListener("click", () => {
      state.windowExtra = (state.windowExtra || 0) + WINDOW;
      markDirtyAll();
      scheduleRepaint();
    });
    const host = messagesEl && messagesEl.parentNode;
    if (!host) return;
    host.insertBefore(loadEarlierEl, messagesEl);
  }
  const shown = Math.min(WINDOW, windowStart);
  const label = `Load ${shown} earlier message${shown === 1 ? "" : "s"}`;
  if (loadEarlierEl.textContent !== label) loadEarlierEl.textContent = label;
}

ensureScrollGrow();

function renderStatus() {
  renderUsage();
  statusLeft.textContent = "";
  statusLeft.appendChild(el("span", "", state.model || "no model"));
  statusLeft.appendChild(
    el("span", "", `${state.sessions.length} chat${state.sessions.length === 1 ? "" : "s"}`)
  );
  renderSessionStats(statusLeft, state.overview);

  statusRight.textContent = "";
  renderArchived(statusRight, state.archived, () => void openArchivedView(), restoreArchived);
  if (state.toolCount > 0) {
    statusRight.appendChild(
      el("span", "tools", `⚙ ${state.toolCount} tool call${state.toolCount === 1 ? "" : "s"}`)
    );
  }
  if (state.compacting[state.activeId]) {
    statusRight.appendChild(el("span", "ctx", "⧗ compacting…"));
  }
  if (state.initializing) {
    statusRight.appendChild(el("span", "ctx", "⧗ generating AGENTS.md…"));
  }
  const mcp = state.mcpStatus;
  if (mcp && mcp.servers && mcp.servers.length) {
    const enabled = mcp.servers.filter((s) => s.enabled).length;
    const on = mcp.servers.filter((s) => s.connected).length;
    const text = mcp.connecting ? "⛁ mcp connecting…" : `⛁ mcp ${on}/${enabled}`;
    const failed = mcp.servers.find((s) => s.enabled && !s.connected);
    const span = el("span", failed && !mcp.connecting ? "err" : "mcp", text);
    if (failed && failed.error) span.title = failed.error;
    statusRight.appendChild(span);
  }
  const budget = state.budget || 0;
  if (budget > 0) {
    const used = estimateContext();
    const ratio = used / budget;
    const cls =
      ratio >= 0.9 ? "ctx ctx-err" : ratio >= 0.7 ? "ctx ctx-warn" : "ctx ctx-ok";
    const span = el("span", cls, `ctx ~${fmtTokens(used)}/${fmtTokens(budget)}`);
    span.title =
      `Estimated outgoing context (~${used} of ${budget} token budget); ` +
      "older messages are compacted into a summary above the budget";
    statusRight.appendChild(span);
  }
  if (state.usage) {
    const u = state.usage;
    statusRight.appendChild(
      el("span", "usage", `↑ ${u.prompt} ↓ ${u.completion} tok`)
    );
  }
  if (state.error) {
    statusRight.appendChild(el("span", "err", clip(state.error, 160)));
  }
}

function renderSend() {
  if (streamOf(state.activeId)) {
    sendBtn.textContent = "■ Stop";
    sendBtn.classList.add("stop");
    sendBtn.disabled = false;
  } else {
    sendBtn.textContent = "Send";
    sendBtn.classList.remove("stop");
    sendBtn.disabled = !input.value.trim();
  }
}

/* Follow-ups typed while a turn is streaming wait in `state.queue` and are
   sent FIFO as soon as that chat's stream ends. One row per waiting message,
   each removable; built with textContent so message text stays inert. */
function renderQueue() {
  const q = state.queue[state.activeId] || [];
  queueList.replaceChildren();
  if (!q.length) {
    queueList.classList.add("hidden");
    return;
  }
  q.forEach((text, i) => {
    const row = document.createElement("div");
    row.className = "queue-item";
    const label = document.createElement("span");
    label.className = "queue-text";
    label.textContent = `queued · ${text}`;
    const drop = document.createElement("button");
    drop.type = "button";
    drop.className = "queue-x";
    drop.title = "Remove from queue";
    drop.textContent = "✕";
    drop.addEventListener("click", () => {
      const cur = state.queue[state.activeId] || [];
      cur.splice(i, 1);
      if (!cur.length) delete state.queue[state.activeId];
      renderQueue();
      renderSend();
    });
    row.append(label, drop);
    queueList.appendChild(row);
  });
  queueList.classList.remove("hidden");
}

function tick() {
  const here = streamOf(state.activeId);
  const others = otherStreams();
  // The shell owns the status chip; the app supplies the live detail and the
  // task identity (session + generation) so state transitions stay ordered.
  let detail = "";
  if (here) {
    const secs = Math.max(0, Math.floor(Date.now() / 1000) - here.started_at);
    detail =
      others.length > 0
        ? `streaming · ${secs}s · +${others.length} chat${others.length === 1 ? "" : "s"}`
        : `streaming · ${secs}s`;
    Shell.applyTaskEvent({
      taskId: here.session_id,
      type: "progress",
      seq: here.gen,
      timestamp: Date.now(),
    });
  } else if (others.length > 0) {
    detail =
      others.length === 1
        ? "another chat is streaming"
        : `${others.length} chats streaming`;
    Shell.setTaskStatus("Running");
  } else {
    Shell.setTaskStatus("Idle");
  }
  Shell.setTaskDetail(detail);
}

function renderAll() {
  renderSidebar();
  renderTitle();
  renderMessages();
  renderStatus();
  renderSend();
  renderQueue();
  tick();
}

/* ---------- working plan ---------- */

const PLAN_ICONS = { pending: "☐", in_progress: "◐", completed: "☑" };

function nextPlanStatus(s) {
  return s === "pending" ? "in_progress" : s === "in_progress" ? "completed" : "pending";
}

function renderPlan(items) {
  const panel = $("plan-panel");
  const list = $("#plan-list");
  const editBtn = $("plan-edit");
  if (!panel || !list) return;
  // A stale footer from a previous edit pass never survives a render.
  panel.querySelectorAll(".plan-edit-foot").forEach((n) => n.remove());
  if (state.planEditing && state.planDraft) {
    // Edit mode: the draft wins over any refresh racing it (the footer and
    // the Cancel path own exiting; the header button hides until saved).
    if (editBtn) editBtn.classList.add("hidden");
    renderPlanEditor(panel, list);
    panel.classList.remove("hidden");
    return;
  }
  state.plan = items || [];
  const has = state.plan.length > 0;
  if (editBtn) {
    // Editing races a mid-stream update_plan tool call, so it only shows
    // once this chat is fully idle.
    const editable =
      has && !streamOf(state.activeId) && !state.compacting[state.activeId];
    editBtn.classList.toggle("hidden", !editable);
  }
  if (!has) {
    list.replaceChildren();
    panel.classList.add("hidden");
    return;
  }
  let done = 0;
  let active = 0;
  const nodes = state.plan.map((it) => {
    if (it.status === "completed") done += 1;
    if (it.status === "in_progress") active += 1;
    const li = document.createElement("li");
    li.className = "plan-item status-" + it.status;
    const icon = document.createElement("span");
    icon.className = "plan-icon";
    icon.textContent = PLAN_ICONS[it.status] || PLAN_ICONS.pending;
    const text = document.createElement("span");
    text.className = "plan-text";
    text.textContent = it.text;
    li.append(icon, text);
    return li;
  });
  list.replaceChildren(...nodes);
  const count = $("#plan-count");
  if (count) {
    count.textContent =
      done + "/" + state.plan.length + (active ? " · " + active + " active" : "");
  }
  panel.classList.remove("hidden");
}

/* Reorder helper for the plan editor: swap draft step `i` one slot in
   `dir` (-1 = up, +1 = down), re-render the rows, and put the caret back
   on the step that moved so repeated presses chain smoothly. Draft-local
   until Save persists through `set_plan`. */
function movePlanStep(i, dir) {
  const draft = state.planDraft;
  const j = i + dir;
  if (!draft || j < 0 || j >= draft.length) return;
  [draft[i], draft[j]] = [draft[j], draft[i]];
  renderPlan(state.plan);
  const moved = document.querySelectorAll("#plan-list .plan-edit-input")[j];
  if (moved) {
    moved.focus();
    moved.setSelectionRange(moved.value.length, moved.value.length);
  }
}

/* Editable rows: each step is a text input, a status cycler, up/down move
   buttons, and a remove button; the footer carries add/save/cancel.
   Everything stays draft-local until Save persists through `set_plan`. */
function renderPlanEditor(panel, list) {
  const draft = state.planDraft;
  let done = 0;
  let active = 0;
  const nodes = draft.map((it, i) => {
    if (it.status === "completed") done += 1;
    if (it.status === "in_progress") active += 1;
    const li = document.createElement("li");
    li.className = "plan-item plan-edit-row";
    const input = document.createElement("input");
    input.className = "plan-edit-input";
    input.type = "text";
    input.value = it.text;
    input.placeholder = "Step description";
    input.setAttribute("aria-label", `Plan step ${i + 1}`);
    input.addEventListener("input", () => {
      draft[i].text = input.value;
    });
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        savePlanEdit();
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        cancelPlanEdit();
      } else if (ev.altKey && !ev.shiftKey && !ev.ctrlKey &&
                 (ev.key === "ArrowUp" || ev.key === "ArrowDown")) {
        // Reorder without leaving the keyboard (same gesture as the buttons).
        ev.preventDefault();
        movePlanStep(i, ev.key === "ArrowUp" ? -1 : 1);
      }
    });
    const status = document.createElement("button");
    status.className = "mini-btn plan-status-btn status-" + it.status;
    status.type = "button";
    status.title = "Cycle status (pending → active → done)";
    status.textContent = PLAN_ICONS[it.status] || PLAN_ICONS.pending;
    status.addEventListener("click", () => {
      it.status = nextPlanStatus(it.status);
      renderPlan(state.plan);
    });
    const up = document.createElement("button");
    up.className = "mini-btn";
    up.type = "button";
    up.textContent = "↑";
    up.title = "Move step up (Alt+↑)";
    up.disabled = i === 0;
    up.addEventListener("click", () => movePlanStep(i, -1));
    const down = document.createElement("button");
    down.className = "mini-btn";
    down.type = "button";
    down.textContent = "↓";
    down.title = "Move step down (Alt+↓)";
    down.disabled = i === draft.length - 1;
    down.addEventListener("click", () => movePlanStep(i, 1));
    const del = document.createElement("button");
    del.className = "mini-btn";
    del.type = "button";
    del.textContent = "×";
    del.title = "Remove step";
    del.addEventListener("click", () => {
      draft.splice(i, 1);
      renderPlan(state.plan);
    });
    li.append(input, status, up, down, del);
    return li;
  });
  list.replaceChildren(...nodes);

  const count = $("#plan-count");
  if (count) {
    count.textContent =
      done + "/" + draft.length + (active ? " · " + active + " active" : "");
  }

  const foot = document.createElement("div");
  foot.className = "plan-edit-foot";
  const add = document.createElement("button");
  add.className = "mini-btn";
  add.type = "button";
  add.textContent = "+ Add step";
  add.addEventListener("click", () => {
    state.planDraft.push({ text: "", status: "pending" });
    renderPlan(state.plan);
    const inputs = list.querySelectorAll(".plan-edit-input");
    const last = inputs[inputs.length - 1];
    if (last) last.focus();
  });
  const save = document.createElement("button");
  save.className = "mini-btn";
  save.type = "button";
  save.textContent = "Save";
  save.addEventListener("click", () => savePlanEdit());
  const cancel = document.createElement("button");
  cancel.className = "mini-btn";
  cancel.type = "button";
  cancel.textContent = "Cancel";
  cancel.addEventListener("click", () => cancelPlanEdit());
  foot.append(add, save, cancel);
  panel.appendChild(foot);
}

/* Enter edit mode: clone the rendered plan into a draft (adding a blank
   first row for an empty panel so the flow is self-starting). */
function startPlanEdit() {
  if (state.planEditing) {
    cancelPlanEdit();
    return;
  }
  state.planDraft = state.plan.map((it) => ({ text: it.text, status: it.status }));
  if (!state.planDraft.length) state.planDraft.push({ text: "", status: "pending" });
  state.planEditing = true;
  renderPlan(state.plan);
}

/* Persist the draft; the backend sanitizes (trim, cap, drop blanks) and
   returns the canonical plan, which replaces both view and draft. */
async function savePlanEdit() {
  const id = state.activeId;
  const items = state.planDraft
    .map((it) => ({ text: it.text.trim(), status: it.status }))
    .filter((it) => it.text);
  state.planEditing = false;
  state.planDraft = null;
  if (!id) {
    renderPlan([]);
    return;
  }
  try {
    const saved = await invoke("set_plan", { sessionId: id, items });
    renderPlan(saved);
    toast("Plan updated");
  } catch (err) {
    toast(`Plan save failed: ${err}`);
    refreshPlan(); // fall back to the server's truth
  }
}

function cancelPlanEdit() {
  state.planEditing = false;
  state.planDraft = null;
  renderPlan(state.plan);
}

/* Live plan from the active session's agent — works mid-stream. */
async function refreshPlan() {
  if (!state.activeId) return;
  try {
    renderPlan(await invoke("get_plan", { sessionId: state.activeId }));
  } catch (_) {
    /* keep the current panel on a transient failure */
  }
}

/* Hydrate a session's own agent with its persisted plan (start / switch).
   Switching sessions also drops any half-finished plan edit — the draft
   belongs to the session it was opened in. */
async function restorePlan(id) {
  state.planEditing = false;
  state.planDraft = null;
  try {
    renderPlan(await invoke("restore_plan", { id }));
  } catch (_) {
    renderPlan([]);
  }
}

/* ---------- session management ---------- */

async function refreshSessions() {
  let list = await invoke("list_sessions");
  if (list.length === 0) list = [await invoke("create_session")];
  state.sessions = list;
  if (!list.some((s) => s.id === state.activeId)) state.activeId = list[0].id;
  if (!state.cache[state.activeId]) await loadActive();
  // The active session may have changed (delete, restart, stream end);
  // re-point the shared plan store at it so the panel matches.
  await restorePlan(state.activeId);
}

async function loadActive() {
  try {
    state.cache[state.activeId] = await invoke("load_session", { id: state.activeId });
  } catch (_) {
    state.cache[state.activeId] = [];
  }
  void loadArchived(state.activeId);
  void loadOverview(state.activeId);
}

function ensureSessionCache(id) {
  const msgs = state.cache[id];
  if (msgs) return msgs;
  invoke("load_session", { id })
    .then((fresh) => {
      if (!state.cache[id]) state.cache[id] = fresh;
      if (id === state.activeId) renderMessages();
    })
    .catch(() => {});
  return null;
}

async function switchSession(id) {
  if (id === state.activeId) return;
  state.activeId = id;
  state.promptNav = null; // the walk belonged to the previous chat
  if (!state.cache[id]) await loadActive();
  await restorePlan(id);
  renderAll();
}

async function newChat() {
  const s = await invoke("create_session");
  state.sessions.unshift(s);
  state.activeId = s.id;
  state.cache[s.id] = [];
  state.usage = null;
  state.toolCount = 0;
  state.error = null;
  state.planEditing = false; // any open plan edit belonged to the old chat
  state.planDraft = null;
  renderPlan([]);
  renderAll();
  input.focus();
}

async function deleteSession(id) {
  if (!window.confirm("Move this chat to the trash? You can restore it from settings."))
    return;
  await invoke("delete_session", { id }).catch(() => {});
  delete state.cache[id];
  delete state.streams[id]; // backend stopped it if it was streaming
  delete state.queue[id];
  await refreshSessions();
  renderAll();
}

/* ---------- sending ---------- */

/* Stop just this chat's turn; background streams keep going. The Stop button
   takes this path, and a stop also cancels that chat's waiting queue. */
async function stopStream() {
  const id = state.activeId;
  delete state.queue[id];
  await invoke("stop_streaming", { sessionId: id }).catch(() => {});
  // stop_streaming denies this chat's pending approvals server-side.
  clearApprovals(id);
  renderQueue();
}

async function send() {
  const text = input.value;
  if (!text.trim()) return;
  closePalette();
  closeMention();
  input.value = "";
  state.promptNav = null; // a send ends the history walk (list moves on)
  autosize();
  renderSend();
  if (runCommand(text)) return;
  if (state.compacting[state.activeId]) {
    // /compact rewrites history when the summary lands; hold new turns out
    // of it so the backend never has to discard a just-finished summary.
    input.value = text;
    autosize();
    renderSend();
    toast("Compacting… wait for the summary to land before sending.");
    return;
  }
  if (streamOf(state.activeId)) {
    // A turn is in flight: keep it running and wait in line — queued text
    // is sent FIFO the moment this chat's stream ends — instead of stopping.
    const q = state.queue[state.activeId] || (state.queue[state.activeId] = []);
    if (q.length >= MAX_QUEUE) {
      // Give the text back: typed content is never dropped silently.
      input.value = text;
      autosize();
      state.error = `Queue full (${MAX_QUEUE} messages waiting) — wait for the turn to finish or press Stop.`;
      renderAll();
      return;
    }
    q.push(text);
    renderQueue();
    return;
  }
  await sendText(text);
}

/* Push the local (user, assistant) pair, then ask the backend to stream.
   On failure the pair is rolled back and the prompt restored to the input
   (active chat only — a flushed background follow-up never hijacks the
   composer). Returns whether the backend accepted the turn, so queue
   flushing can tell a real failure from a clean hand-off.

   `truncateFrom` (edit-and-resend) drops every local message from that
   index on before pushing the pair; the backend drops the same tail from
   the stored record, so a failed invoke must splice the saved rows back. */
async function sendText(text, sessionId = state.activeId, truncateFrom = null) {
  const msgs = state.cache[sessionId] || (state.cache[sessionId] = []);
  const removed = truncateFrom != null ? msgs.splice(truncateFrom) : null;
  msgs.push({ role: "User", text, reasoning: "", tools: [], error: null });
  msgs.push({ role: "Assistant", text: "", reasoning: "", tools: [], error: null });
  state.usage = null;
  state.toolCount = 0;
  state.error = null;
  renderAll();

  try {
    const streaming = await invoke("send_message", {
      sessionId,
      text,
      truncateFrom,
    });
    state.streams[sessionId] = streaming;
    renderStatus();
    renderSend();
    renderQueue();
    tick();
    return true;
  } catch (err) {
    msgs.pop();
    msgs.pop();
    if (removed) msgs.push(...removed); // restore the tail the edit dropped
    if (sessionId === state.activeId && !input.value.trim()) {
      input.value = text;
      autosize();
    }
    state.error = String(err);
    renderAll();
    return false;
  }
}

/* Send the next waiting follow-up for `sessionId` once its stream is idle. */
async function flushQueue(sessionId) {
  const q = state.queue[sessionId];
  if (!q || !q.length) return;
  if (state.streams[sessionId]) return; // a newer turn already took over
  const text = q.shift();
  if (!q.length) delete state.queue[sessionId];
  renderQueue();
  const ok = await sendText(text, sessionId);
  if (!ok) {
    // Rows are kept (typed content is never dropped): the next manual send
    // starts a turn whose end re-flushes them, and each row stays
    // individually removable. Surface the stall so it isn't silent.
    const waiting = (state.queue[sessionId] || []).length;
    if (waiting) {
      state.error = `Queued follow-up failed to send — ${waiting} message${waiting === 1 ? "" : "s"} still waiting. ${state.error || ""}`.trim();
      renderAll();
    }
  }
}

/* Re-run the last exchange. The backend peels the stored pair and re-saves,
   so the local cache mirrors the same pop/push and resyncs on failure. */
async function regenerate() {
  if (streamOf(state.activeId)) return; // only this chat's stream blocks a re-run
  const msgs = state.cache[state.activeId] || [];
  if (msgs.length < 2) return;
  const dropped = msgs.pop();
  const user = msgs.pop();
  if (!user || user.role !== "User" || !dropped || dropped.role !== "Assistant") {
    if (user) msgs.push(user);
    if (dropped) msgs.push(dropped);
    return;
  }
  msgs.push({ role: "User", text: user.text, reasoning: "", tools: [], error: null });
  msgs.push({ role: "Assistant", text: "", reasoning: "", tools: [], error: null });
  state.usage = null;
  state.toolCount = 0;
  state.error = null;
  renderAll();

  try {
    const streaming = await invoke("retry_last", { sessionId: state.activeId });
    state.streams[state.activeId] = streaming;
    renderStatus();
    renderSend();
    tick();
  } catch (err) {
    // The store is the source of truth; adopt whatever it now contains.
    try {
      state.cache[state.activeId] = await invoke("load_session", {
        id: state.activeId,
      });
    } catch (_e) {
      msgs.splice(msgs.length - 2, 2, user, dropped);
    }
    state.error = String(err);
    renderAll();
  }
}

function autosize() {
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 180) + "px";
}

/* ---------- slash-command palette ----------
   Typing "/" in the composer lists built-in commands plus every registered
   skill (skills fetch once, then cache). Commands also run directly from a
   bare "/name" first token in send(), so the palette is discoverability, not
   a gate. */

const BUILTIN_COMMANDS = [
  { name: "/new", hint: "Start a new chat", run: () => newChat() },
  { name: "/export", hint: "Export this chat as Markdown", run: exportActive },
  { name: "/restore", hint: "Put turns folded by /compact back", run: () => void restoreArchived() },
  { name: "/stats", hint: "Counts and outline for this chat", run: () => void openStatsView() },
  { name: "/files", hint: "Show the workspace files in the sidebar", run: () => void toggleFilePanel() },
  { name: "/run", hint: "Run a shell command, output streams into the run panel", run: () => void toggleRunPanel() },
  { name: "/export", hint: "Keep this chat as a ZIP file", run: () => void openExportView() },
  { name: "/settings", hint: "Open settings & capabilities", run: () => openSettings() },
  {
    name: "/compact",
    hint: "Summarize this chat's history with the model",
    run: compactActive,
  },
  {
    name: "/init",
    hint: "Generate AGENTS.md for the project (codex parity)",
    run: initProject,
  },
  {
    name: "/plan",
    hint: "Jump to the working plan",
    run: () => $("plan-panel").scrollIntoView({ behavior: "smooth", block: "nearest" }),
  },
];

async function exportActive() {
  if (!state.activeId) return;
  const path = await invoke("export_session", { id: state.activeId });
  toast(`Exported: ${path}`);
}

/* /compact (codex parity): ask the backend to summarize the stored history
   with a tool-less probe agent. The command returns immediately and the
   rewrite arrives via the `compact-finished` event, which reloads the
   transcript; this side only guards re-entry and reports failures. */
async function compactActive() {
  const id = state.activeId;
  if (!id) return;
  if (streamOf(id)) {
    toast("A turn is streaming — wait for it to finish before compacting.");
    return;
  }
  if (state.compacting[id]) {
    toast("Compaction already in flight for this chat.");
    return;
  }
  state.compacting[id] = true;
  renderStatus();
  try {
    await invoke("compact_session", { sessionId: id });
    toast("Compacting… the chat will reload when the summary lands.");
  } catch (err) {
    delete state.compacting[id];
    renderStatus();
    toast(`Compact failed: ${err}`);
  }
}

/* /init (codex parity): draft AGENTS.md from a tool-less probe of the
   project tree. Returns immediately; the outcome arrives via the
   `init-finished` event. An existing file is never overwritten. */
async function initProject() {
  if (state.initializing) {
    toast("AGENTS.md generation already running.");
    return;
  }
  state.initializing = true;
  renderStatus();
  try {
    await invoke("init_agents", { force: false });
    toast("Generating AGENTS.md…");
  } catch (err) {
    state.initializing = false;
    renderStatus();
    toast(`Init failed: ${err}`);
  }
}

function toast(msg) {
  const node = el("div", "toast", msg);
  document.body.appendChild(node);
  setTimeout(() => node.remove(), 4500);
}

async function paletteSkills() {
  if (state.skills.length) return state.skills;
  try {
    const caps = await invoke("list_capabilities");
    state.skills = (caps.skills || []).map((s) => ({
      name: s.name,
      description: s.description || "",
    }));
  } catch (_) {
    state.skills = [];
  }
  return state.skills;
}

function paletteEntries(query) {
  const q = query.toLowerCase();
  const cmds = BUILTIN_COMMANDS.filter((c) => c.name.slice(1).startsWith(q));
  const skills = state.skills
    .filter((s) => s.name.toLowerCase().startsWith(q))
    .map((s) => ({ kind: "skill", item: s }));
  return cmds.map((c) => ({ kind: "cmd", item: c })).concat(skills);
}

function syncPalette() {
  const v = input.value;
  // Open only for a leading command token; a space ends the token.
  if (v.startsWith("/") && !v.includes("\n") && !v.includes(" ")) {
    openPalette(v.slice(1));
  } else {
    closePalette();
  }
}

function openPalette(query) {
  const firstOpen = !state.palette;
  state.palette = { query, entries: paletteEntries(query), index: 0 };
  renderPalette();
  if (firstOpen) {
    // Populate skill entries once, then re-render if still open.
    paletteSkills().then(() => {
      if (state.palette) renderPalette();
    });
  }
}

function closePalette() {
  if (!state.palette) return;
  state.palette = null;
  paletteEl.classList.add("hidden");
  input.setAttribute("aria-expanded", "false");
  input.removeAttribute("aria-activedescendant");
}

function renderPalette() {
  const p = state.palette;
  if (!p) return;
  p.entries = paletteEntries(p.query);
  if (p.index >= p.entries.length) p.index = Math.max(0, p.entries.length - 1);
  paletteList.textContent = "";
  if (!p.entries.length) {
    paletteList.appendChild(el("li", "cmd-empty", "no matching command"));
    paletteEl.classList.remove("hidden");
    input.setAttribute("aria-expanded", "true");
    input.removeAttribute("aria-activedescendant");
    return;
  }
  p.entries.forEach((entry, i) => {
    const li = el("li", "cmd-item" + (i === p.index ? " sel" : ""));
    li.setAttribute("role", "option");
    li.id = `cmd-opt-${i}`;
    li.setAttribute("aria-selected", i === p.index ? "true" : "false");
    li.appendChild(el("span", "cmd-name", entry.item.name));
    const hint = entry.kind === "cmd"
      ? entry.item.hint
      : `skill · ${clip(entry.item.description, 64) || "insert prompt"}`;
    li.appendChild(el("span", "cmd-hint", hint));
    // mousedown fires before the textarea would blur — select reliably.
    li.addEventListener("mousedown", (e) => {
      e.preventDefault();
      selectPalette(i);
    });
  if (p.entries.length) {
    input.setAttribute("aria-expanded", "true");
    input.setAttribute("aria-activedescendant", `cmd-opt-${p.index}`);
  }
    paletteList.appendChild(li);
  });
  paletteEl.classList.remove("hidden");
}

async function selectPalette(i) {
  const p = state.palette;
  if (!p || !p.entries[i]) return;
  const entry = p.entries[i];
  closePalette();
  if (entry.kind === "cmd") {
    input.value = "";
    autosize();
    renderSend();
    try {
      await entry.item.run();
    } catch (err) {
      state.error = String(err);
      renderStatus();
    }
  } else {
    // Skills are prompts the model invokes: prefill a ready opening line.
    input.value = `Use the ${entry.item.name} skill: `;
    input.focus();
    autosize();
    renderSend();
  }
}

/* Run a bare "/name …" first token as a command; returns false for
   ordinary prompts. This is what makes commands work even when the
   palette was dismissed or filtered away. */
function runCommand(text) {
  const token = text.trim().split(/\s+/)[0];
  const cmd = BUILTIN_COMMANDS.find((c) => c.name === token);
  if (!cmd) return false;
  Promise.resolve(cmd.run()).catch((err) => {
    state.error = String(err);
    renderStatus();
  });
  return true;
}

/* ---------- @file mention picker ----------
   Typing "@" in the composer fuzzy-searches the workspace (Settings →
   workspace root) through the search_files command and inserts the picked
   path. Expansion happens send-side in Rust (mentions::expand), so the
   stored transcript keeps the raw "@path" text and the byte/file caps
   re-apply on every turn. */

const MENTION_RE = /(?:^|[\s([{])@([A-Za-z0-9_./~-]*)$/;
let mentionFetch = null; // debounce timer for search_files

/** Token before the caret that should trigger the picker, or null. */
function mentionToken() {
  const caret = input.selectionStart ?? input.value.length;
  const before = input.value.slice(0, caret);
  const m = MENTION_RE.exec(before);
  if (!m) return null;
  return { query: m[1], atStart: before.length - m[1].length - 1 };
}

function closeMention() {
  if (mentionFetch) {
    clearTimeout(mentionFetch);
    mentionFetch = null;
  }
  state.mention = null;
  mentionEl.classList.add("hidden");
}

/** Re-evaluate the token under the caret; open, refresh, or close. */
function syncMention() {
  const tok = mentionToken();
  if (!tok) {
    state.mentionDismissed = "";
    closeMention();
    return;
  }
  if (state.mentionDismissed === tok.query) return; // Esc dismissed it
  if (state.mention && state.mention.query === tok.query) return; // unchanged
  state.mention = {
    query: tok.query,
    atStart: tok.atStart,
    entries: [],
    index: 0,
    fresh: true,
  };
  renderMention();
  if (mentionFetch) clearTimeout(mentionFetch);
  const query = tok.query;
  mentionFetch = setTimeout(async () => {
    mentionFetch = null;
    let entries = [];
    try {
      entries = await invoke("search_files", { query, limit: 8 });
    } catch (_) {
      entries = [];
    }
    const m = state.mention;
    if (!m || m.query !== query) return; // superseded or closed
    m.entries = entries;
    m.index = 0;
    m.fresh = false;
    renderMention();
  }, 130);
}

function renderMention() {
  const m = state.mention;
  if (!m) return;
  mentionList.textContent = "";
  if (m.fresh) {
    mentionList.appendChild(el("li", "cmd-empty", "searching…"));
  } else if (!m.entries.length) {
    mentionList.appendChild(el("li", "cmd-empty", "no matching files"));
  } else {
    m.entries.forEach((path, i) => {
      const li = el("li", "cmd-item" + (i === m.index ? " sel" : ""));
      li.textContent = path;
      li.addEventListener("mousedown", (ev) => {
        ev.preventDefault();
        insertMention(i);
      });
      mentionList.appendChild(li);
    });
  }
  mentionEl.classList.remove("hidden");
}

/** Replace the "@query" span under the caret with "@path ". */
function insertMention(i) {
  const m = state.mention;
  if (!m || !m.entries.length) return;
  const path = m.entries[i];
  const caret = input.selectionStart ?? input.value.length;
  const v = input.value;
  input.value = `${v.slice(0, m.atStart)}@${path} ${v.slice(caret)}`;
  const pos = m.atStart + path.length + 2;
  input.setSelectionRange(pos, pos);
  closeMention();
  input.focus();
  autosize();
  renderSend();
  syncPalette();
  // Mentioning a file is also a request to read it: show it in the right panel.
  openFileInPanel(path);
}

/* Show a workspace file in the right-hand document panel.

   The Rust command owns the policy (workspace confinement, bounded read,
   binary and encoding refusal); failures surface inside the panel rather than
   as a toast, so the reader sees exactly what happened. */
async function openFileInPanel(relPath) {
  const id = "file:" + relPath;
  const title = relPath.split("/").pop() || relPath;
  try {
    const view = await invoke("read_file", { path: relPath });
    const notice = view.truncated
      ? ` — showing the first ${view.text.length} of ${view.bytes} bytes`
      : "";
    Shell.openDocument({
      id,
      title,
      path: (view.path || relPath) + notice,
      text: view.text,
    });
  } catch (err) {
    Shell.openDocument({ id, title, path: relPath, error: String(err) });
  }
}

/* ---------- settings modal ---------- */

/* Render the agent's registered tools, skills, and plugins as DOM text
   (never innerHTML — descriptions come from tool/plugin definitions). */
/* Draw one capabilities snapshot into `host`. This is pure DOM work with the
   snapshot passed in, so the harness can render a fixture without a backend —
   which is the only way to check what the panel shows. */
function renderCapabilities(host, caps) {
  host.textContent = "";
  const plugins = (caps.plugins || []).map((p) => ({
    name: p,
    description: "",
    category: "plugin",
  }));
  const asyncTools = (caps.async_tools || []).filter((t) => t.category !== "mcp");
  const mcpTools = (caps.async_tools || []).filter((t) => t.category === "mcp");
  const groups = [
    ["Tools", caps.tools || []],
    ["Async tools", asyncTools],
    ["MCP tools", mcpTools],
    ["Skills", caps.skills || []],
    ["Plugins", plugins],
  ];
  let any = false;
  for (const [title, items] of groups) {
    if (!items.length) continue;
    any = true;
    const head = document.createElement("div");
    head.className = "cap-group";
    head.textContent = title;
    host.appendChild(head);
    for (const item of items) {
      const row = document.createElement("div");
      row.className = "cap-item";
      const name = document.createElement("span");
      name.className = "cap-name";
      name.textContent = item.name;
      row.appendChild(name);
      if (item.description) {
        const desc = document.createElement("span");
        desc.className = "cap-desc";
        desc.textContent = item.description;
        row.appendChild(desc);
      }
      host.appendChild(row);
    }
  }
  if (!any) {
    const empty = document.createElement("div");
    empty.className = "cap-empty";
    empty.textContent = "No capabilities registered yet.";
    host.appendChild(empty);
  }
  // The hook surface is not a registered capability: it is what a Rust-side
  // extension can attach to. It comes from the agent's own enumeration and is
  // shown whether or not anything is registered, because "what can I build
  // against?" has an answer even in an empty agent.
  const hooks = caps.hook_events || [];
  if (hooks.length) {
    const head = document.createElement("div");
    head.className = "cap-group";
    head.textContent = "Hook events (Rust extensions)";
    host.appendChild(head);
    const row = document.createElement("div");
    row.className = "cap-item";
    const name = document.createElement("span");
    name.className = "cap-name cap-hooks";
    name.textContent = hooks.join(" · ");
    row.appendChild(name);
    host.appendChild(row);
  }
}

/* Load and draw the capabilities for the active workspace. Descriptions come
   from tool and plugin definitions, so this is DOM text, never innerHTML. */
async function loadCapabilities() {
  capList.textContent = "loading…";
  try {
    renderCapabilities(capList, await invoke("list_capabilities"));
  } catch (err) {
    capList.textContent = String(err);
  }
}

/* Counts, abbreviated, because the status bar is a glance and not a ledger. */
function fmtCount(n) {
  const v = Number(n) || 0;
  if (v < 1000) return String(v);
  if (v < 1000000) return `${(v / 1000).toFixed(1)}k`;
  return `${(v / 1000000).toFixed(1)}m`;
}

/* What the active chat amounts to, in the status bar. Pure, so the harness can
   render a fixture and read it; renders nothing until an overview arrives, so
   switching chats never shows the previous chat's numbers. */
function renderSessionStats(host, overview) {
  if (!overview) return;
  host.appendChild(
    el(
      "span",
      "stats",
      `${overview.messages} msgs · ${overview.tool_calls} tools · ${fmtCount(
        overview.characters,
      )} chars`,
    ),
  );
}

/* The full breakdown, for the document panel. Pure text: these are counts and
   the user's own words, and neither belongs in an HTML sink. */
function overviewLines(o) {
  if (!o) return "No overview is available for this chat.";
  const day = (sec) => new Date((Number(sec) || 0) * 1000).toISOString().slice(0, 10);
  const lines = [
    `${o.messages} messages (${o.archived} folded) · ${o.asks} asks · ${o.replies} replies`,
    `${o.tool_calls} tool calls · ${o.errors} turns ended in an error`,
    `${o.characters} characters · ${o.reasoning_characters} characters of reasoning`,
    `created ${day(o.created_at)} · updated ${day(o.updated_at)}`,
    "",
    `Outline (${o.outline.length} asks)`,
  ];
  for (const entry of o.outline) {
    lines.push(` ${entry.n}. ${entry.preview} (${entry.characters} chars)`);
  }
  if (!o.outline.length) lines.push(" (this chat has no asks yet)");
  return lines.join("\n");
}

async function loadOverview(id) {
  if (!id) {
    state.overview = null;
    return;
  }
  if (state.overviewId !== id) {
    state.overviewId = id;
    state.overview = null;
    renderStatus();
  }
  try {
    const o = await invoke("session_overview", { sessionId: id });
    if (id !== state.activeId) return;
    state.overview = o;
    renderStatus();
  } catch (err) {
    // A missing overview is a convenience lost, not a broken shell.
  }
}

/* Keep this chat as a file. The archive is written by the core, so this only
   reports where it landed: an export is a file the user owns. */
async function openExportView() {
  const id = state.activeId;
  if (!id) return;
  try {
    const path = await invoke("export_chat", { sessionId: id });
    Shell.openDocument({
      id: "export:" + id,
      title: "Exported",
      path,
      text:
        `Wrote ${path}\n\n` +
        "It holds README.txt, chat.json and chat.md. The entries are stored, " +
        "not compressed, so any ZIP tool can read the archive.\n",
    });
  } catch (err) {
    Shell.openDocument({
      id: "export:" + id,
      title: "Export failed",
      path: "unavailable",
      error: String(err),
    });
  }
}

/* The counts and outline in the document panel: orientation for a long chat.
   Read-only — the transcript already offers everything that changes state. */
async function openStatsView() {
  const id = state.activeId;
  if (!id) return;
  try {
    const o = await invoke("session_overview", { sessionId: id });
    Shell.openDocument({
      id: "stats:" + id,
      title: "This chat",
      path: `${o.messages} messages · ${o.asks} asks`,
      text: overviewLines(o),
    });
  } catch (err) {
    Shell.openDocument({ id: "stats:" + id, title: "This chat", path: "unavailable", error: String(err) });
  }
}

/* Chats that were deleted, each with a way back.
   Deleting a chat moves it here instead of unlinking it, so a mis-click is
   recoverable; emptying the trash is the only permanent removal. The list shows
   titles, because a bare UUID is not something anyone can recognise. */
function renderTrash(host, chats, onRestore) {
  host.textContent = "";
  const list = Array.isArray(chats) ? chats : [];
  if (list.length === 0) {
    host.appendChild(el("div", "cap-empty", "Nothing in the trash."));
    return;
  }
  for (const chat of list) {
    const row = el("div", "trash-row");
    row.appendChild(el("span", "trash-title", chat.title || "untitled"));
    const back = el("button", "trash-restore", "↺ restore");
    back.title = "Bring this chat back";
    back.addEventListener("click", () => onRestore(chat.id));
    row.appendChild(back);
    host.appendChild(row);
  }
}

async function loadTrash() {
  try {
    const chats = await invoke("trashed_chats");
    state.trashed = chats;
    renderTrash(trashList, chats, restoreTrashed);
  } catch (err) {
    trashList.textContent = String(err);
  }
}

async function restoreTrashed(id) {
  try {
    const title = await invoke("restore_session", { id });
    toast(`Restored “${title}”`);
    await loadTrash();
    await refreshSessions();
  } catch (err) {
    toast(`Restore failed: ${err}`);
  }
}

async function purgeTrash() {
  const count = (state.trashed || []).length;
  if (!count) {
    toast("The trash is already empty.");
    return;
  }
  const many = count === 1 ? "chat" : "chats";
  if (!window.confirm(`Permanently delete ${count} ${many}? This cannot be undone.`)) return;
  try {
    const purged = await invoke("purge_trash");
    toast(`Permanently deleted ${purged} ${purged === 1 ? "chat" : "chats"}`);
    await loadTrash();
  } catch (err) {
    toast(`Purge failed: ${err}`);
  }
}

async function openSettings() {
  state.settingsOpener = document.activeElement;
  loadMemories();
  void loadTrash();
  try {
    const s = await invoke("get_settings");
    state.model = s.model;
    setModel.value = s.model;
    setBaseUrl.value = s.base_url;
    setApiKey.value = s.api_key;
    setSystem.value = s.system_prompt;
    setInstructions.checked = s.project_instructions !== false;
    setTemp.value = String(s.temperature);
    setEffort.value = s.reasoning_effort || "";
    setBash.checked = !!s.bash_enabled;
    setFiles.checked = !!s.file_tools_enabled;
    setApproval.checked = s.require_approval !== false;
    // The permanent allowances are shown next to the gate they relax, so the
    // reason a tool stopped asking is visible where the gate is configured.
    state.allowedTools = s.allowed_tools || [];
    if (allowedList) renderAllowedTools(allowedList, state.allowedTools);
    // The profile names are not part of this payload; ask for them separately.
    void loadProfiles();
    setWorkspace.value = s.bash_workspace || "";
    // The file sidebar lists the same root, and keys its per-workspace
    // memory by it.
    state.workspaceRoot = s.bash_workspace || "";
    // Keep the document panel's Start page in step with saved settings.
    Shell.setContext({ workspace: s.bash_workspace || "", model: s.model || "" });
    setSkillsDir.value = s.skills_dir || "";
    setMemory.checked = s.memory_enabled !== false;
    setMemoryPath.value = s.memory_path || "";
    state.budget = s.context_budget ?? 32768;
    setContextBudget.value = String(state.budget);
    state.mcpServers = Array.isArray(s.mcp_servers) ? s.mcp_servers : [];
    mcpMsg.textContent = "";
    renderMcp();
    refreshMcpStatus();
    state.providers = Array.isArray(s.providers) ? s.providers : [];
    provMsg.textContent = "";
    renderProviders();
  } catch (err) {
    state.error = String(err);
    renderStatus();
  }
  modal.classList.remove("hidden");
  setModel.focus();
  loadCapabilities();
}

function closeSettings() {
  modal.classList.add("hidden");
  // Hand focus back to whatever opened the dialog: leaving it on a hidden
  // control strands keyboard users at the top of the document.
  const back = state.settingsOpener;
  state.settingsOpener = null;
  if (back && typeof back.focus === "function" && document.contains(back)) back.focus();
}

settingsForm.addEventListener("submit", async (ev) => {
  ev.preventDefault();
  try {
    const temp = parseFloat(setTemp.value);
    const s = await invoke("save_settings", {
      model: setModel.value,
      baseUrl: setBaseUrl.value,
      apiKey: setApiKey.value,
      systemPrompt: setSystem.value,
      projectInstructions: setInstructions.checked,
      temperature: Number.isNaN(temp) ? 0.7 : temp,
      reasoningEffort: setEffort.value || null,
      bashEnabled: setBash.checked,
      fileToolsEnabled: setFiles.checked,
      bashWorkspace: setWorkspace.value,
      skillsDir: setSkillsDir.value,
      memoryEnabled: setMemory.checked,
      memoryPath: setMemoryPath.value,
      requireApproval: setApproval.checked,
      contextBudget: Math.max(0, Math.floor(Number(setContextBudget.value) || 0)),
      mcpServers: state.mcpServers,
      providers: state.providers,
    });
    state.model = s.model;
    state.budget = s.context_budget ?? 32768;
    // A new workspace root makes every cached listing about the wrong
    // directory, so the tree is dropped and refetched from the new root.
    state.workspaceRoot = setWorkspace.value || "";
    fileTree.cache = {};
    if (fileTree.open) void restoreFileTree();
    // The run output is stored per root, so the new root's history takes over.
    restoreRunLines();
    renderRunPanel();
    state.mcpServers = Array.isArray(s.mcp_servers) ? s.mcp_servers : [];
    state.providers = Array.isArray(s.providers) ? s.providers : [];
    state.error = null;
    closeSettings();
    renderStatus();
    // The backend connects servers in the background: follow the progress.
    pollMcpStatus();
  } catch (err) {
    state.error = String(err);
    renderStatus();
  }
});

/* ---------- MCP servers (plugins) ---------- */

/* Split "npx -y pkg" into command + args, honoring double/single quotes. */
function parseCommand(text) {
  const parts = [];
  let cur = "";
  let quote = null;
  for (const ch of text.trim()) {
    if (quote) {
      if (ch === quote) quote = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (/\s/.test(ch)) {
      if (cur) {
        parts.push(cur);
        cur = "";
      }
    } else {
      cur += ch;
    }
  }
  if (quote) return null;
  if (cur) parts.push(cur);
  return parts;
}

function mcpTarget(entry) {
  if (entry.transport === "http") return entry.url || "(no url)";
  return [entry.command, ...(entry.args || [])].join(" ");
}

function mcpStatusFor(name) {
  const rep = state.mcpStatus;
  if (!rep || !Array.isArray(rep.servers)) return null;
  return rep.servers.find((s) => s.name === name) || null;
}

function renderMcp() {
  mcpList.textContent = "";
  if (!state.mcpServers.length) {
    mcpList.appendChild(
      el("li", "mcp-empty", "No MCP servers yet — add one below to plug in external tools.")
    );
    return;
  }
  state.mcpServers.forEach((entry, idx) => {
    const row = el("li", "mcp-row");
    const check = document.createElement("input");
    check.type = "checkbox";
    check.checked = entry.enabled !== false;
    check.id = `mcp-enabled-${idx}`;
    check.setAttribute("aria-label", `Enable ${entry.name}`);
    check.addEventListener("change", () => {
      entry.enabled = check.checked;
      renderMcp();
    });
    row.appendChild(check);

    const body = el("div", "mcp-body");
    const head = el("div", "mcp-head");
    head.appendChild(el("span", "mcp-name", entry.name));
    head.appendChild(el("span", "mcp-tag", entry.transport));
    const st = mcpStatusFor(entry.name);
    if (st && st.connected) {
      head.appendChild(el("span", "mcp-badge ok", `${st.tools} tool${st.tools === 1 ? "" : "s"}`));
    } else if (st && st.error) {
      head.appendChild(el("span", "mcp-badge bad", "failed"));
    } else if (st && state.mcpStatus && state.mcpStatus.connecting) {
      head.appendChild(el("span", "mcp-badge wait", "connecting…"));
    } else {
      head.appendChild(el("span", "mcp-badge off", entry.enabled === false ? "off" : "pending"));
    }
    body.appendChild(head);
    body.appendChild(el("div", "mcp-target", mcpTarget(entry)));
    if (st && st.error) body.appendChild(el("div", "mcp-error", st.error));
    row.appendChild(body);

    const del = el("button", "btn btn-ghost mcp-del", "Remove");
    del.type = "button";
    del.setAttribute("aria-label", `Remove ${entry.name}`);
    del.addEventListener("click", () => {
      state.mcpServers.splice(idx, 1);
      renderMcp();
    });
    row.appendChild(del);
    mcpList.appendChild(row);
  });
  updateMcpButton();
}

/* The reconnect button must never promise a pass that cannot run: it is off
   while a connect/retry pass is in flight and when nothing is enabled. */
function updateMcpButton() {
  const btn = $("mcp-reconnect-btn");
  const anyEnabled = state.mcpServers.some((s) => s.enabled !== false);
  const connecting = Boolean(state.mcpStatus && state.mcpStatus.connecting);
  btn.disabled = connecting || !anyEnabled;
  btn.title = !anyEnabled
    ? "No enabled servers — enable one below first"
    : connecting
      ? "A connect pass is running…"
      : "Reconnect every enabled server";
}

function mcpStatusLabel(rep) {
  if (!rep.servers || !rep.servers.length) return "";
  if (rep.connecting) return "connecting…";
  const enabled = rep.servers.filter((s) => s.enabled);
  const failed = enabled.filter((s) => !s.connected);
  if (failed.length) return `${enabled.length - failed.length}/${enabled.length} connected`;
  return `${enabled.length} connected`;
}

async function refreshMcpStatus() {
  try {
    const rep = await invoke("mcp_status");
    state.mcpStatus = rep;
    mcpConn.textContent = mcpStatusLabel(rep);
    renderMcp();
    renderStatus();
  } catch (_err) {
    mcpConn.textContent = "";
  }
}

/* Follow a connect pass: poll until the backend stops reporting "connecting".
   One loop at a time — a second reconnect click must not stack pollers. */
async function pollMcpStatus() {
  if (state.mcpPolling) return;
  state.mcpPolling = true;
  try {
    for (let i = 0; i < 40; i += 1) {
      let rep;
      try {
        rep = await invoke("mcp_status");
      } catch (_err) {
        return;
      }
      state.mcpStatus = rep;
      mcpConn.textContent = mcpStatusLabel(rep);
      if (!modal.classList.contains("hidden")) renderMcp();
      renderStatus();
      if (!rep.connecting) return;
      await new Promise((resolve) => setTimeout(resolve, 1500));
    }
  } finally {
    state.mcpPolling = false;
  }
}

function addMcpServer() {
  const name = mcpName.value.trim();
  const transport = mcpTransport.value === "http" ? "http" : "stdio";
  if (!name) {
    mcpMsg.textContent = "Give the server a name.";
    mcpName.focus();
    return;
  }
  if (state.mcpServers.some((e) => e.name.trim() === name)) {
    mcpMsg.textContent = `A server named “${name}” already exists.`;
    mcpName.focus();
    return;
  }
  const entry = { name, transport, command: "", args: [], url: "", enabled: true };
  if (transport === "http") {
    entry.url = mcpUrl.value.trim();
    if (!entry.url) {
      mcpMsg.textContent = "http transport needs a URL.";
      mcpUrl.focus();
      return;
    }
  } else {
    const parts = parseCommand(mcpCmd.value);
    if (!parts || !parts.length) {
      mcpMsg.textContent =
        parts === null ? "Unclosed quote in the command." : "stdio transport needs a command.";
      mcpCmd.focus();
      return;
    }
    entry.command = parts[0];
    entry.args = parts.slice(1);
  }
  state.mcpServers.push(entry);
  renderMcp();
  mcpName.value = "";
  mcpCmd.value = "";
  mcpUrl.value = "";
  mcpMsg.textContent = `Added “${name}” — press Save to connect.`;
  mcpName.focus();
}

mcpAddBtn.addEventListener("click", addMcpServer);

/* ---------- Fallback providers (failover chain) ---------- */

function renderProviders() {
  provList.textContent = "";
  if (!state.providers.length) {
    provList.appendChild(
      el("li", "mcp-empty", "No fallback providers — add one below to survive a dead primary endpoint.")
    );
    return;
  }
  state.providers.forEach((entry, idx) => {
    const row = el("li", "mcp-row");
    const check = document.createElement("input");
    check.type = "checkbox";
    check.checked = entry.enabled !== false;
    check.id = `prov-enabled-${idx}`;
    check.setAttribute("aria-label", `Enable ${entry.name}`);
    check.addEventListener("change", () => {
      entry.enabled = check.checked;
      renderProviders();
    });
    row.appendChild(check);

    const body = el("div", "mcp-body");
    const head = el("div", "mcp-head");
    head.appendChild(el("span", "mcp-name", entry.name));
    head.appendChild(
      el("span", "mcp-tag", entry.enabled === false ? "off" : `try #${idx + 1}`)
    );
    body.appendChild(head);
    body.appendChild(el("div", "mcp-target", `${entry.model} · ${entry.base_url}`));
    row.appendChild(body);

    const del = el("button", "btn btn-ghost mcp-del", "Remove");
    del.type = "button";
    del.setAttribute("aria-label", `Remove ${entry.name}`);
    del.addEventListener("click", () => {
      state.providers.splice(idx, 1);
      renderProviders();
    });
    row.appendChild(del);
    provList.appendChild(row);
  });
}

function addProvider() {
  const name = provName.value.trim();
  const model = provModel.value.trim();
  const baseUrl = provUrl.value.trim();
  if (!name || !model || !baseUrl) {
    provMsg.textContent = "Name, model, and base URL are all required.";
    (!name ? provName : !model ? provModel : provUrl).focus();
    return;
  }
  if (state.providers.some((e) => e.name.trim() === name)) {
    provMsg.textContent = `A provider named “${name}” already exists.`;
    provName.focus();
    return;
  }
  state.providers.push({
    name,
    model,
    base_url: baseUrl,
    api_key: provKey.value.trim(),
    enabled: true,
  });
  renderProviders();
  provName.value = "";
  provModel.value = "";
  provUrl.value = "";
  provKey.value = "";
  provMsg.textContent = `Added “${name}” — press Save to apply.`;
  provName.focus();
}

provAddBtn.addEventListener("click", addProvider);

$("mcp-reconnect-btn").addEventListener("click", async () => {
  const btn = $("mcp-reconnect-btn");
  if (btn.disabled) return;
  btn.disabled = true; // re-enabled once the follow-up poll ends
  mcpMsg.textContent = "Reconnecting…";
  try {
    await invoke("reconnect_mcp");
    await pollMcpStatus();
    const rep = state.mcpStatus;
    if (rep) {
      const failed = rep.servers.filter((s) => s.enabled && !s.connected).length;
      mcpMsg.textContent = failed
        ? `${failed} server${failed === 1 ? "" : "s"} still failing after automatic retries — check the command or URL below.`
        : `Connection updated: ${mcpStatusLabel(rep)}`;
    } else {
      mcpMsg.textContent = "";
    }
  } catch (err) {
    mcpMsg.textContent = String(err);
  } finally {
    updateMcpButton();
  }
});
mcpTransport.addEventListener("change", () => {
  const http = mcpTransport.value === "http";
  mcpCmd.disabled = http;
  mcpUrl.disabled = !http;
});
mcpCmd.disabled = false;
mcpUrl.disabled = true;
// Enter inside the add-row adds instead of submitting the settings form.
for (const field of [mcpName, mcpCmd, mcpUrl]) {
  field.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      addMcpServer();
    }
  });
}

$("settings-cancel").addEventListener("click", closeSettings);
modal.addEventListener("click", (e) => {
  if (e.target === modal) closeSettings();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !modal.classList.contains("hidden")) closeSettings();
});

/* A labelled checkbox for the workspace form. */
function workspaceCheck(form, cls, label, checked) {
  const box = document.createElement("label");
  box.className = "row-check";
  const input = document.createElement("input");
  input.type = "checkbox";
  input.className = cls;
  input.checked = checked;
  box.appendChild(input);
  box.appendChild(document.createTextNode(" " + label));
  return box;
}

/* A flag the form can state: an absent control restates what the workspace had,
   rather than reading the absence as "off". */
function workspaceFlag(form, known, cls, field) {
  const box = form.querySelector("." + cls);
  return box ? box.checked : known[field] !== false;
}

/* The budget is a count, so an absent or unusable input restates the old value
   instead of sending zero. */
function workspaceBudget(form, known) {
  const input = form.querySelector(".ws-budget");
  const value = input ? Number(input.value) : Number.NaN;
  return Number.isFinite(value) && value > 0 ? Math.floor(value) : known.context_budget;
}

/* The discovered config profiles. A profile is selected by putting its name in
   the model field, so a chip is a one-click way to type it rather than a second
   place where a model is configured. */
/* How many turns a compaction folded away, with a way to put them back.
   Zero renders nothing: a control that cannot do anything is worse than none.
   The count comes from the backend, which keeps the folded turns in the
   session file, so the affordance survives a reload. */
function renderArchived(host, count, view, restore) {
  host.textContent = "";
  if (typeof count !== "number" || count <= 0) return;
  const chip = el("button", "archived-chip", `⧉ ${count} folded`);
  chip.title = "Read the turns /compact folded into the summary";
  chip.addEventListener("click", () => view());
  const back = el("button", "archived-chip archived-restore", "↺ restore");
  back.title = "Put these turns back into this chat, in front of the summary";
  back.addEventListener("click", () => restore());
  host.appendChild(chip);
  host.appendChild(back);
}

/* The folded turns as plain text for the document panel. Pure, so the harness
   can check what a reader would see; each turn is clipped because a folded
   history can be long and the panel is for recognition, not reading in full. */
function archivedPreviewText(messages) {
  const turns = Array.isArray(messages) ? messages : [];
  if (turns.length === 0) return "Nothing is folded in this chat.";
  const body = turns.map((m, i) => {
    const text = String(m.text || "").trim();
    const clipped =
      text.length > 1200 ? text.slice(0, 1200) + `\n… (${text.length - 1200} more characters)` : text;
    return `--- ${i + 1}. ${m.role || "unknown"} ---\n${clipped || "(empty)"}`;
  });
  return (
    `These ${turns.length} turns were folded into the summary by /compact. ` +
    `The summary is in the chat; these are the originals.\n\n` +
    body.join("\n\n")
  );
}

/* Show the folded turns in the document panel. Read-only on purpose: seeing
   what was folded and putting it back are separate decisions. */
async function openArchivedView() {
  const id = state.activeId;
  if (!id) return;
  try {
    const turns = await invoke("archived_messages", { sessionId: id });
    Shell.openDocument({
      id: "archived:" + id,
      title: "Folded turns",
      path: `${turns.length} turn${turns.length === 1 ? "" : "s"} folded by /compact`,
      text: archivedPreviewText(turns),
    });
  } catch (err) {
    Shell.openDocument({
      id: "archived:" + id,
      title: "Folded turns",
      path: "unavailable",
      error: String(err),
    });
  }
}

/* Ask the backend what this chat has folded, and offer it back if anything. */
async function loadArchived(sessionId) {
  if (!sessionId) return;
  let count = 0;
  try {
    count = await invoke("compacted_archive", { sessionId });
  } catch (err) {
    return; // a chat with no archive is the ordinary case
  }
  if (sessionId !== state.activeId) return; // the user switched away meanwhile
  state.archived = count;
  renderStatus();
}

/* Put the folded turns back and show them. */
async function restoreArchived() {
  const id = state.activeId;
  if (!id) return;
  try {
    const restored = await invoke("restore_compacted", { sessionId: id });
    toast(`Restored ${restored} folded message${restored === 1 ? "" : "s"}`);
    state.archived = 0;
    await loadActive();
  } catch (err) {
    toast(`Restore failed: ${err}`);
  }
}

function renderProfiles(host, profiles, pick) {
  host.textContent = "";
  if (!profiles || !profiles.length) {
    host.appendChild(el("div", "cap-empty", "No [llm.<name>] profiles in the config file."));
    return;
  }
  for (const profile of profiles) {
    const chip = el("button", "profile-chip", profile.name);
    chip.type = "button";
    chip.title = `${profile.model} · ${profile.base_url}${profile.has_key ? "" : " · no key"}`;
    chip.addEventListener("click", () => pick(profile));
    host.appendChild(chip);
  }
}

/* The names live in the config file, which is why this asks the backend for
   them: the settings payload never serializes the profile list. */
async function loadProfiles() {
  const list = await invoke("list_profiles").catch(() => null);
  state.profiles = Array.isArray(list) ? list : [];
  if (profilesHost) {
    renderProfiles(profilesHost, state.profiles, (profile) => {
      setModel.value = profile.name;
      toast(`Model set to profile "${profile.name}"`);
    });
  }
}

/* Draw the permanent allowances. Pure DOM work, so the harness can drive it
   with a fixture rather than through a backend. */
function renderAllowedTools(host, names) {
  host.textContent = "";
  if (!names || !names.length) {
    const empty = document.createElement("div");
    empty.className = "cap-empty";
    empty.textContent = "Approvals are asked every time.";
    host.appendChild(empty);
    return;
  }
  for (const name of names) {
    const row = document.createElement("div");
    row.className = "cap-item";
    const label = document.createElement("span");
    label.className = "cap-name";
    label.textContent = name;
    row.appendChild(label);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "mini-btn";
    remove.textContent = "Remove";
    remove.addEventListener("click", () => forgetAllowedTool(name));
    row.appendChild(remove);
    host.appendChild(row);
  }
}

/* Asking to forget an allowance is server work: the broker has to let go of it
   too, or it would keep waving the tool through until the next restart. */
async function forgetAllowedTool(name) {
  const left = await invoke("forget_allowed_tool", { tool: name }).catch(() => null);
  if (Array.isArray(left)) state.allowedTools = left;
  if (allowedList) renderAllowedTools(allowedList, state.allowedTools);
}

/* ---------- approval gate ---------- */

let currentApproval = null; // request shown in the modal, if any

function showNextApproval() {
  if (currentApproval) return;
  currentApproval = state.approvals.shift() || null;
  if (!currentApproval) {
    approvalModal.classList.add("hidden");
    return;
  }
  approvalTool.textContent = currentApproval.tool;
  renderApprovalDiff(currentApproval.diff);
  approvalModal.classList.remove("hidden");
  // Move focus to the explicit choice. Escape deliberately does not dismiss
  // this dialog: an approval has to be answered, not skipped past.
  const allow = $("approval-allow");
  if (allow) allow.focus();
}

// Render a write preview as colored diff lines (one textContent per line, so
// file content stays inert and never parses as HTML). Without a diff — other
// tools, unchanged/binary/oversized files — fall back to the plain args JSON.
function renderApprovalDiff(diff) {
  approvalDiff.textContent = "";
  if (!diff) {
    approvalArgs.textContent = currentApproval.args || "{}";
    approvalArgs.classList.remove("hidden");
    approvalDiff.classList.add("hidden");
    return;
  }
  const lines = diff.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  for (const line of lines) {
    const el = document.createElement("div");
    el.textContent = line;
    el.className = line.startsWith("-")
      ? "d-del"
      : line.startsWith("+")
        ? "d-add"
        : "d-ctx";
    approvalDiff.appendChild(el);
  }
  approvalArgs.classList.add("hidden");
  approvalDiff.classList.remove("hidden");
}

function clearApprovals(sessionId) {
  if (!sessionId) {
    // Global wipe (shutdown path): drop everything.
    state.approvals = [];
    currentApproval = null;
    approvalModal.classList.add("hidden");
    return;
  }
  // Scoped: drop only this chat's requests — other chats' pending approvals
  // must survive this stream ending.
  state.approvals = state.approvals.filter((a) => a.session_id !== sessionId);
  if (currentApproval && currentApproval.session_id === sessionId) {
    currentApproval = null;
  }
  if (currentApproval) return; // another chat's request is on screen
  approvalModal.classList.add("hidden");
  if (state.approvals.length) showNextApproval();
}

async function answerApproval(approved) {
  const req = currentApproval;
  currentApproval = null;
  if (req) {
    await invoke("respond_approval", { id: req.id, approved }).catch(() => {});
  }
  showNextApproval();
}async function answerApproval(approved) {
  const req = currentApproval;
  currentApproval = null;
  if (req) {
    await invoke("respond_approval", { id: req.id, approved }).catch(() => {});
  }
  showNextApproval();
}

/* An allowance is recorded before the pending request is answered, so the tool
   cannot run again in the gap, and the user sees the answer take effect now. */
async function allowApproval(scope) {
  const req = currentApproval;
  if (!req) return;
  const persistent = await invoke("allow_tool", { tool: req.tool, scope }).catch(() => null);
  if (Array.isArray(persistent)) state.allowedTools = persistent;
  await answerApproval(true);
}

listen("approval-request", (event) => {
  state.approvals.push(event.payload);
  showNextApproval();
});

$("approval-deny").addEventListener("click", () => answerApproval(false));
$("approval-session").addEventListener("click", () => allowApproval("session"));
$("approval-always").addEventListener("click", () => allowApproval("always"));
$("approval-allow").addEventListener("click", () => answerApproval(true));

/* ---------- streaming events ---------- */

/* Coalesce streaming repaints to one per animation frame. A fast stream
   delivers dozens of token events per frame; rebuilding every bubble for
   each one is what makes web UIs feel less than native, so the DOM only
   changes on frame boundaries. */
let repaintQueued = false;
function scheduleRepaint() {
  if (repaintQueued) return;
  repaintQueued = true;
  requestAnimationFrame(() => {
    repaintQueued = false;
    renderMessages();
    renderStatus();
  });
}

listen("agent-event", (event) => {
  const p = event.payload;
  const msgs = ensureSessionCache(p.session_id);
  if (!msgs) return;
  const last = msgs[msgs.length - 1];
  // Every event below lands on the tail row (text, reasoning, tool, tool_result),
  // so the cursor only has to reach it.
  markDirty(msgs.length - 1);
  switch (p.kind) {
    case "text":
      if (last) last.text += p.text;
      break;
    case "reasoning":
      if (last) last.reasoning += p.text;
      break;
    case "tool":
      if (last) last.tools.push({ name: p.name, args: p.args, output: null, ms: null });
      // Counters/status belong to the chat being watched; background
      // streams still write into their own session cache above.
      if (p.session_id === state.activeId) {
        state.toolCount += 1;
        // Tool calls include `update_plan`; re-pull the live plan each time.
        refreshPlan();
      }
      break;
    case "tool_result":
      // The ReAct loop finished a call: attach the result to the matching
      // entry (same name, no result yet — mirrors the Rust-side matching).
      if (last) {
        const tools = last.tools || [];
        for (let i = tools.length - 1; i >= 0; i--) {
          if (tools[i].name === p.name && tools[i].output == null) {
            tools[i].output = p.output;
            tools[i].ms = p.ms;
            break;
          }
        }
      }
      break;
    case "usage":
      if (p.session_id === state.activeId) {
        state.usage = { prompt: p.prompt, completion: p.completion };
      }
      break;
    case "compacted":
      // Sent ahead of the stream when the send-side budget trimmed context.
      toast(
        `Compacted context: ~${p.before} → ~${p.after} tokens ` +
          `(${p.dropped} message${p.dropped === 1 ? "" : "s"} summarized)`
      );
      void loadArchived(state.activeId);
  void loadOverview(state.activeId);
      break;
    case "error":
      if (last) last.error = p.message;
      if (p.session_id === state.activeId) state.error = p.message;
      break;
    default:
      break;
  }
  if (
    p.kind === "usage" ||
    p.kind === "compacted" ||
    p.session_id !== state.activeId
  ) {
    renderStatus();
  } else {
    scheduleRepaint();
  }
});

listen("stream-finished", async (event) => {
  const p = event.payload;
  // The provider's count for the turn that just ended; absent when it reported none.
  if (p.session_id === state.activeId) {
    state.usage = { prompt: p.prompt_tokens || 0, completion: p.completion_tokens || 0 };
    renderStatus();
  }
  // The server denied that chat's pending approvals when its stream ended.
  clearApprovals(p.session_id);
  try {
    const fresh = await invoke("load_session", { id: p.session_id });
    state.cache[p.session_id] = fresh;
  } catch (_) {
    delete state.cache[p.session_id];
  }
  // Gen guard: a superseded (re-sent) turn finishing must not clear the
  // newer stream's entry.
  const s = state.streams[p.session_id];
  if (s && s.gen === p.gen) delete state.streams[p.session_id];
  // Terminal task event: `gen` orders it, so a stale finish for a superseded
  // turn cannot regress a newer task's state (Part II §12.3).
  Shell.applyTaskEvent({
    taskId: p.session_id,
    type: p.cancelled ? "cancelled" : p.error ? "failed" : "completed",
    seq: p.gen,
    timestamp: Date.now(),
  });
  await refreshSessions();
  renderAll();
  // Turn over: hand the baton to whatever this chat queued meanwhile.
  flushQueue(p.session_id).catch(() => {});
});

/* The backend finished (or abandoned) a /compact attempt: drop the
   in-flight flag, then reload the rewritten transcript so the summary
   replaces the history on screen. Failures leave the history untouched
   and surface through the toast. */
listen("compact-finished", async (event) => {
  const p = event.payload;
  delete state.compacting[p.session_id];
  if (!p.ok) {
    state.error = p.error || "compaction failed";
    renderStatus();
    toast(`Compact failed: ${state.error}`);
    return;
  }
  state.error = null;
  state.usage = null;
  state.toolCount = 0;
  try {
    if (state.cache[p.session_id]) {
      state.cache[p.session_id] = await invoke("load_session", { id: p.session_id });
    }
  } catch (_) {
    delete state.cache[p.session_id];
  }
  if (p.session_id === state.activeId) renderAll();
  else renderStatus();
  toast(`Compacted: ${p.before} → ${p.after} messages`);
});

/* The backend finished (or abandoned) an /init attempt: drop the in-flight
   flag and report where the document landed (or why it did not). */
listen("init-finished", (event) => {
  const p = event.payload;
  state.initializing = false;
  renderStatus();
  if (p.ok) {
    toast(`AGENTS.md written: ${p.path}`);
  } else {
    toast(`Init failed: ${p.error || "unknown error"}`);
  }
});

/* ---------- wiring ---------- */

/* Delegated: bubbles are rebuilt on every event, so per-render listeners
   would leak; these handle clicks inside re-rendered content instead. */
messagesEl.addEventListener("click", async (ev) => {
  const codeBtn = ev.target.closest(".code-copy");
  if (codeBtn) {
    const codeEl = codeBtn.closest("pre") && codeBtn.closest("pre").querySelector("code");
    if (codeEl) flash(codeBtn, (await copyText(codeEl.textContent)) ? "Copied" : "Copy failed");
    return;
  }
  const link = ev.target.closest("a[href]");
  if (link) {
    ev.preventDefault();
    invoke("open_url", { url: link.getAttribute("href") }).catch((err) => {
      state.error = String(err);
      renderStatus();
    });
  }
});

$("new-chat").addEventListener("click", () => newChat().catch(() => {}));
$("open-settings").addEventListener("click", () => openSettings());

/* Sidebar navigation opens the panel that owns the thing instead of inventing a
   second surface for it. Automation stays disabled until background jobs land
   (milestone M7), which is more honest than a button that does nothing. */
/* Settings is one surface with several sections. Both the sidebar shortcuts and
   the tabs inside the dialog route through here, so plugins, skills, MCP servers
   and providers never grow a second place to be edited. */
/* The Workspaces section of Settings: what each workspace is, and the two
   things the sidebar header does — make it active, or open its form. The form
   itself stays in the sidebar, because that is where the chats are. */
/* Content search (§17). The box filters titles locally as you type; from two
   characters on, the server also looks inside the chats, and its hits are drawn
   above the groups. A response that arrives after the query changed is dropped,
   because a late answer must not repaint a search the user has moved on from. */
async function refreshSearchHits() {
  const query = state.search.trim();
  if (query.length < 2) {
    state.hits = [];
    return;
  }
  const hits = await invoke("search_sessions", { query, limit: 20 });
  if (state.search.trim() !== query) return;
  state.hits = hits;
  renderSidebar();
}

/* Open a search result. A hit is findable from any workspace, so the workspace
   has to move first — awaiting it keeps the order right, since the command
   re-lists sessions and the chat has to be switched after that, not during. */
async function openHit(hit) {
  if (hit.workspace && hit.workspace !== state.activeWorkspace) {
    await workspaceCommand(invoke("set_active_workspace", { id: hit.workspace }));
  }
  switchSession(hit.session);
}

/* The memory panel (§20). The store is shared by every workspace, so these all
   answer with the list as it stands and the panel simply draws that — no local
   editing of a list the agent also writes to. */
async function loadMemories() {
  try {
    state.memories = await invoke("list_memories", { sessionId: state.activeId });
    renderMemories();
  } catch (err) {
    renderMemoryNote(String(err));
  }
}

async function addMemory() {
  const field = $("mem-text");
  const text = field ? field.value : "";
  // The server checks this too; refusing here just saves a round trip.
  if (!text.trim()) {
    renderMemoryNote("a memory needs some text");
    return;
  }
  try {
    state.memories = await invoke("remember_memory", { sessionId: state.activeId, text });
    if (field) field.value = "";
    renderMemories();
  } catch (err) {
    renderMemoryNote(String(err));
  }
}

async function forgetMemory(id) {
  try {
    state.memories = await invoke("forget_memory", { sessionId: state.activeId, id });
    renderMemories();
  } catch (err) {
    renderMemoryNote(String(err));
  }
}

function renderMemoryNote(text) {
  const host = $("mem-items");
  if (!host) return;
  host.textContent = "";
  host.appendChild(el("div", "ws-form-hint", text));
}

function renderMemories() {
  const host = $("mem-items");
  if (!host) return;
  host.textContent = "";
  // Whose memory this is. The store follows the workspace, so a list without an
  // owner line is a list that cannot be checked against the right file.
  const owner = state.workspaces.find((w) => w.id === state.activeWorkspace);
  host.appendChild(el("div", "ws-head", `Memory for ${owner ? owner.name : "this workspace"}`));
  if (!state.memories.length) {
    host.appendChild(el("div", "ws-form-hint", "Nothing remembered yet."));
    return;
  }
  for (const item of state.memories) {
    const row = el("div", "ws-tools mem-row");
    row.appendChild(el("div", "", item.content));
    const when = new Date(item.created_at_ms || 0);
    row.appendChild(el("div", "ws-form-hint", Number.isNaN(when.getTime()) ? "" : when.toLocaleString()));
    const forget = el("button", "ws-form-go", "Forget");
    forget.type = "button";
    forget.addEventListener("click", (e) => {
      e.preventDefault();
      forgetMemory(item.id);
    });
    row.appendChild(forget);
    host.appendChild(row);
  }
}

/* Token counts, shortened the way a status line is read: 999 stays exact, 12 345
   is "12.3k", and past ten thousand the decimal is noise. */
function fmtTokens(n) {
  const value = Number(n) || 0;
  if (value < 1000) return String(value);
  return value < 10000 ? `${(value / 1000).toFixed(1)}k` : `${(value / 1000).toFixed(0)}k`;
}

/* The token meter. It reports the provider's own numbers rather than an estimate,
   and warns at 80% of the context budget, which is the point where the next turn
   is more likely to be compacted than answered. */
function renderUsage() {
  const host = $("status-right");
  if (!host) return;
  let meter = host.querySelector(".usage-meter");
  if (!meter) {
    meter = el("span", "usage-meter", "");
    host.appendChild(meter);
  }
  const usage = state.usage || { prompt: 0, completion: 0 };
  const parts = [`in ${fmtTokens(usage.prompt)}`, `out ${fmtTokens(usage.completion)}`];
  const budget = Number(state.budget) || 0;
  const near = budget > 0 && usage.prompt / budget >= 0.8;
  if (budget > 0) parts.push(`${Math.round((usage.prompt / budget) * 100)}% of ${fmtTokens(budget)}`);
  if (near) parts.push("near the context limit");
  meter.textContent = parts.join(" · ");
  meter.classList.toggle("near-limit", near);
}

function renderSearchHits() {
  if (!state.hits || !state.hits.length) return;
  sessionList.appendChild(el("div", "ws-head", `${state.hits.length} match${state.hits.length === 1 ? "" : "es"} inside chats`));
  for (const hit of state.hits) {
    const row = el("div", "session-row hit-row");
    row.appendChild(el("span", "title", clip(hit.title || "(untitled)", 24)));
    // Which workspace it came from, because a hit is findable from anywhere and
    // the chat it belongs to may not be in the one that is in use.
    const where = state.workspaces.find((w) => w.id === hit.workspace);
    row.appendChild(el("span", "time", clip(where ? where.name : hit.workspace, 10)));
    row.appendChild(el("div", "hit-snippet", hit.snippet));
    row.addEventListener("click", () => openHit(hit));
    sessionList.appendChild(row);
  }
}

function renderWorkspaceManager() {
  const host = $("ws-manage-list");
  if (!host) return;
  host.textContent = "";
  for (const w of state.workspaces || []) {
    const card = el("div", "ws-tools");
    const active = w.id === state.activeWorkspace;
    card.appendChild(el("div", "ws-form-hint", `${w.name || "(unnamed)"}${active ? " · in use" : ""}`));
    card.appendChild(el("div", "", `root: ${w.root || "(current folder)"}`));
    card.appendChild(el("div", "", `model: ${w.model || "inherits the defaults"}`));
    if (w.reasoning_effort) card.appendChild(el("div", "", `effort: ${w.reasoning_effort}`));
    const chats = (state.sessions || []).filter((s) => s.workspace === w.id).length;
    card.appendChild(el("div", "", `${chats} chat${chats === 1 ? "" : "s"}`));
    const use = el("button", "ws-form-go", active ? "In use" : "Use");
    use.type = "button"; // inside the settings form, a bare button would submit it
    use.disabled = active;
    use.addEventListener("click", (e) => {
      e.preventDefault();
      switchWorkspace(w.id);
    });
    card.appendChild(use);
    const edit = el("button", "ws-form-go", "Edit");
    edit.type = "button";
    edit.addEventListener("click", (e) => {
      e.preventDefault();
      closeSettings();
      openWorkspaceForm("edit", w.id);
    });
    card.appendChild(edit);
    host.appendChild(card);
  }
}

function focusSection(sectionId) {
  const target = document.getElementById(sectionId);
  if (target) target.scrollIntoView({ block: "start" });
}

function navTo(sectionId) {
  Promise.resolve(openSettings())
    .catch(() => {})
    .then(() => focusSection(sectionId));
}

document.querySelectorAll(".set-tab").forEach((tab) => {
  tab.addEventListener("click", () => focusSection(tab.dataset.section));
});

$("nav-plugins").addEventListener("click", () => navTo("cap-list"));
$("nav-mcp").addEventListener("click", () => navTo("mcp-list"));

// Narrow windows turn the sidebar into a drawer (see the 720px rules): the
// button opens it, and picking a chat closes it, because the point of that tap is
// the chat rather than the list it came from.
const sidebarToggle = $("sidebar-toggle");
const sidebarEl = $("sidebar");
if (sidebarToggle && sidebarEl) {
  const setDrawer = (open) => {
    sidebarEl.classList.toggle("open", open);
    sidebarToggle.setAttribute("aria-expanded", open ? "true" : "false");
  };
  sidebarToggle.addEventListener("click", () => setDrawer(!sidebarEl.classList.contains("open")));
  sidebarEl.addEventListener("click", (event) => {
    const row = event.target.closest ? event.target.closest(".session-row") : null;
    if (row) setDrawer(false);
  });
}

const addMemoryButton = $("mem-add");
if (addMemoryButton) addMemoryButton.addEventListener("click", () => addMemory());

// Guarded: the settings section exists in the app shell, and the harness page
// loads this file without it.
const addWorkspace = $("ws-manage-add");
if (addWorkspace) {
  addWorkspace.addEventListener("click", () => {
    closeSettings();
    openWorkspaceForm("create", "");
  });
}
$("plan-edit").addEventListener("click", () => startPlanEdit());
sendBtn.addEventListener("click", () =>
  streamOf(state.activeId) ? stopStream() : send(),
);
sessionSearch.addEventListener("input", () => {
  state.search = sessionSearch.value;
  renderSidebar();
  refreshSearchHits();
});
input.addEventListener("input", () => {
  autosize();
  renderSend();
  syncPalette();
  syncMention();
});
// Caret moves change which token the picker sees — re-evaluate on click and
// on the horizontal cursor keys (Enter/Tab/arrows are handled in keydown).
input.addEventListener("click", () => syncMention());
input.addEventListener("keyup", (e) => {
  if (e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "Home" || e.key === "End") {
    syncMention();
  }
});
input.addEventListener("keydown", (e) => {
  const p = state.palette;
  const open = p && !paletteEl.classList.contains("hidden");
  if (open && p.entries.length) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      p.index = (p.index + 1) % p.entries.length;
      renderPalette();
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      p.index = (p.index - 1 + p.entries.length) % p.entries.length;
      renderPalette();
      return;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      selectPalette(p.index);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      closePalette();
      return;
    }
  }
  if (open && e.key === "Escape") {
    e.preventDefault();
    closePalette();
    return;
  }
  // @file picker (palette wins when both would be open — they are disjoint
  // in practice: "/" needs the whole value, "@" needs a trailing token).
  const m = state.mention;
  const mentionOpen = m && !mentionEl.classList.contains("hidden");
  if (mentionOpen && m.entries.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
    e.preventDefault();
    m.index = (m.index + (e.key === "ArrowDown" ? 1 : -1) + m.entries.length) % m.entries.length;
    renderMention();
    return;
  }
  if (mentionOpen && m.entries.length && (e.key === "Enter" || e.key === "Tab")) {
    e.preventDefault();
    insertMention(m.index);
    return;
  }
  if (mentionOpen && e.key === "Escape") {
    e.preventDefault();
    state.mentionDismissed = m.query; // stays closed until the token changes
    closeMention();
    return;
  }
  /* Prompt history: ↑/↓ walk this chat's stored user prompts. Recall only
     engages on an empty composer (or while already engaged), so multi-line
     drafts keep their normal caret movement. The list is re-read on every
     step and PromptNav clamps positions, so edits/deletes mid-walk can never
     strand the cursor on a stale index. */
  if (e.key === "ArrowUp" || (e.key === "ArrowDown" && state.promptNav)) {
    const engageable = state.promptNav || input.value === "";
    if (engageable) {
      e.preventDefault();
      const list = (state.cache[state.activeId] || [])
        .filter((m) => m.role === "User" && m.text && m.text.trim())
        .map((m) => m.text);
      const step =
        e.key === "ArrowUp"
          ? PromptNav.up(list, state.promptNav, input.value)
          : PromptNav.down(list, state.promptNav);
      state.promptNav = step.nav;
      if (step.text !== null) {
        input.value = step.text;
        input.setSelectionRange(step.text.length, step.text.length);
        autosize();
        renderSend();
      }
      return;
    }
  }
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    closePalette();
    closeMention();
    send();
  }
});

/* ---------- file sidebar ----------
   The workspace on disk, as a lazy tree in the left sidebar. One directory
   per click: the Rust side lists a single level — contained in the
   workspace, capped, sorted — and this keeps what came back, so expanding a
   folder never walks the repository. A click on a file is the same
   `read_file` the document panel already trusts, and the doc panel stays a
   plain viewer: the tree is interactive, the panel is not. */

/* cache[path] is missing (closed) or { status, entries, error }; "" is the
   workspace root. `status` is "loading" | "ok" | "error". */
const fileTree = { open: false, cache: {} };

/* Flatten the lazy cache into the rows the tree paints, in paint order.
   Pure — the harness feeds a fixture cache and checks order, depth,
   expansion and the failure rows, so the drawing rules are testable without
   a backend. */
function fileRowsFrom(cache) {
  const rows = [];
  const walk = (path, depth) => {
    const node = cache[path];
    if (!node) return;
    if (node.status === "loading") {
      rows.push({ path, name: "", depth, isDir: true, bytes: 0, expanded: false, state: "loading" });
      return;
    }
    if (node.status === "error") {
      rows.push({
        path,
        name: path ? path.split("/").pop() : "workspace",
        depth,
        isDir: true,
        bytes: 0,
        expanded: false,
        state: "error",
        error: node.error || "unreadable",
      });
      return;
    }
    for (const entry of node.entries || []) {
      // The backend sends snake_case like every other payload here, so the
      // sidebar reads is_dir — reading a camelCase alias would make every
      // folder look like a file.
      const child = entry.is_dir ? cache[entry.path] : null;
      const rowState = !entry.is_dir
        ? "file"
        : !child
          ? "closed"
          : child.status === "ok"
            ? "open"
            : child.status;
      rows.push({
        path: entry.path,
        name: entry.name,
        depth,
        isDir: !!entry.is_dir,
        bytes: Number(entry.bytes) || 0,
        expanded: rowState === "open",
        state: rowState,
        // A failed folder says why on its own row: its children are never
        // walked, so this row is the only place the reason can appear.
        error: rowState === "error" && child ? child.error || "unreadable" : undefined,
      });
      if (rowState === "open") walk(entry.path, depth + 1);
    }
  };
  walk("", 0);
  return rows;
}

/* Human sizes for the sidebar: 512 → "512 B", 20480 → "20.0 kB". */
function formatFileBytes(bytes) {
  const n = Number(bytes) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} kB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

/* Load one directory into the cache and repaint. A failure lands in the
   cache as an error row, so a folder that will not open says why instead of
   silently staying closed. */
async function loadFileDir(path) {
  fileTree.cache[path] = { status: "loading", entries: [] };
  renderFileTree();
  try {
    const list = await invoke("list_files", { path });
    fileTree.cache[path] = {
      status: "ok",
      entries: (list && list.entries) || [],
      truncated: !!(list && list.truncated),
    };
  } catch (err) {
    fileTree.cache[path] = { status: "error", error: String(err), entries: [] };
  }
  renderFileTree();
  persistFileTree();
}

/* Open a folder, or collapse the one that is open. An error row retries on
   the next click — the only affordance a failure has. */
function toggleFileDir(path) {
  const node = fileTree.cache[path];
  if (node && node.status === "ok") {
    delete fileTree.cache[path];
    renderFileTree();
    persistFileTree();
    return;
  }
  if (!node || node.status === "error") void loadFileDir(path);
}

/* Paint the rows. textContent only: names come off disk and are untrusted. */
function renderFileTree() {
  const host = $("file-tree");
  if (!host) return;
  host.textContent = "";
  if (!fileTree.open) {
    host.hidden = true;
    return;
  }
  host.hidden = false;
  for (const row of fileRowsFrom(fileTree.cache)) {
    const el = document.createElement("button");
    el.type = "button";
    el.className = `file-row state-${row.state}`;
    el.style.paddingLeft = `${6 + row.depth * 14}px`;
    el.setAttribute("role", "treeitem");
    el.dataset.path = row.path;
    el.dataset.dir = row.isDir ? "1" : "";
    if (row.isDir) el.setAttribute("aria-expanded", String(row.expanded));
    const glyph = document.createElement("span");
    glyph.className = "file-glyph";
    glyph.setAttribute("aria-hidden", "true");
    glyph.textContent =
      row.state === "loading"
        ? "…"
        : row.state === "error"
          ? "⚠"
          : row.isDir
            ? row.expanded
              ? "▾"
              : "▸"
            : "·";
    const name = document.createElement("span");
    name.className = "file-name";
    name.textContent = row.name || row.path;
    el.appendChild(glyph);
    el.appendChild(name);
    if (row.state === "error") {
      // The reason a folder will not open, beside the folder that will not
      // open; the full text is also on hover, since the row clips it.
      el.title = row.error || "";
      const why = document.createElement("span");
      why.className = "file-size";
      why.textContent = row.error || "";
      el.appendChild(why);
    } else if (row.state === "file") {
      const size = document.createElement("span");
      size.className = "file-size";
      size.textContent = formatFileBytes(row.bytes);
      el.appendChild(size);
    }
    host.appendChild(el);
  }
  // A directory the backend had to cut short says so, the way a truncated
  // file read does, rather than looking like a complete listing.
  const note = Object.values(fileTree.cache).some((node) => node.truncated);
  if (note) {
    const cut = document.createElement("div");
    cut.className = "file-note";
    cut.textContent = "Some entries are not shown — this folder is too large to list in full.";
    host.appendChild(cut);
  }
}

/* Open folders survive a restart: the open paths are stored per workspace
   root and re-fetched on load — a fetch, not a snapshot, so what comes back
   is the directory as it is now. */
function fileTreeKey() {
  return "bos.filetree." + (state.workspaceRoot || "default");
}

function persistFileTree() {
  try {
    const open = Object.keys(fileTree.cache)
      .filter((path) => fileTree.cache[path].status === "ok")
      .sort();
    localStorage.setItem(fileTreeKey(), JSON.stringify(open));
  } catch (_) {
    /* private mode or a full quota: the tree still works, it just forgets */
  }
}

async function restoreFileTree() {
  let open = [];
  try {
    const saved = JSON.parse(localStorage.getItem(fileTreeKey()) || "[]");
    if (Array.isArray(saved)) open = saved.filter((path) => typeof path === "string");
  } catch (_) {
    open = [];
  }
  if (!open.length) {
    await loadFileDir("");
    return;
  }
  // Each listing stands on its own, so they fill in as they arrive.
  await Promise.all(open.map((path) => loadFileDir(path)));
}

/* Show or hide the panel; the first open loads the workspace root. */
async function toggleFilePanel() {
  fileTree.open = !fileTree.open;
  const toggle = $("files-toggle");
  if (toggle) toggle.setAttribute("aria-expanded", String(fileTree.open));
  if (fileTree.open && !Object.keys(fileTree.cache).length) await restoreFileTree();
  renderFileTree();
}

function refreshFileTree() {
  fileTree.cache = {};
  void restoreFileTree();
}

$("files-toggle").addEventListener("click", () => void toggleFilePanel());
$("files-refresh").addEventListener("click", () => refreshFileTree());
$("file-tree").addEventListener("click", (ev) => {
  const row = ev.target && ev.target.closest ? ev.target.closest(".file-row") : null;
  if (!row) return;
  const path = row.dataset.path || "";
  if (!path) return;
  if (row.dataset.dir === "1") toggleFileDir(path);
  else void openFileInPanel(path);
});

/* ---------- run panel ---------- */
/* Bounded command output — deliberately not a terminal. The backend runs the
   command against pipes (no PTY in this build), so there is no interactivity
   and no ANSI colour; the header says so out loud. What the panel showed
   survives a reload: kept lines are stored per workspace root, capped like
   the file tree, and re-shown as they were — the next run appends below. */

/* Cap on lines kept in the panel and in storage; past this the oldest scroll
   off, exactly as they scrolled off the screen. */
const MAX_RUN_LINES = 400;

function runPanelKey() {
  return "bos.runs." + (state.workspaceRoot || "default");
}

/** Keep the newest `max` lines — what fell off the front stays gone. */
function trimRunLines(lines, max) {
  return lines.length > max ? lines.slice(lines.length - max) : lines;
}

/** One-line receipt for a finished run: the honest bits — code, size, time. */
function formatRunFooter(p) {
  const exit = p.exit_code === null || p.exit_code === undefined ? "stopped" : `exit ${p.exit_code}`;
  const secs = ((p.duration_ms || 0) / 1000).toFixed(1);
  const notes = [];
  if (p.truncated) notes.push("stopped by the budget");
  if (p.error) notes.push(p.error);
  return `[${exit} · ${p.lines} lines · ${secs}s${notes.map((n) => ` · ${n}`).join("")}]`;
}

function pushRunLine(stream, text) {
  state.runPanel.lines.push({ stream, text });
  state.runPanel.lines = trimRunLines(state.runPanel.lines, MAX_RUN_LINES);
}

function setRunChip(running) {
  const el = $("run-chip");
  if (!el) return;
  el.textContent = running ? "running" : "idle";
  el.className = "chip " + (running ? "streaming" : "idle");
}

function renderRunPanel() {
  const panel = $("run-panel");
  if (panel) panel.hidden = !state.runPanel.open;
  const out = $("run-output");
  if (!out) return;
  const frag = document.createDocumentFragment();
  for (const line of state.runPanel.lines) {
    const span = document.createElement("span");
    const kind = line.stream === "stderr" ? "err" : line.stream === "stdout" ? "out" : "note";
    span.className = "run-line " + kind;
    // Command text and file names are untrusted: textContent only.
    span.textContent = line.text;
    frag.appendChild(span);
    frag.appendChild(document.createTextNode("\n"));
  }
  out.textContent = "";
  out.appendChild(frag);
  out.scrollTop = out.scrollHeight;
}

/* A chatty command must not repaint per line: the panel catches up once per
   frame, the same trick the transcript uses. */
let runPaintQueued = false;
function scheduleRunPaint() {
  if (runPaintQueued) return;
  runPaintQueued = true;
  requestAnimationFrame(() => {
    runPaintQueued = false;
    renderRunPanel();
  });
}

function persistRunLines() {
  state.runPanel.lines = trimRunLines(state.runPanel.lines, MAX_RUN_LINES);
  try {
    localStorage.setItem(runPanelKey(), JSON.stringify(state.runPanel.lines));
  } catch (_) {
    /* private mode or a full quota: the panel still works, it just forgets */
  }
}

/** Load the lines the last session showed for this workspace root. */
function restoreRunLines() {
  let lines = [];
  try {
    const saved = JSON.parse(localStorage.getItem(runPanelKey()) || "[]");
    if (Array.isArray(saved)) {
      lines = saved.filter(
        (l) => l && typeof l.text === "string" && typeof l.stream === "string"
      );
    }
  } catch (_) {
    lines = [];
  }
  state.runPanel.lines = lines;
}

async function toggleRunPanel() {
  state.runPanel.open = !state.runPanel.open;
  renderRunPanel();
  if (state.runPanel.open) {
    const field = $("run-input");
    if (field) field.focus();
  }
}

$("run-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const field = $("run-input");
  const command = (field.value || "").trim();
  // One run at a time: a second command would interleave its lines with the
  // first, and the panel has no way to tell them apart on screen.
  if (!command || state.runPanel.activeId) return;
  field.value = "";
  const runId = `run-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
  state.runPanel.activeId = runId;
  setRunChip(true);
  pushRunLine("note", "$ " + command);
  scheduleRunPaint();
  try {
    await invoke("run_command", { runId, command });
  } catch (err) {
    state.runPanel.activeId = null;
    setRunChip(false);
    pushRunLine("note", `[refused: ${err}]`);
    scheduleRunPaint();
    persistRunLines();
  }
});

$("run-clear").addEventListener("click", () => {
  state.runPanel.lines = [];
  renderRunPanel();
  persistRunLines();
});

$("run-close").addEventListener("click", () => {
  state.runPanel.open = false;
  renderRunPanel();
});

listen("run-line", (event) => {
  const p = event.payload;
  if (p.run_id !== state.runPanel.activeId) return;
  pushRunLine(p.stream, p.text);
  scheduleRunPaint();
});

listen("run-finished", (event) => {
  const p = event.payload;
  if (p.run_id !== state.runPanel.activeId) return;
  state.runPanel.activeId = null;
  setRunChip(false);
  pushRunLine("note", formatRunFooter(p));
  scheduleRunPaint();
  persistRunLines();
});

/* ---------- init ---------- */

async function init() {
  try {
    await refreshSessions();
    const streams = await invoke("current_stream");
    for (const s of streams) {
      state.streams[s.session_id] = s;
      // Warm each streaming chat's cache so switching to it renders live.
      ensureSessionCache(s.session_id);
    }
    const settings = await invoke("get_settings");
    state.model = settings.model;
    state.budget = settings.context_budget ?? 32768;
    // Workspaces drive how the panel groups chats and which model each group
    // will use, so the sidebar needs them even before settings are opened.
    state.workspaces = settings.workspaces || [];
    state.activeWorkspace = settings.active_workspace || "";
    // The file sidebar lists this root and keys its per-workspace memory by
    // it, so it needs it before the first open.
    state.workspaceRoot = settings.bash_workspace || "";
    state.presets = settings.presets || [];
    // The run panel keeps what it showed: reload that history for this root.
    restoreRunLines();
    renderRunPanel();
    // Start page context: what this app instance is pointed at.
    Shell.setContext({
      workspace: settings.bash_workspace || "",
      model: settings.model || "",
    });
  } catch (err) {
    state.error = String(err);
  }
  renderAll();
  setInterval(tick, 1000);
}

init();
