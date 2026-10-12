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
  wsAddOpener: null,    // what had focus when "Add a workspace" opened
  wsBrowse: null,       // the folder chooser's current level, or null when closed
  wsBusy: false,        // a workspace command is in flight
  windowExtra: 0,       // extra rows kept above the tail window
  collapsed: {},        // workspace groups whose chats are hidden
  // How the sidebar draws the chats, as the head's sliders menu sets it: which
  // rows are grouped (and under what), in what order, and which archived chats
  // are listed at all. One object, one writer (that menu), one reader
  // (renderSidebar) — the archive used to be a second flag of its own.
  view: { group: "tree", order: "updated", filter: "hide" },
  centerTab: "chat",    // header view switch: "chat" (transcript) | "trajectory" (steps)
  hits: [],             // content matches for the current search
  usage: { prompt: 0, completion: 0 }, // what the provider said the last turn cost
  memories: [],         // what the agent remembers across chats
  memoryStatus: null,   // where that store lives and what bounds it (§20)
  knowledge: null,      // the curated notes, as the last command answered (§21)
  skillsReport: null,   // the skills directory, as skill_list answered
  skillActivation: null, // which skills the open chat carries, as skill_activation answered
  kbEditing: null,      // id of the entry the form is editing, or null
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
  mcpPolicy: null,       // {default, rules, servers} from mcp_policy_status
  mcpCatalog: null,      // what the connected servers publish: resources + prompts
  mcpRead: null,         // the last resource read, as the command answered
  mcpPolling: false,     // true while a status follow-up loop is running
  search: "",            // sidebar filter (session-title substring)
  palette: null,         // {query, entries, index} | null — slash palette
  skills: [],            // cached skills for the palette [{name, description}]
  mention: null,         // {query, atStart, entries, index, fresh} | null — @file picker
  mentionDismissed: "",  // token query dismissed with Esc (reopens on change)
  promptNav: null,       // {pos, draft} | null — ↑/↓ prompt-history walk
  pendingImages: [],     // data URLs attached to the next message (this composer)
  runPanel: { open: false, activeId: null, lines: [], mode: "run", partial: "" }, // streamed command output, or an interactive PTY session
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
const setWebTools = $("set-web-tools");
const setSearchUrl = $("set-search-url");
const setSearchKey = $("set-search-key");
const setContextBudget = $("set-context-budget");
const capItems = $("cap-items");
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

/* The row's own icons, drawn the way the header's are written in the markup: an
   outline on a 24-grid, stroked in currentColor, so the sidebar speaks one icon
   language instead of a font's idea of "+", "…" and a folder (§24.4). Built with
   createElementNS rather than innerHTML, because a string of markup is a second
   place the icon set can disagree with itself. */
function ico(paths, solid) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("class", solid ? "ico ico-solid" : "ico");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  for (const d of paths) {
    const p = document.createElementNS(ns, "path");
    p.setAttribute("d", d);
    svg.appendChild(p);
  }
  return svg;
}
const ICONS = {
  // The folder is the header's folder-plus without its plus: a workspace *is* a folder.
  folder: ["M3 7.6A1.6 1.6 0 0 1 4.6 6h3.6L10.4 8.2h9A1.6 1.6 0 0 1 21 9.8v7.6A1.6 1.6 0 0 1 19.4 19H4.6A1.6 1.6 0 0 1 3 17.4z"],
  // A plus inside a ring: the row's own "start a chat here", the reference's ⊕.
  plusCircle: ["M12 4.4a7.6 7.6 0 1 1 0 15.2 7.6 7.6 0 0 1 0-15.2z", "M12 8.8v6.4M8.8 12h6.4"],
  // Three dots, drawn as zero-length strokes with the round cap they inherit.
  dots: ["M7 12h.01M12 12h.01M17 12h.01"],
  // The disclosure's two states, as two solid triangles: a collapsed group points
  // right, an open one points down, and the shape carries the state because the
  // user could not see the small stroked triangle the row used to have (rev 119).
  triRight: ["M9.2 6.4 17.6 12 9.2 17.6z"],
  triDown: ["M6.4 9.2 17.6 9.2 12 17.6z"],
};

/* What a chat's row offers, in one place and free of the DOM.
 *
 * The labels flip with the chat's own state (a pinned chat offers "Unpin", an
 * archived one offers "Restore") because a menu that always says "Pin" cannot
 * tell the reader what the click will do. `run` names the action rather than
 * carrying a closure, so the model is a plain value the fixture guard can read.
 * Accelerators are spelled "Cmd/Ctrl+…" and not the bare glyph: this interface
 * answers the same keys on every platform, and a hint that names only one of
 * them is wrong on the other.
 */
function sessionMenu(s) {
  const on = !!s.pinned;
  const archived = !!s.archived;
  return [
    {
      run: "pin",
      label: on
        ? tr("session.unpin", "Unpin session")
        : tr("session.pin", "Pin session"),
    },
    { run: "rename", label: tr("session.rename", "Rename"), accel: "Cmd/Ctrl+G" },
    { run: "fork", label: tr("session.fork", "Fork session"), accel: "Cmd/Ctrl+F" },
    {
      run: "archive",
      label: archived
        ? tr("session.unarchive", "Restore session")
        : tr("session.archive", "Archive session"),
      accel: archived ? "" : "Cmd/Ctrl+Shift+A",
    },
    { run: "delete", label: tr("session.delete", "Delete session"), danger: true },
  ];
}

function workspaceMenu(w) {
  return [
    { run: "new", label: tr("ws.new", "New session"), accel: "Cmd/Ctrl+N" },
    { run: "rename", label: tr("ws.rename", "Rename workspace") },
    { run: "delete", label: tr("ws.delete", "Delete workspace"), danger: true },
  ];
}

/* The archived chats, newest first. Archived is not gone: the row is drawn
   behind a disclosure and this is the list that puts it back. */
function archivedSessions(sessions) {
  return (sessions || [])
    .filter((s) => !!s.archived)
    .slice()
    .sort((a, b) => (b.updated_at || 0) - (a.updated_at || 0));
}

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
/* How the sidebar is drawn survives a restart: it is how this person reads their
   own chats, not a property of any workspace, so it is one blob of three choices
   in local storage. A value that is gone or nonsense means the default, never a
   crash — an older blob may not have the field at all. */
const SESSION_VIEW_KEY = "bos.sessionView";
const SESSION_VIEW_CHOICES = {
  group: ["tree", "workspace", "list"],
  order: ["updated", "manual"],
  filter: ["hide", "all", "archived"],
};
function parseViewBlob(raw) {
  const out = { group: "tree", order: "updated", filter: "hide" };
  if (raw && typeof raw === "object") {
    for (const key of Object.keys(out)) {
      if (SESSION_VIEW_CHOICES[key].includes(raw[key])) out[key] = raw[key];
    }
  }
  return out;
}
function persistSessionView() {
  try {
    localStorage.setItem(SESSION_VIEW_KEY, JSON.stringify(state.view));
  } catch (_) {
    /* private mode or a full quota: the sidebar still follows the choice, it
       just forgets it next time */
  }
}
function restoreSessionView() {
  let raw = null;
  try {
    raw = JSON.parse(localStorage.getItem(SESSION_VIEW_KEY) || "null");
  } catch (_) {
    raw = null;
  }
  state.view = parseViewBlob(raw);
}
/** Set one of the three choices, remember it, and redraw the list it decides. */
function setSessionView(patch) {
  state.view = { ...state.view, ...patch };
  persistSessionView();
  renderSidebar();
}

/* Pinned first, then the order the person chose in the head's menu. A pin is a
   statement about the chat, not about which workspace holds it, so it floats to
   the top of whatever list the chat is drawn in. "Manual" keeps the backend's
   order — the sort is stable, so ties stay where the store put them — and "Last
   updated" puts the freshest first. */
function orderSessions(items, view) {
  return items.slice().sort(
    (a, b) =>
      (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0) ||
      (view.order === "manual" ? 0 : (b.updated_at || 0) - (a.updated_at || 0)),
  );
}

function groupSessions(sessions, workspaces, activeId, view) {
  const onlyArchived = view.filter === "archived";
  const byId = new Map((workspaces || []).map((w) => [w.id, w]));
  const groups = new Map();
  for (const s of sessions) {
    // Which archived chats are listed is the filter's decision, made in one
    // place: "hide" leaves them to the count in the menu, "all" puts them back
    // among their own workspace's chats, "archived" shows nothing else.
    if (onlyArchived ? !s.archived : s.archived && view.filter === "hide") continue;
    const id = s.workspace || activeId || "";
    if (!groups.has(id)) groups.set(id, []);
    groups.get(id).push(s);
  }
  // A workspace exists whether or not it has a chat yet, so it gets a group
  // either way: a folder just added would otherwise be invisible until someone
  // happened to start a chat in it, and there would be nothing to aim "+" at.
  // The ones the sessions did not mention come after them, in the order the
  // workspaces are configured. "Archived only" is the exception: a workspace
  // with nothing archived is not a group in a list of archived chats.
  if (!onlyArchived) {
    for (const w of workspaces || []) {
      if (w && w.id && !groups.has(w.id)) groups.set(w.id, []);
    }
  }
  return [...groups.entries()].map(([id, items]) => {
    const w = byId.get(id);
    return {
      id,
      name: (w && (w.name || w.root)) || id || "Default",
      root: (w && w.root) || "",
      model: (w && w.model) || "",
      sessions: orderSessions(items, view),
    };
  });
}

/* Every workspace command returns the newly effective settings, because the
   server bumps its settings generation when the choice changes (each cached agent
   is then rebuilt from the new model and tool set). One applier keeps that
   invariant in a single place — and re-points the transcript at the workspace
   the snapshot names, so the chat on screen is always one the active
   workspace's own configuration would run. */
async function applyWorkspaceSnapshot(s) {
  state.workspaces = s.workspaces || [];
  state.presets = s.presets || [];
  state.activeWorkspace = s.active_workspace || "";
  state.model = s.model || state.model;
  state.budget = s.context_budget ?? state.budget;
  const list = await invoke("list_sessions");
  state.sessions = Array.isArray(list) ? list : [];
  await syncTranscriptToWorkspace();
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

/* ---------- Add a workspace ---------------------------------------------- */

/* Adding a workspace starts where a workspace is: the folder. The sidebar's
   folder-plus therefore asks the system's own panel *first*, and the pick is the
   workspace — named for the folder, inheriting today's defaults, and with its own
   `.bos` written before the record exists (§24.15), the way a project carries
   `.vscode`.

   The form below is the fallback, not the door: a machine with no panel gets it
   with the reason and its own way into the tree, and a folder that will not take
   its configuration gets it with the folder and the reason still in it. Nothing
   is lost by creating in one gesture, because the model, the MCP servers, the
   plugins, the tools and the skills are all on the workspace's own Edit form — a
   workspace made this way inherits until it is asked differently, rather than
   being configured wrong and edited right. */
function addWorkspaceModal() {
  return $("ws-add");
}

function addWorkspaceOpen() {
  const modal = addWorkspaceModal();
  return !!modal && !modal.classList.contains("hidden");
}

function openAddWorkspace() {
  const modal = addWorkspaceModal();
  if (!modal) return;
  state.wsAddOpener = document.activeElement;
  state.wsBrowse = null;
  hideInAppBrowser();
  $("ws-add-err").textContent = "";
  $("ws-add-name").value = "";
  $("ws-add-root").value = "";
  $("ws-add-model").value = "";
  $("ws-add-bash").checked = true;
  $("ws-add-files").checked = true;
  $("ws-add-approval").checked = true;
  fillAddWorkspaceLists();
  modal.classList.remove("hidden");
  $("ws-add-name").focus();
  // The plugins and the skills are whatever the backend last registered, so the
  // lists are re-read rather than remembered: a skill installed a minute ago
  // should be offerable now.
  void refreshAddWorkspaceLists();
}

function closeAddWorkspace() {
  const modal = addWorkspaceModal();
  if (!modal || modal.classList.contains("hidden")) return;
  modal.classList.add("hidden");
  const back = state.wsAddOpener;
  state.wsAddOpener = null;
  if (back && back.focus) back.focus();
}

/* A folder's own name: what a workspace made from a pick is called, and what the
   form suggests when the same folder is chosen there. */
function folderName(path) {
  const parts = String(path).split("/").filter(Boolean);
  return parts.length ? parts[parts.length - 1] : "";
}

/* The sidebar's folder-plus: the system's panel first, then the workspace a
   folder makes. A machine with no panel is not a dead end — the form opens with
   the reason and with the app's own browser already listing folders — and a
   dismissal adds nothing and writes no error, because changing your mind is a
   decision rather than a failure. */
async function addWorkspaceFromPanel() {
  const button = $("workspace-add-primary");
  if (button) button.disabled = true;
  let picked;
  try {
    picked = await invoke("choose_folder", {
      title: tr("ws.add.pickTitle", "Choose a folder for this workspace"),
      start: null,
    });
  } catch (err) {
    openAddWorkspace();
    showInAppBrowser(String(err));
    await browseFolder(null);
    return;
  } finally {
    if (button) button.disabled = false;
  }
  if (!picked) return;
  await createWorkspaceAt(picked);
}

/* One pick, one workspace. A folder that is already a workspace is focused rather
   than added twice: with a panel, picking a folder you already have is one click
   away, and two records for one folder is a state nobody asked for. */
async function createWorkspaceAt(root) {
  const existing = state.workspaces.find((w) => w.root === root);
  if (existing) {
    toast(tr("ws.add.have", "Already a workspace") + ": " + existing.name);
    if (existing.id !== state.activeWorkspace) await switchWorkspace(existing.id);
    return;
  }
  // "No opinion" is what picking a folder expressed, so the new workspace
  // inherits today's installation; every field is on its own Edit form after.
  state.error = "";
  await workspaceCommand(
    invoke("create_workspace", { name: folderName(root), root, config: null }),
  );
  if (!state.error) {
    toast(tr("ws.add.done", "Workspace added") + ": " + (folderName(root) || root));
    return;
  }
  // Refused: the form opens with the folder still in it, so the next attempt is
  // one field away rather than one panel away.
  const reason = state.error;
  openAddWorkspace();
  adoptRoot(root);
  $("ws-add-err").textContent = reason;
}

/* One checkbox row, with the label the workspace will carry the name of. */
function addWorkspaceRow(host, cls, name, checked) {
  const row = el("label", "ws-tool-row");
  const box = el("input", cls);
  box.type = "checkbox";
  box.value = name;
  box.checked = !!checked;
  row.appendChild(box);
  row.appendChild(el("span", "", name));
  host.appendChild(row);
}

/* The MCP servers this installation knows, from the workspace list the sidebar
   already holds: a new workspace is seeded from the same servers, so the box
   starts where adoption would leave it — doing nothing changes nothing. */
function fillAddWorkspaceLists(caps) {
  const mcpHost = $("ws-add-mcp");
  const pluginHost = $("ws-add-plugins");
  const toolHost = $("ws-add-tools");
  const skillHost = $("ws-add-skills");
  if (!mcpHost) return;
  const active = state.workspaces.find((w) => w.id === state.activeWorkspace) || {};
  const servers = active.mcp_servers || [];
  mcpHost.textContent = "";
  if (!servers.length) {
    mcpHost.appendChild(el("div", "ws-form-hint", tr("ws.add.noMcp", "No MCP servers are configured yet")));
  }
  for (const server of servers) addWorkspaceRow(mcpHost, "ws-add-mcp-on", server.name, server.enabled);
  toolHost.textContent = "";
  for (const tool of BUILTIN_TOOLS) addWorkspaceRow(toolHost, "ws-add-tool-on", tool, true);
  if (pluginHost && caps) {
    pluginHost.textContent = "";
    if (!caps.plugins || !caps.plugins.length) {
      pluginHost.appendChild(el("div", "ws-form-hint", tr("ws.add.noPlugins", "No plugins are registered")));
    }
    for (const name of caps.plugins || []) addWorkspaceRow(pluginHost, "ws-add-plugin-on", name, true);
  }
  if (skillHost && caps) {
    skillHost.textContent = "";
    if (!caps.skills || !caps.skills.length) {
      skillHost.appendChild(el("div", "ws-form-hint", tr("ws.add.noSkills", "No skills are installed")));
    }
    // Skills arrive as records (`{name, description}`), plugins as bare names:
    // the row is named for the skill, never for its description.
    for (const skill of caps.skills || []) {
      addWorkspaceRow(skillHost, "ws-add-skill-on", skill.name, true);
    }
  }
}

async function refreshAddWorkspaceLists() {
  let caps = null;
  try {
    caps = await invoke("list_capabilities", { session: null });
  } catch (e) {
    // A capability list that will not load is not a reason to refuse to add a
    // workspace: the dialog still has the folder, the model and the MCP servers,
    // and the two sections say they are unknown rather than pretending.
    $("ws-add-plugins").textContent = "";
    $("ws-add-plugins").appendChild(el("div", "ws-form-hint", String(e)));
    $("ws-add-skills").textContent = "";
    $("ws-add-skills").appendChild(el("div", "ws-form-hint", String(e)));
    return;
  }
  if (!addWorkspaceOpen()) return;
  fillAddWorkspaceLists(caps);
}

/* What the dialog would send: the folder, the name, and the deny-lists the
   backend stores — a box that is ticked is a capability this workspace may use,
   so an unticked one is the name in the deny-list. */
function addWorkspaceDraft() {
  const unticked = (cls) =>
    [...document.querySelectorAll(`.${cls}`)]
      .filter((box) => !box.checked)
      .map((box) => box.value);
  return {
    model: $("ws-add-model").value.trim(),
    system_prompt: "",
    bash_enabled: $("ws-add-bash").checked,
    file_tools_enabled: $("ws-add-files").checked,
    require_approval: $("ws-add-approval").checked,
    disabled_tools: unticked("ws-add-tool-on"),
    disabled_skills: unticked("ws-add-skill-on"),
    disabled_plugins: unticked("ws-add-plugin-on"),
    mcp_enabled: [...document.querySelectorAll(".ws-add-mcp-on")]
      .filter((box) => box.checked)
      .map((box) => box.value),
  };
}

async function submitAddWorkspace() {
  const name = $("ws-add-name").value;
  const root = $("ws-add-root").value.trim();
  if (!root) {
    $("ws-add-err").textContent = tr("ws.add.root", "Choose a root folder for this workspace");
    $("ws-add-root").focus();
    return;
  }
  $("ws-add-err").textContent = "";
  $("ws-add-create").disabled = true;
  // The verdict is this attempt's, so the status is cleared first: a stale error
  // from something else entirely would otherwise keep the dialog open forever
  // and blame the folder for it.
  state.error = "";
  await workspaceCommand(
    invoke("create_workspace", { name, root, config: addWorkspaceDraft() }),
  );
  $("ws-add-create").disabled = false;
  // A command that failed leaves the dialog open with the reason *and* the form
  // still filled in, so the folder does not have to be chosen again; one that
  // succeeded closes.
  if (state.error) $("ws-add-err").textContent = state.error;
  else closeAddWorkspace();
}

/* The folder chooser. `list_folder` answers with sub-folders only and never
   reads a file's contents — it exists to point the app at a folder, so it
   deliberately looks outside the current workspace, where the file sidebar
   would refuse to go. */
/* The system's own folder panel, asked for through the backend. It is the door
   the user expects, and it is a modal: nothing here can be automated end to end,
   so the *decisions* are what this function owns — a dismissal changes nothing,
   and a panel this machine does not have falls back to the in-app browser with
   the reason shown rather than leaving the user with a dead button. */
async function chooseFolderNative() {
  const start = $("ws-add-root").value.trim();
  $("ws-add-browse").disabled = true;
  let picked;
  try {
    picked = await invoke("choose_folder", {
      title: tr("ws.add.pickTitle", "Choose a folder for this workspace"),
      start: start || null,
    });
  } catch (err) {
    // No panel here. The reason travels *into* the fallback, which is where the
    // user is about to look — writing it first and then revealing the browser
    // would overwrite it, which is a bug this guard caught.
    showInAppBrowser(String(err));
    await browseFolder(start || null);
    return;
  } finally {
    $("ws-add-browse").disabled = false;
  }
  if (!picked) return; // dismissed: the field keeps what it had
  adoptRoot(picked);
}

/* One field, two doors: the panel and the browser both end here, so the path is
   resolved once, the name is suggested once, and neither door can drift from the
   other. */
function adoptRoot(path) {
  $("ws-add-root").value = path;
  hideInAppBrowser();
  // The name starts empty and is optional, so the folder's own name is only
  // *suggested* — never forced — and only while the field is still untouched.
  if (!$("ws-add-name").value.trim()) {
    const parts = path.split("/").filter(Boolean);
    $("ws-add-name").value = parts.length ? parts[parts.length - 1] : "";
  }
  $("ws-add-err").textContent = "";
  $("ws-add-name").focus();
}

function showInAppBrowser(reason) {
  $("ws-add-browser").classList.remove("hidden");
  $("ws-add-browse-manual").setAttribute("aria-expanded", "true");
  $("ws-add-browse-hint").textContent = reason
    ? `${tr("ws.add.noPicker", "The system folder panel is unavailable")} — ${reason}`
    : tr("ws.add.browseHint", "Listing folders from this app instead.");
}

function hideInAppBrowser() {
  $("ws-add-browser").classList.add("hidden");
  $("ws-add-browse-manual").setAttribute("aria-expanded", "false");
}

async function browseFolder(path) {
  state.wsBrowse = { path: path || "", entries: [], parent: null, loading: true, error: "" };
  renderFolderBrowser();
  try {
    const listing = await invoke("list_folder", { path: path || null });
    state.wsBrowse = { ...listing, loading: false, error: "" };
  } catch (e) {
    state.wsBrowse = { path: path || "", entries: [], parent: null, loading: false, error: String(e) };
  }
  renderFolderBrowser();
}

function renderFolderBrowser() {
  const host = $("ws-browse-list");
  const browse = state.wsBrowse;
  if (!host || !browse) return;
  $("ws-browse-path").textContent = browse.loading ? "…" : browse.path || "";
  $("ws-browse-up").disabled = !browse.parent;
  host.textContent = "";
  if (browse.error) {
    $("ws-browse-note").textContent = browse.error;
    return;
  }
  for (const entry of browse.entries || []) {
    const row = el("button", "ws-browse-row", entry.name);
    row.type = "button";
    row.setAttribute("role", "option");
    row.dataset.path = entry.path;
    row.addEventListener("click", () => void browseFolder(entry.path));
    host.appendChild(row);
  }
  const notes = [];
  if (browse.truncated) notes.push(`showing the first ${(browse.entries || []).length}`);
  if (browse.unreadable) notes.push(`${browse.unreadable} not readable`);
  $("ws-browse-note").textContent = notes.join(" — ");
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
    // The last workspace has no successor to keep its chats in, so the form
    // offers no choice there and the payload says what will happen (§24.6).
    const last = (state.workspaces || []).length <= 1;
    return workspaceCommand(
      invoke("remove_workspace", { id, deleteSessions: last ? true : !!(del && del.checked) }),
    );
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
   so the sidebar re-groups from the server's answer rather than guessing.
   Moving the chat the reader is looking at carries them along: the transcript
   must never keep showing a chat that another workspace's agent would now run. */
async function moveSession(id, workspaceId) {
  const list = await invoke("move_session", { id, workspaceId });
  state.sessions = list;
  if (id === state.activeId && workspaceId !== state.activeWorkspace) {
    state.activeId = id;
    await switchWorkspace(workspaceId);
  }
  renderSidebar();
}

/* The form is rebuilt from `state.wsEdit`, so it survives a re-render (a stream
   tick, a search keystroke) without losing what the user is looking at. */
function workspaceFormEl(workspace) {
  const mode = state.wsEdit.mode;
  const form = el("div", "ws-form");
  if (mode === "remove") {
    form.appendChild(el("div", "ws-form-title", `Remove “${workspace.name}”?`));
    // "Keep them in a rescued workspace" needs a rescued workspace. The last one
    // has none, so the choice is not offered — it is stated, because a silent
    // delete is the one outcome §24.6 rules out (§24.6).
    if ((state.workspaces || []).length <= 1) {
      form.appendChild(
        el(
          "div",
          "ws-form-hint",
          tr(
            "ws.remove.last",
            "This is the last workspace: its chats go with it, because there is no workspace left to keep them in.",
          ),
        ),
      );
    } else {
      const label = el("label", "ws-form-row");
      const del = el("input", "ws-del-chats");
      del.type = "checkbox";
      label.appendChild(del);
      label.appendChild(el("span", "", "delete its chats too"));
      form.appendChild(label);
    }
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

/* ---------- the row menu ------------------------------------------------
   One popover serves every row: it is anchored to the button that opened it,
   closes on Escape or on a click outside, and hands focus back to that button
   so the keyboard never lands nowhere. Only one can be open at a time, and the
   anchor is the only element that knows it is expanded. */
let rowMenu = null;

function closeRowMenu(refocus) {
  if (!rowMenu) return false;
  const { menu, anchor } = rowMenu;
  rowMenu = null;
  menu.remove();
  anchor.setAttribute("aria-expanded", "false");
  if (refocus) anchor.focus();
  return true;
}

function openRowMenu(anchor, items, onPick) {
  closeRowMenu(false);
  const menu = el("div", "row-menu");
  menu.setAttribute("role", "menu");
  menu.setAttribute("aria-label", anchor.getAttribute("aria-label") || "Actions");
  let heading = null;
  for (const item of items) {
    // A heading is a caption for the choices under it, not a choice: it is
    // skipped by the keyboard walk, which only ever visits `.row-menu-item`.
    if (item.group && item.group !== heading) {
      heading = item.group;
      const head = el("div", "row-menu-head", heading);
      head.setAttribute("role", "presentation");
      menu.appendChild(head);
    }
    const b = el("button", "row-menu-item" + (item.danger ? " danger" : ""), item.label);
    b.type = "button";
    // A checked item is one of a set: role=menuitemradio plus aria-checked is
    // what tells a screen reader that picking it un-picks its siblings. The tick
    // is decoration on top of that, not the state itself.
    b.setAttribute("role", item.checked === undefined ? "menuitem" : "menuitemradio");
    if (item.checked !== undefined) b.setAttribute("aria-checked", item.checked ? "true" : "false");
    b.dataset.run = item.run;
    if (item.accel) b.appendChild(el("span", "row-menu-key", item.accel));
    if (item.checked) b.appendChild(el("span", "row-menu-tick", "✓"));
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      if (closeRowMenu(true)) onPick(item);
    });
    menu.appendChild(b);
  }
  menu.addEventListener("keydown", (e) => {
    const items = [...menu.querySelectorAll(".row-menu-item")];
    const at = items.indexOf(document.activeElement);
    if (e.key === "Escape") {
      e.preventDefault();
      // One Escape closes one thing: without this the document-level handler
      // would find no menu (this one just closed it) and close the dialog behind.
      e.stopPropagation();
      closeRowMenu(true);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      const next = at < 0 ? 0 : (at + step + items.length) % items.length;
      items[next].focus();
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      items[e.key === "Home" ? 0 : items.length - 1].focus();
    }
  });
  // Positioned after insertion so the menu's own height is known: a row near
  // the bottom opens upward rather than off the screen.
  document.body.appendChild(menu);
  const box = anchor.getBoundingClientRect();
  const size = menu.getBoundingClientRect();
  const left = Math.min(
    Math.max(6, box.left),
    Math.max(6, document.documentElement.clientWidth - size.width - 6),
  );
  const below = box.bottom + 4;
  const top = below + size.height > window.innerHeight - 6 ? Math.max(6, box.top - size.height - 4) : below;
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(top)}px`;
  anchor.setAttribute("aria-expanded", "true");
  rowMenu = { menu, anchor };
  const first = menu.querySelector(".row-menu-item");
  if (first) first.focus();
  return menu;
}

/* The head's sliders menu: how the sidebar draws the chats. Each item is a radio
   rather than a command — the current choice is ticked and announced as checked,
   because there is no other place the state is written — and picking one redraws
   the list underneath with the menu still open, since the three choices are
   usually set together. "Reload chats" is the one item that acts instead of
   choosing: it is the reload button the head used to carry. */
function sessionViewMenu(anchor) {
  const view = state.view;
  const archived = archivedSessions(state.sessions).length;
  const count = archived ? ` (${archived})` : "";
  openRowMenu(
    anchor,
    [
      // Every label goes through `tr` like the row menus' do; the two counts are
      // appended *after* the translation, so a translator never sees the number.
      {
        group: tr("view.groupBy", "Group by"),
        label: tr("view.groupWorkspace", "WorkSpace"),
        run: "group:workspace",
        checked: view.group === "workspace",
      },
      {
        group: tr("view.groupBy", "Group by"),
        label: tr("view.groupTree", "Workspace Tree"),
        run: "group:tree",
        checked: view.group === "tree",
      },
      {
        group: tr("view.groupBy", "Group by"),
        label: tr("view.groupList", "In one list"),
        run: "group:list",
        checked: view.group === "list",
      },
      {
        group: tr("view.orderBy", "Order by"),
        label: tr("view.orderManual", "Manual"),
        run: "order:manual",
        checked: view.order === "manual",
      },
      {
        group: tr("view.orderBy", "Order by"),
        label: tr("view.orderUpdated", "Last updated"),
        run: "order:updated",
        checked: view.order === "updated",
      },
      {
        group: tr("view.filterSessions", "Filter sessions"),
        label: tr("view.filterHide", "Hide archived") + count,
        run: "filter:hide",
        checked: view.filter === "hide",
      },
      {
        group: tr("view.filterSessions", "Filter sessions"),
        label: tr("view.filterAll", "All conversations (show archived)"),
        run: "filter:all",
        checked: view.filter === "all",
      },
      {
        group: tr("view.filterSessions", "Filter sessions"),
        label: tr("view.filterArchived", "Archived only") + count,
        run: "filter:archived",
        checked: view.filter === "archived",
      },
      { group: tr("view.chats", "Chats"), label: tr("view.reload", "Reload chats"), run: "reload" },
    ],
    (item) => {
      const [key, value] = String(item.run).split(":");
      if (key === "reload") return void reloadSessionList().catch(() => {});
      setSessionView({ [key]: value });
      sessionViewMenu(anchor);
    },
  );
}

/* An archived chat's row, drawn wherever the filter says it belongs: beside its
   own workspace's chats, or alone in the archived-only list. It carries no pin
   and no workspace picker — an archived chat is a target for neither — and both
   the row and its "…" menu lead back to the live list. */
function archivedRow(s) {
  const row = el("div", "session-row archived-row");
  row.dataset.id = s.id;
  row.appendChild(el("span", "sess-mark"));
  row.appendChild(el("span", "title", clip(s.title, 24)));
  const when = el("span", "time", fmtAge(s.updated_at));
  when.title = `archived ${fmtTime(s.updated_at)}`;
  row.appendChild(when);
  const back = el("button", "sess-restore", tr("session.restore", "Restore"));
  back.type = "button";
  back.setAttribute("aria-label", `Restore ${s.title || "New Session"}`);
  back.addEventListener("click", (e) => {
    e.stopPropagation();
    void setArchived(s.id, false);
  });
  row.appendChild(back);
  const more = el("button", "sess-menu", "…");
  more.type = "button";
  more.setAttribute("aria-label", `Actions for ${s.title || "New Session"}`);
  more.setAttribute("aria-haspopup", "menu");
  more.setAttribute("aria-expanded", "false");
  more.addEventListener("click", (e) => {
    e.stopPropagation();
    if (rowMenu && rowMenu.anchor === more) return void closeRowMenu(true);
    openRowMenu(more, sessionMenu(s), (item) => runSessionAction(s, item.run));
  });
  row.appendChild(more);
  row.addEventListener("click", () => void setArchived(s.id, false));
  return row;
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
  const view = state.view;
  const groups = groupSessions(visible, state.workspaces, state.activeWorkspace, view);
  // "In one list" is the one mode with no workspace rows at all, so the whole
  // list is ordered in one pass and every row names the workspace it runs in —
  // without that name the mode would drop the fact the grouping was carrying.
  const flat = view.group === "list";
  const drawn = flat
    ? [
        {
          id: "",
          name: "",
          root: "",
          model: "",
          sessions: orderSessions(groups.flatMap((g) => g.sessions), view),
        },
      ]
    : groups;
  const wsNames = new Map(state.workspaces.map((w) => [w.id, w.name || w.root || w.id]));
  for (const group of drawn) {
    // Declared before the header's own block because the `continue` below reads
    // it from outside that block (the flat list has no headers at all).
    let folded = false;
    if (!flat) {
      // The header is the workspace switcher's anchor: the root on hover, and the
      // two gestures the reference sidebar puts on a row. They sit on the right,
      // where the bound model used to be — the row is about the workspace, and the
      // model of the chat in use is the status line's job (§24.4) — so the name
      // keeps the left edge to itself.
      const head = el("div", "ws-head");
      head.title = group.root
        ? `${group.root} — click to switch`
        : `${group.name} — click to switch`;
      if (group.id === state.activeWorkspace) {
        head.classList.add("active");
        // The dot is decorative — the status is in the class, and this is how a
        // screen reader hears it. It is an element and not a `::before` so a guard
        // can see it: a surface drawn only by CSS is a surface nothing is true of.
        head.setAttribute("aria-current", "true");
      }
      head.addEventListener("click", () => switchWorkspace(group.id));
      // "WorkSpace" keeps every workspace as a row and opens only the active one,
      // so switching workspace is what opens a group; a fold the person set by
      // hand still wins inside the group it was set on.
      // The row leads with the workspace's own folder, the way the reference sidebar
      // draws it. The disclosure takes that slot while the row is hovered (the class
      // below is what lets the stylesheet do that without guessing), so a nested list
      // is still openable without a second column of glyphs (§24.4).
      const canFold = view.group !== "workspace" || group.id === state.activeWorkspace;
      folded =
        !!state.collapsed[group.id] ||
        (view.group === "workspace" && group.id !== state.activeWorkspace);
      if (canFold) {
        const fold = el("button", "ws-act ws-fold");
        fold.appendChild(ico(folded ? ICONS.triRight : ICONS.triDown, true));
        // The icon is the whole label, so it needs a name of its own — the title is
        // a tooltip, and a screen reader does not read one as a name.
        fold.setAttribute("aria-label", folded ? `Show ${group.name}'s chats` : `Hide ${group.name}'s chats`);
        fold.setAttribute("aria-expanded", folded ? "false" : "true");
        fold.title = folded ? "Show this workspace's chats" : "Hide them";
        fold.addEventListener("click", (e) => {
          // Otherwise the click would also switch the active workspace, which is the
          // header's job, not the fold's.
          e.stopPropagation();
          state.collapsed[group.id] = !folded;
          renderSidebar();
        });
        head.appendChild(fold);
      } else {
        // A row that cannot fold keeps the column, so every name starts at the same x:
        // an unfolderable row is aligned with the foldable ones, not shifted left.
        head.appendChild(el("span", "ws-fold-gap"));
      }
      // Then the workspace's own folder, the way the reference sidebar draws it.
      const folder = el("span", "ws-folder");
      folder.appendChild(ico(ICONS.folder));
      head.appendChild(folder);
      // A chat inside is streaming: the folder takes the success colour, so the
      // workspace that owns the work is visible with its group still closed.
      if (group.sessions.some((s) => streamOf(s.id))) head.classList.add("working");
      head.appendChild(el("span", "ws-name", clip(group.name, 18)));
      // The two gestures the reference sidebar puts on a row, in its order: the menu
      // first, and the row's own "start a chat here" (the ⊕) last, nearest the edge.
      // Rename, set-root and the remove form are unchanged; they are reached from the
      // menu, so the header is not a row of glyphs whose meaning has to be guessed.
      const more = el("button", "ws-act ws-menu");
      more.appendChild(ico(ICONS.dots));
      more.setAttribute("aria-label", `${group.name} actions`);
      more.setAttribute("aria-haspopup", "menu");
      more.setAttribute("aria-expanded", "false");
      more.title = tr("ws.actions", "Workspace actions");
      more.addEventListener("click", (e) => {
        e.stopPropagation();
        if (rowMenu && rowMenu.anchor === more) return void closeRowMenu(true);
        openRowMenu(more, workspaceMenu(group), (item) => {
          if (item.run === "new") void newSessionIn(group.id);
          else if (item.run === "rename") openWorkspaceForm("edit", group.id);
          else if (item.run === "delete") openWorkspaceForm("remove", group.id);
        });
      });
      head.appendChild(more);
      const add = el("button", "ws-act ws-new");
      add.appendChild(ico(ICONS.plusCircle));
      add.setAttribute("aria-label", `New session in ${group.name}`);
      add.title = `${tr("ws.new", "New session")} (Cmd/Ctrl+N)`;
      add.setAttribute("aria-keyshortcuts", "Meta+N Control+N");
      add.addEventListener("click", (e) => {
        e.stopPropagation();
        void newSessionIn(group.id);
      });
      head.appendChild(add);

      if (group.id === state.activeWorkspace) {
        const dot = el("span", "ws-dot", "●");
        dot.setAttribute("aria-hidden", "true");
        dot.title = tr("ws.active", "Active workspace");
        head.appendChild(dot);
      }
      head.appendChild(el("span", "ws-count", String(group.sessions.length)));
      sessionList.appendChild(head);
    }
    // A collapsed group still counts its chats in the header; it just hides them.
    if (!flat && folded) continue;
    for (const s of group.sessions) {
    if (s.archived) {
      sessionList.appendChild(archivedRow(s));
      continue;
    }
    const row = el("div", "session-row" + (s.id === state.activeId ? " active" : ""));
    // The row names the chat it stands for, so a check can address one chat
    // instead of matching its clipped title.
    row.dataset.id = s.id;
    // One marker per chat: hollow while idle, filled while it is streaming. The
    // ring is decoration, so the state is also spelled out for a screen reader
    // instead of being carried by colour alone.
    const live = !!streamOf(s.id);
    // The row carries the state as well as its marker: a streaming chat's title takes
    // the success colour, so the list says which chat is working without a hover.
    if (live) row.classList.add("working");
    const mark = el("span", "sess-mark" + (live ? " live" : ""));
    mark.setAttribute("aria-hidden", "true");
    row.appendChild(mark);
    row.appendChild(el("span", "title", clip(s.title, 24)));
    // In one list the row has no workspace header above it, so it carries the
    // workspace itself: a chip, not a second name in the title.
    if (flat) {
      row.appendChild(
        el("span", "sess-ws-chip", clip(wsNames.get(s.workspace) || s.workspace || "Default", 14)),
      );
    }
    if (live) row.appendChild(el("span", "vh", "streaming"));
    // The star is the pin's own control, so the state is one click from the row
    // instead of two: it reads as pressed to a screen reader and stays visible
    // when the row is not hovered, which a hover-only affordance never can.
    const pin = el("button", "sess-pin" + (s.pinned ? " on" : ""), s.pinned ? "★" : "☆");
    pin.type = "button";
    pin.setAttribute("aria-pressed", s.pinned ? "true" : "false");
    pin.setAttribute("aria-label", s.pinned ? "Unpin this chat" : "Pin this chat");
    pin.title = tr("session.pin", s.pinned ? "Unpin session" : "Pin session");
    pin.addEventListener("click", (e) => {
      e.stopPropagation();
      void setPinned(s.id, !s.pinned);
    });
    row.appendChild(pin);
    const age = el("span", "time", fmtAge(s.updated_at));
    age.title = fmtTime(s.updated_at);
    row.appendChild(age);
    const more = el("button", "sess-menu", "…");
    more.type = "button";
    more.setAttribute("aria-label", `Actions for ${s.title || "New chat"}`);
    more.setAttribute("aria-haspopup", "menu");
    more.setAttribute("aria-expanded", "false");
    more.title = tr("session.actions", "Chat actions");
    more.addEventListener("click", (e) => {
      e.stopPropagation();
      if (rowMenu && rowMenu.anchor === more) return void closeRowMenu(true);
      openRowMenu(more, sessionMenu(s), (item) => runSessionAction(s, item.run));
    });
    row.appendChild(more);
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
  // No workspace at all is a state with a face: nothing can run, no chat has a
  // home, and the one gesture that changes that is the folder-plus this panel
  // already has (§24.6).
  if (!state.workspaces.length) {
    const empty = el("div", "ws-empty");
    empty.appendChild(
      el(
        "div",
        "ws-empty-hint",
        tr("ws.none.hint", "A workspace is a folder. Add one to work in it."),
      ),
    );
    const add = el("button", "ws-empty-add", tr("btn.addWorkspace", "Add a workspace"));
    add.type = "button";
    add.addEventListener("click", () => void addWorkspaceFromPanel());
    empty.appendChild(add);
    sessionList.appendChild(empty);
  }
  if (state.wsEdit) {
    const known = state.workspaces.find((w) => w.id === state.wsEdit.id);
    sessionList.appendChild(
      workspaceFormEl(known || { id: state.wsEdit.id, name: state.wsEdit.id, root: "" }),
    );
  } else {
    // The row that used to sit here ("+ New workspace") is gone: the folder-plus
    // in this section's header is the one door to the Add-workspace dialog, and a
    // second one under the list was a duplicate 40 px from the first — the same
    // shape as the two folder-plus glyphs rev 111 removed from the brand row.
    // Presets stay: applying one is a way of creating a workspace — a copy of it,
    // never a link to it (§24.9) — and it is not a second way to that dialog.
    for (const preset of state.presets) {
      const from = el("button", "ws-preset-row", `▸ ${preset.name || preset.id}`);
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

/* Message actions are built from the message alone, so a stream ending cannot
   change any row's shape. They used to be gated on this chat being idle, which
   meant the idle flip at the end of every turn rewrote the whole drawn window
   (200 rebuilt rows, ~400 DOM mutations, measured). The gate moves to the click
   and to `#messages.busy` in CSS: rendered, hover-revealed, inert while busy. */
function actionsReady() {
  return !streamOf(state.activeId) && !state.compacting[state.activeId];
}

function buildBubble(m, showCaret, opts) {
  const bubble = el("div", "bubble");
  if (m.reasoning) bubble.appendChild(el("div", "reasoning", m.reasoning));
  for (const t of m.tools || []) {
    bubble.appendChild(buildToolCard(t));
  }
  // Attachments render above the text, thumbnail-sized; a click enlarges the
  // image in place (the class toggles, so no navigation and no new window).
  for (const url of m.images || []) {
    const img = document.createElement("img");
    img.className = "msg-img";
    img.src = url;
    img.alt = "attached image";
    img.loading = "lazy";
    img.addEventListener("click", () => img.classList.toggle("zoomed"));
    bubble.appendChild(img);
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
  } else if (
    !m.reasoning &&
    !(m.tools || []).length &&
    !(m.images || []).length &&
    !m.error
  ) {
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
    regenBtn.addEventListener("click", () => {
      if (actionsReady()) regenerate();
    });
    acts.appendChild(regenBtn);
    hasActs = true;
  }
  if (opts && opts.edit) {
    const editBtn = el("button", "mini-btn", "Edit");
    editBtn.type = "button";
    editBtn.addEventListener("click", () => {
      if (!actionsReady()) return;
      state.editing = { sessionId: state.activeId, index: opts.index };
      renderMessages();
    });
    acts.appendChild(editBtn);
    hasActs = true;
  }
  if (opts && opts.del) {
    const delBtn = el("button", "mini-btn", "Delete");
    delBtn.type = "button";
    delBtn.addEventListener("click", () => {
      if (actionsReady()) deleteFrom(opts.index);
    });
    acts.appendChild(delBtn);
    hasActs = true;
  }
  if (opts && opts.branch) {
    const branchBtn = el("button", "mini-btn", "Branch");
    branchBtn.type = "button";
    branchBtn.title = "Fork this chat at this message into a new session";
    branchBtn.addEventListener("click", () => {
      if (actionsReady()) branchFrom(opts.branchIndex);
    });
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
  const msgs = state.cache[state.activeId] || [];
  const original = msgs[index];
  const images = original && original.role === "User" ? original.images || [] : [];
  state.editing = null;
  sendText(text, state.activeId, index, images).catch(() => {});
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
    await refreshGoal(fork.id);
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
  // The attachments are shown, not edited: a resend keeps them (the edited
  // turn re-asks about the same pictures with new words).
  for (const url of m.images || []) {
    const img = document.createElement("img");
    img.className = "msg-img";
    img.src = url;
    img.alt = "attached image";
    box.appendChild(img);
  }
  const row = el("div", "bubble-actions");
  const save = el("button", "mini-btn", "Send");
  save.type = "button";
  save.addEventListener("click", () => {
    const v = ta.value.trim();
    if (!v && !(m.images || []).length) return;
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
/* A finished turn re-reads the transcript from the store, which hands back fresh
   objects for messages we already hold. Pairing the drawn tail by content keeps
   those objects — and the DOM nodes drawn for them — so a turn stops rewriting
   the whole window (measured: ~400 DOM mutations per turn before this, ~7 after).

   The pairing must be exact. The renderer's own signature is deliberately lossy
   (`length` + the last 24 characters), which is enough to notice a streamed token
   but not a rewrite of the same length and tail — so a row kept for a message
   whose text actually changed would never update, which is precisely what a
   backend finalizing the reply does. And only the drawn tail is paired: the rows
   outside it are not drawn, so their identity buys nothing and scanning the whole
   session on every turn would not scale. */
function sameMessage(a, b) {
  if (a === b) return true;
  if (!a || !b || a.role !== b.role || a.text !== b.text) return false;
  if ((a.reasoning || "") !== (b.reasoning || "")) return false;
  if ((a.error || "") !== (b.error || "")) return false;
  const ia = a.images || [];
  const ib = b.images || [];
  if (ia.length !== ib.length) return false;
  for (let k = 0; k < ia.length; k += 1) {
    if (ia[k] !== ib[k]) return false;
  }
  const ta = a.tools || [];
  const tb = b.tools || [];
  if (ta.length !== tb.length) return false;
  for (let i = 0; i < ta.length; i += 1) {
    const x = ta[i];
    const y = tb[i];
    if (
      x.name !== y.name ||
      x.args !== y.args ||
      (x.output == null ? null : x.output) !== (y.output == null ? null : y.output) ||
      (x.ms == null ? null : x.ms) !== (y.ms == null ? null : y.ms) ||
      !!x.open !== !!y.open
    ) {
      return false;
    }
  }
  return true;
}

function reuseIdentical(oldList, fresh) {
  if (!Array.isArray(oldList) || !Array.isArray(fresh)) return fresh;
  const span = Math.min(WINDOW + (state.windowExtra || 0) + 8, oldList.length, fresh.length);
  for (let k = 1; k <= span; k += 1) {
    const i = fresh.length - k;
    const j = oldList.length - k;
    if (sameMessage(oldList[j], fresh[i])) fresh[i] = oldList[j];
  }
  return fresh;
}

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
  let images = "";
  for (const url of m.images || []) images += `${sigOf(url)};`;
  return [
    m.role,
    sigOf(m.text),
    sigOf(m.reasoning),
    sigOf(m.error),
    images,
    tools,
    opts.showCaret ? 1 : 0,
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

/* ---------- center tabs: Chat | Trajectory (M10 step 1) ----------
   The reference gives the center column a tab strip. Chat is the transcript
   that already exists; Trajectory reads the same message cache as steps — one
   row per turn with its tool calls, durations and errors — so the seat needs
   no backend and no second store. The tab choice is device-local UI state: it
   follows the window, not the session. */
const TRAJ_MAX = 300;
let trajKey = null;

function buildTrajTool(t) {
  const row = el("div", "traj-tool");
  row.appendChild(el("span", "traj-tool-name", t.name));
  row.appendChild(el("span", "traj-tool-args", toolArgsSummary(t.args)));
  const done = t.output != null;
  // The badge carries the state in text first — color is decoration, never
  // the signal (the chat's own badges say the same two words).
  row.appendChild(
    el("span", "traj-badge" + (done ? " done" : " running"),
      done ? (t.ms != null ? `${t.ms} ms` : "done") : "running…")
  );
  return row;
}

function buildTrajStep(m, index) {
  const user = m.role === "User";
  const step = el("div", "traj-step" + (user ? " traj-user" : ""));
  const head = el("div", "traj-head");
  head.appendChild(el("span", "traj-idx", String(index + 1)));
  head.appendChild(el("span", "traj-role", user ? "User" : "Assistant"));
  const tools = m.tools || [];
  if (tools.length) {
    head.appendChild(el("span", "traj-count", `${tools.length} tool${tools.length === 1 ? "" : "s"}`));
  }
  step.appendChild(head);
  const text = (m.text || "").replace(/\s+/g, " ").trim();
  if (text) {
    step.appendChild(el("div", "traj-text", text.length > 240 ? `${text.slice(0, 240)}…` : text));
  } else if (!tools.length && !m.error && !m.reasoning) {
    step.appendChild(el("div", "traj-text traj-muted", user ? "" : "…"));
  }
  if (m.reasoning) step.appendChild(el("div", "traj-meta", "reasoning shown in Chat"));
  if (m.error) step.appendChild(el("div", "traj-err", `⚠ ${m.error}`));
  for (const t of tools) step.appendChild(buildTrajTool(t));
  return step;
}

function renderTrajectory() {
  const host = $("trajectory");
  if (!host) return;
  const msgs = activeMsgs();
  let tools = 0;
  let errs = 0;
  for (const m of msgs) {
    tools += (m.tools || []).length;
    if (m.error) errs += 1;
  }
  const last = msgs[msgs.length - 1];
  // Rebuild only on a structural change: the tail preview refreshes every
  // 256 characters of streamed text instead of on every chunk, so watching a
  // long stream costs one rebuild per turn, not one per frame.
  const key = [state.activeId, msgs.length, tools, errs,
    Math.floor(((last && last.text) || "").length / 256)].join("|");
  if (key === trajKey) return;
  const atTail = host.scrollHeight - host.scrollTop - host.clientHeight < 8;
  trajKey = key;
  host.textContent = "";
  if (msgs.length === 0) {
    const empty = el("div", "traj-empty");
    empty.appendChild(el("div", "", "No turns yet"));
    empty.appendChild(el("div", "traj-muted", "Steps appear here as the conversation runs."));
    host.appendChild(empty);
    return;
  }
  const start = Math.max(0, msgs.length - TRAJ_MAX);
  if (start > 0) {
    host.appendChild(
      el("div", "traj-note", `Showing the last ${TRAJ_MAX} of ${msgs.length} steps.`)
    );
  }
  for (let i = start; i < msgs.length; i++) host.appendChild(buildTrajStep(msgs[i], i));
  if (atTail) host.scrollTop = host.scrollHeight;
}

function setCenterTab(name) {
  if (name !== "chat" && name !== "trajectory") return;
  state.centerTab = name;
  for (const [key, id] of [["chat", "tab-chat"], ["trajectory", "tab-trajectory"]]) {
    const t = $(id);
    if (!t) continue;
    const on = key === name;
    t.setAttribute("aria-selected", on ? "true" : "false");
    t.tabIndex = on ? 0 : -1;
    t.classList.toggle("active", on);
  }
  for (const [id, on] of [["messages", name === "chat"], ["trajectory", name === "trajectory"]]) {
    const pane = $(id);
    if (!pane) continue;
    pane.hidden = !on;
    pane.classList.toggle("hidden", !on);
  }
  if (name === "trajectory") renderTrajectory();
}

function bindCenterTabs() {
  const list = $("center-tabs");
  if (!list) return;
  const tabs = [$("tab-chat"), $("tab-trajectory")].filter(Boolean);
  for (const t of tabs) {
    t.addEventListener("click", () =>
      setCenterTab(t.id === "tab-chat" ? "chat" : "trajectory"));
  }
  // Roving tabindex: one tab stop for the strip, arrows move and activate
  // (automatic activation — with two views, selection and focus stay together).
  list.addEventListener("keydown", (e) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
    e.preventDefault();
    const i = Math.max(0, tabs.findIndex((t) => t.getAttribute("aria-selected") === "true"));
    let next = i;
    if (e.key === "ArrowLeft") next = (i - 1 + tabs.length) % tabs.length;
    if (e.key === "ArrowRight") next = (i + 1) % tabs.length;
    if (e.key === "Home") next = 0;
    if (e.key === "End") next = tabs.length - 1;
    tabs[next].focus();
    setCenterTab(tabs[next].id === "tab-chat" ? "chat" : "trajectory");
  });
}
bindCenterTabs();

function renderMessages() {
  growDrawn = true; // the previous growth, if any, has now been drawn
  // The trajectory reads the same cache, so every transcript repaint — stream
  // chunk, splice, switch, restore — refreshes it while that tab is the one
  // on screen.
  if (state.centerTab === "trajectory") renderTrajectory();
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

  // Whether the row actions are live is a property of the transcript, not of
  // each row: one class write here replaces a rebuild of every drawn row.
  messagesEl.classList.toggle("busy", !idle);

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
      // The branch keeps the first `index` messages, so "branch at this message"
      // asks for one more than its 0-based number. The double used to add the
      // one itself, which hid this: against the real backend the branch came out
      // one message short — without the message it was taken from.
      branchIndex: abs + 1,
      regen: abs === lastAssistant && (!!m.text || (m.images || []).length),
      edit: m.role === "User" && (!!m.text || (m.images || []).length),
      del: true,
      branch: abs > 0,
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
   backwards off the start of a long chat needs no button.

   A gesture fires dozens of scroll events per frame, and the first version grew
   the window per *event*: one flick added the whole burst (measured in the ui
   fixture: 50 events took the drawn transcript from 200 rows to all 1 000), so
   the window cap held only until the first scroll-up. The gate below grants one
   growth per drawn frame — `renderMessages` re-opens it — which is also what
   makes the gesture self-limiting: the growth prepends rows above the reader, the
   render restores their offset, and `scrollTop` is then ~200 rows below the top
   again, so the next growth needs another deliberate scroll. */
let growDrawn = true;
function growWindow() {
  if (!growDrawn) return;
  growDrawn = false;
  const total = (state.cache[state.activeId] || []).length;
  state.windowExtra = Math.min((state.windowExtra || 0) + WINDOW, total);
  markDirtyAll();
  scheduleRepaint();
}
function resetWindow() {
  state.windowExtra = 0;
  growDrawn = true;
}
function ensureScrollGrow() {
  if (scrollGrowBound || !messagesEl) return;
  scrollGrowBound = true;
  messagesEl.addEventListener("scroll", () => {
    if (messagesEl.scrollTop > 320 || lastWindowStart <= 0) return;
    growWindow();
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
      growWindow();
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
  statusLeft.textContent = "";
  statusLeft.appendChild(el("span", "", state.model || "no model"));
  statusLeft.appendChild(
    el("span", "", `${state.sessions.length} chat${state.sessions.length === 1 ? "" : "s"}`)
  );
  renderSessionStats(statusLeft, state.overview);

  statusRight.textContent = "";
  renderArchived(statusRight, state.archived, () => void openArchivedView(), restoreArchived);
  // After *both* clears, not just this one: `renderUsage` appends its meter to
  // this very element, and `renderArchived` clears the host it is handed too, so
  // a meter written between the two clears is built and thrown away on every
  // frame — which is what happened during a stream. Two writers and one element
  // is a lost update; the fixture's direct `renderUsage()` call could not see it,
  // so its check now goes through this pass.
  renderUsage();
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
    sendBtn.disabled = !input.value.trim() && !state.pendingImages.length;
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
  q.forEach((entry, i) => {
    const text = typeof entry === "string" ? entry : entry.text || "";
    const imageCount = typeof entry === "string" ? 0 : (entry.images || []).length;
    const row = document.createElement("div");
    row.className = "queue-item";
    const label = document.createElement("span");
    label.className = "queue-text";
    label.textContent = `queued · ${imageCount ? `[${imageCount} image${imageCount === 1 ? "" : "s"}] ` : ""}${text}`;
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
  const list = $("plan-list");
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
  const count = $("plan-count");
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
    // The status class carries the strikethrough and the accent; without it
    // every row in the editor looks pending, so a done step reads as untouched.
    li.className = "plan-item plan-edit-row status-" + it.status;
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

  const count = $("plan-count");
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

/* ---------- goal surface (M3) ------------------------------------------ */

/* The plan answers "what steps are left"; the goal answers "what is all this
   for", and it outlives any single turn. The reference layout keeps it as a
   card right above the composer — visible while chatting, not behind a panel
   switch — so the round counter and the state read at a glance. */

const GOAL_STATES = {
  active: { label: "active", icon: "◎" },
  paused: { label: "paused", icon: "‖" },
  completed: { label: "completed", icon: "✓" },
  blocked: { label: "blocked", icon: "⛔" },
};

/* Read the card's nodes on demand: the markup is static in index.html, so
   there is nothing to build and nothing to leak. */
function goalRefs() {
  return {
    card: $("goal-card"),
    icon: $("goal-icon"),
    state: $("goal-state"),
    rounds: $("goal-rounds"),
    objective: $("goal-objective"),
    blocked: $("goal-blocked"),
    error: $("goal-error"),
    form: $("goal-form"),
    input: $("goal-input"),
    max: $("goal-max"),
    blockForm: $("goal-block-form"),
    reason: $("goal-reason"),
    advance: $("goal-advance"),
    pause: $("goal-pause"),
    edit: $("goal-edit"),
    block: $("goal-block"),
    complete: $("goal-complete"),
    remove: $("goal-delete"),
    save: $("goal-save"),
    clear: $("goal-clear"),
    cancel: $("goal-cancel"),
  };
}

function goalFail(msg) {
  const r = goalRefs();
  if (!r.error) return;
  r.error.textContent = msg || "";
  r.error.classList.toggle("hidden", !msg);
}

/* Paint the whole card from one goal value (or from `goalOpen` when there is
   no goal yet and the reader asked to set one). */
function renderGoal(goal) {
  const r = goalRefs();
  if (!r.card) return;
  const g = goal || null;
  const meta = g ? GOAL_STATES[g.state] || GOAL_STATES.active : null;
  r.card.classList.toggle("hidden", !g && !state.goalOpen);
  goalFail("");
  if (!g) {
    r.icon.textContent = "◎";
    r.state.textContent = "no goal";
    r.state.dataset.state = "none";
    r.rounds.textContent = "";
    r.objective.textContent = "No goal yet — say what this work is for.";
    r.objective.title = "";
    r.objective.classList.add("dim");
    r.blocked.classList.add("hidden");
    for (const b of [r.pause, r.block, r.complete, r.remove]) {
      if (b) b.disabled = true;
    }
    if (r.advance) r.advance.disabled = true;
    if (r.edit) r.edit.textContent = "✎";
    return;
  }
  r.icon.textContent = meta.icon;
  r.state.textContent = meta.label;
  r.state.dataset.state = g.state;
  r.rounds.textContent = g.max_rounds != null
    ? `${g.rounds} of ${g.max_rounds} rounds${g.rounds >= g.max_rounds ? " · cap" : ""}`
    : `${g.rounds} round${g.rounds === 1 ? "" : "s"}`;
  r.objective.textContent = g.objective;
  r.objective.title = g.objective;
  r.objective.classList.remove("dim");
  if (g.state === "blocked" && g.blocked_reason) {
    r.blocked.textContent = `Blocked: ${g.blocked_reason}`;
    r.blocked.classList.remove("hidden");
  } else {
    r.blocked.classList.add("hidden");
  }
  if (r.advance) {
    r.advance.disabled = !(g.state === "active" && (g.max_rounds == null || g.rounds < g.max_rounds));
  }
  if (r.pause) r.pause.disabled = g.state !== "active" && g.state !== "paused";
  if (r.pause) r.pause.textContent = g.state === "paused" ? "▶" : "⏸";
  if (r.pause) r.pause.title = g.state === "paused" ? "Resume the goal" : "Pause the goal";
  if (r.block) r.block.disabled = g.state === "completed" || g.state === "blocked";
  if (r.complete) r.complete.disabled = g.state === "completed" || g.state === "blocked";
  if (r.remove) r.remove.disabled = false;
}

function goalShowForm(on) {
  const r = goalRefs();
  r.form.classList.toggle("hidden", !on);
  if (on) {
    r.input.value = state.goal ? state.goal.objective : "";
    r.max.value = state.goal && state.goal.max_rounds != null ? String(state.goal.max_rounds) : "";
    r.blockForm.classList.add("hidden");
    r.input.focus();
  }
}

/* Open the card in the chat column; with no goal yet the editor opens too, so
   the next thing to do is obvious. */
function openGoal() {
  state.goalOpen = true;
  renderGoal(state.goal);
  if (!state.goal) goalShowForm(true);
}

/* The goal of the active session, or `id`'s when switching to it. */
async function refreshGoal(id) {
  const target = id || state.activeId;
  if (!target) {
    state.goal = null;
    renderGoal(null);
    return;
  }
  try {
    state.goal = await invoke("get_goal", { sessionId: target });
  } catch (_) {
    /* a transient failure keeps whatever is on screen */
    return;
  }
  if (target === state.activeId) renderGoal(state.goal);
}

/* Drive the state machine. The transitions live in the backend, so an action
   that does not apply comes back as a sentence rather than a silent no-op. */
async function goalAction(action, reason) {
  if (!state.activeId) return false;
  try {
    state.goal = await invoke("goal_action", {
      sessionId: state.activeId,
      action,
      reason: reason === undefined ? null : reason,
    });
    renderGoal(state.goal);
    return true;
  } catch (err) {
    goalFail(String(err));
    return false;
  }
}

/* Create, replace or clear the goal. A blank objective clears it, which is the
   backend's rule and is surfaced here instead of being second-guessed. */
async function saveGoal(objective, maxRounds) {
  if (!state.activeId) return false;
  try {
    state.goal = await invoke("set_goal", {
      sessionId: state.activeId,
      objective,
      maxRounds: maxRounds === undefined ? null : maxRounds,
    });
    renderGoal(state.goal);
    return true;
  } catch (err) {
    goalFail(String(err));
    return false;
  }
}

/* Bind the card once; the markup is static, so every listener is attached
   here instead of being rebuilt per render. */
function bindGoalCard() {
  // The head chip is the always-reachable entry point while the card is folded.
  const chip = $("goal-open");
  if (chip) chip.addEventListener("click", () => openGoal());
  const r = goalRefs();
  if (!r.card) return;
  if (r.advance) r.advance.addEventListener("click", () => void goalAction("advance"));
  if (r.edit) r.edit.addEventListener("click", () => goalShowForm(true));
  r.cancel.addEventListener("click", () => {
    // Cancelling the first-ever edit leaves no goal, so the card folds away
    // instead of idling as an empty box above the composer.
    if (!state.goal) state.goalOpen = false;
    goalShowForm(false);
    renderGoal(state.goal);
  });
  if (r.pause) {
    r.pause.addEventListener("click", () => void goalAction(state.goal && state.goal.state === "paused" ? "resume" : "pause"));
  }
  if (r.complete) r.complete.addEventListener("click", () => void goalAction("complete"));
  if (r.remove) {
    r.remove.addEventListener("click", () => void saveGoal("", null));
  }
  if (r.block) {
    r.block.addEventListener("click", () => {
      r.blockForm.classList.toggle("hidden");
      if (!r.blockForm.classList.contains("hidden")) r.reason.focus();
    });
  }
  r.blockForm.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const why = r.reason.value.trim();
    // The backend refuses a blank reason too; saying so here means the reader
    // learns it without a round trip.
    if (!why) {
      goalFail("A blocked goal has to say what blocks it.");
      return;
    }
    void goalAction("block", why).then((ok) => {
      if (ok) {
        r.blockForm.classList.add("hidden");
        r.reason.value = "";
      }
    });
  });
  r.form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const cap = r.max.value.trim();
    const parsed = cap === "" ? null : Number(cap);
    if (parsed !== null && (!Number.isFinite(parsed) || parsed < 0)) {
      goalFail("Max rounds must be a whole number, or empty for no cap.");
      return;
    }
    void saveGoal(r.input.value, parsed).then((ok) => {
      if (ok) goalShowForm(false);
    });
  });
  r.clear.addEventListener("click", () =>
    void saveGoal("", null).then((ok) => ok && goalShowForm(false)));
}
bindGoalCard();

/* ---------- session management ---------- */

async function reloadSessionList() {
  // The sidebar's own reload: re-read the list and repaint it, without
  // disturbing whichever chat is open.
  state.sessions = await invoke("list_sessions");
  renderSidebar();
}

/* One rule decides which chat is open, because a chat's workspace decides the
   model, tools and skills that run it (§24.6): the transcript shows a chat of the
   workspace in use — or nothing, when that workspace has no chat yet. Keep the
   open chat while it belongs here; else the newest live chat of this workspace
   (pins first, the sidebar's own order); else **no chat at all**, because starting
   one is a gesture — the row's `+`, Cmd/Ctrl+N, `/new`, or the first message typed
   into the composer — and not a side effect of looking at a folder (rev 117).

   Every workspace command funnels through here (applyWorkspaceSnapshot), so no
   snapshot can leave a transcript of workspace A on screen while workspace B's
   agent would answer the next message. */
async function syncTranscriptToWorkspace() {
  const here = state.activeWorkspace;
  // With no workspace there is nowhere for a chat to live, so this does not
  // invent one: the transcript empties out and the sidebar asks for a folder
  // instead (§24.6). The backend refuses the same create.
  if (!here) {
    state.activeId = null;
    state.promptNav = null;
    clearAttachments();
    forgetSkillScope();
    renderAll();
    return;
  }
  const mine = state.sessions.filter(
    (s) => !s.archived && (s.workspace || here) === here,
  );
  if (state.activeId && mine.some((s) => s.id === state.activeId)) {
    // The open chat still belongs here; the plan store and the goal card are
    // re-pointed anyway, because a stream may have moved either since.
    await restorePlan(state.activeId);
    await refreshGoal(state.activeId);
    return;
  }
  const pick = mine.find((s) => s.pinned) || mine[0];
  if (!pick) {
    // A workspace with no chat keeps none: adding a folder, or switching to an
    // empty workspace, leaves the panel as empty as it found it, and the composer's
    // first message (or the row's "+") is what starts a chat. The reader did not ask
    // a question, so there is nothing to record a "New chat" about (rev 117).
    state.activeId = null;
    state.promptNav = null;
    clearAttachments();
    forgetSkillScope();
    renderAll();
    return;
  }
  state.activeId = pick.id;
  resetWindow(); // a widened window belonged to the chat that was open
  state.promptNav = null; // so did the history walk
  clearAttachments(); // so did the composer's staged images
  forgetSkillScope();
  if (!state.cache[state.activeId]) await loadActive();
  await restorePlan(state.activeId);
  await refreshGoal(state.activeId);
}

async function refreshSessions() {
  const list = await invoke("list_sessions");
  state.sessions = Array.isArray(list) ? list : [];
  await syncTranscriptToWorkspace();
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
  resetWindow(); // a widened window belonged to the chat that was open
  state.promptNav = null; // the walk belonged to the previous chat
  clearAttachments(); // so were the composer's staged images
  forgetSkillScope();
  if (!state.cache[id]) await loadActive();
  await restorePlan(id);
  await refreshGoal(id);
  renderAll();
}

/* Start a chat in a named workspace. A chat's workspace decides the model and
   the tools that run it, so this switches first and creates second: creating in
   a workspace that is not active would need a second way to say which one. */
async function newSessionIn(workspaceId) {
  if (workspaceId && workspaceId !== state.activeWorkspace) {
    await switchWorkspace(workspaceId);
  }
  await newChat();
}

async function newChat() {
  // A chat belongs to a workspace; with none there is nowhere to put it, and the
  // backend refuses — so say why rather than letting the refusal surface raw.
  if (!state.activeWorkspace) {
    toast(tr("ws.none.chat", "Add a workspace first: a chat belongs to one"));
    return;
  }
  const s = await invoke("create_session");
  state.sessions.unshift(s);
  state.activeId = s.id;
  resetWindow();
  clearAttachments(); // staged images belonged to the previous chat's composer
  forgetSkillScope();
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

/* A pin is presentation, so it is the one row action that does not wait for a
   turn to finish; the backend is the authority on that rule, and this only
   reports what it says. */
async function setPinned(id, pinned) {
  try {
    await invoke("set_session_pinned", { id, pinned });
  } catch (e) {
    state.error = String(e);
  }
  await refreshSessions();
  renderSidebar();
}

/* Archive hides a chat without destroying it. Hiding the chat you are looking
   at would leave the transcript on screen for a chat the sidebar no longer
   lists, so the archive moves to a chat that is still listed. */
async function setArchived(id, archived) {
  try {
    await invoke("set_session_archived", { id, archived });
  } catch (e) {
    state.error = String(e);
    renderSidebar();
    return;
  }
  if (archived && id === state.activeId) {
    const next = state.sessions.find(
      (s) =>
        s.id !== id &&
        !s.archived &&
        (s.workspace || state.activeWorkspace) === state.activeWorkspace,
    );
    if (next) await switchSession(next.id);
  }
  await refreshSessions();
  renderAll();
}

/* Fork the whole conversation: branch at the end, which is the same thing as
   copying it into a chat of its own. Branching from the middle is the
   transcript's own "branch from here", not the row menu's job. */
async function forkChat(id) {
  try {
    const messages = await invoke("load_session", { id });
    // The backend refuses a branch before the first turn, and it is right to:
    // a branch at 0 is a blank chat, which is not what "fork this conversation"
    // can mean. Saying so here keeps the failure in the interface instead of in
    // a refused request.
    if (!messages || messages.length === 0) {
      state.error = tr("session.forkEmpty", "this chat has no turns to fork yet");
      renderSidebar();
      scheduleRepaint();
      return;
    }
    const fork = await invoke("fork_session", {
      sessionId: id,
      index: (messages || []).length,
    });
    await refreshSessions();
    state.sessions = [fork, ...state.sessions.filter((s) => s.id !== fork.id)];
    await switchSession(fork.id);
    renderAll();
  } catch (e) {
    state.error = String(e);
    renderSidebar();
    scheduleRepaint();
  }
}

/* Rename in place. A modal would be a second surface for one string; the row
   already shows the name, so the name becomes the field. Enter saves, Escape
   (or losing focus) puts the row back. */
function startRename(id) {
  const row = sessionList.querySelector(`.session-row[data-id="${id}"]`);
  if (!row) return false;
  const holder = row.querySelector(".title");
  if (!holder || row.querySelector(".sess-rename")) return false;
  const input = el("input", "sess-rename");
  input.type = "text";
  input.value = (state.sessions.find((s) => s.id === id) || {}).title || "";
  input.setAttribute("aria-label", tr("session.name", "Chat name"));
  input.maxLength = 80;
  row.classList.add("renaming");
  holder.replaceWith(input);
  input.focus();
  input.select();
  let done = false;
  const restore = () => {
    if (done) return;
    done = true;
    renderSidebar();
  };
  const commit = async () => {
    if (done) return;
    done = true;
    const title = input.value;
    try {
      await invoke("rename_session", { id, title });
      state.error = null;
    } catch (e) {
      state.error = String(e);
    }
    await refreshSessions();
    renderSidebar();
    renderTitle();
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void commit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      restore();
    }
  });
  input.addEventListener("blur", () => void commit());
  return true;
}

/* One place that turns a menu entry into the action it names, so the row menu
   and the keyboard chords cannot drift apart. */
function runSessionAction(s, run) {
  if (run === "pin") return void setPinned(s.id, !s.pinned);
  if (run === "rename") return void startRename(s.id);
  if (run === "fork") return void forkChat(s.id);
  if (run === "archive") return void setArchived(s.id, !s.archived);
  if (run === "delete") return void deleteSession(s.id);
  return undefined;
}

/* The chat the sidebar says is open, or nothing. */
function activeSession() {
  return state.sessions.find((s) => s.id === state.activeId && !s.archived) || null;
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

/* ---------- image attachments ----------

   A turn may carry screenshots/photos alongside its text (codex parity, and
   the deepseek upload affordance). Files arrive three ways — paste in the
   composer, drag onto it, or the picker — and become data URLs on the spot,
   downscaled when the bytes would bloat the transcript. The same URLs travel
   to the backend, which validates and forwards them as binary parts. */

const MAX_ATTACHMENTS = 6;
const attachStrip = $("attach-strip");
const attachBtn = $("attach-btn");
const attachFile = $("attach-file");

/* Read a dropped/pasted image into a data URL, downscaling big frames first:
   a 4K screenshot at full size is megabytes of base64 for no extra meaning. */
function imageFileToDataUrl(file) {
  const keepSmall = () =>
    new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result));
      reader.onerror = () => reject(reader.error || new Error("unreadable image"));
      reader.readAsDataURL(file);
    });
  if (file.size <= 256 * 1024) return keepSmall();
  const url = URL.createObjectURL(file);
  const img = new Image();
  return new Promise((resolve, reject) => {
    img.onload = () => {
      URL.revokeObjectURL(url);
      const max = 1280;
      const scale = Math.min(1, max / Math.max(img.width, img.height));
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.round(img.width * scale));
      canvas.height = Math.max(1, Math.round(img.height * scale));
      canvas.getContext("2d").drawImage(img, 0, 0, canvas.width, canvas.height);
      resolve(canvas.toDataURL("image/jpeg", 0.85));
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      keepSmall().then(resolve, reject);
    };
    img.src = url;
  });
}

function addAttachments(files) {
  const images = Array.from(files || []).filter((f) => f.type.startsWith("image/"));
  if (!images.length) return;
  const room = MAX_ATTACHMENTS - state.pendingImages.length;
  if (room <= 0) {
    toast(`At most ${MAX_ATTACHMENTS} images per message.`);
    return;
  }
  if (images.length > room) {
    toast(`Only ${room} more image${room === 1 ? "" : "s"} fit on this message.`);
  }
  images.slice(0, room).forEach((file) => {
    imageFileToDataUrl(file)
      .then((url) => {
        if (state.pendingImages.length >= MAX_ATTACHMENTS) return;
        state.pendingImages.push(url);
        renderAttachments();
      })
      .catch(() => toast(`Could not read ${file.name || "the pasted image"}.`));
  });
}

function renderAttachments() {
  if (!attachStrip) return;
  attachStrip.replaceChildren();
  state.pendingImages.forEach((url, i) => {
    const chip = document.createElement("div");
    chip.className = "attach-chip";
    const img = document.createElement("img");
    img.src = url;
    img.alt = `attached image ${i + 1}`;
    const drop = document.createElement("button");
    drop.type = "button";
    drop.className = "attach-x";
    drop.textContent = "✕";
    drop.setAttribute("aria-label", `Remove attached image ${i + 1}`);
    drop.addEventListener("click", () => {
      state.pendingImages.splice(i, 1);
      renderAttachments();
    });
    chip.append(img, drop);
    attachStrip.appendChild(chip);
  });
  attachStrip.classList.toggle("hidden", !state.pendingImages.length);
  // The Send gate is derived from the staged images exactly where they are
  // rendered, so every mutation path (add, remove, send, switch) keeps the
  // button honest without each caller remembering to ask.
  renderSend();
}

function clearAttachments() {
  if (!state.pendingImages.length) return;
  state.pendingImages = [];
  renderAttachments();
}

function bindAttachments() {
  if (!attachBtn || !attachFile) return;
  attachBtn.addEventListener("click", () => attachFile.click());
  attachFile.addEventListener("change", () => {
    addAttachments(attachFile.files);
    attachFile.value = "";
  });
  input.addEventListener("paste", (ev) => {
    const files = Array.from(ev.clipboardData?.files || []).filter((f) =>
      f.type.startsWith("image/"),
    );
    if (!files.length) return;
    ev.preventDefault();
    addAttachments(files);
  });
  const composer = $("composer");
  if (!composer) return;
  composer.addEventListener("dragover", (ev) => {
    if (Array.from(ev.dataTransfer?.types || []).includes("Files")) ev.preventDefault();
  });
  composer.addEventListener("drop", (ev) => {
    const files = ev.dataTransfer?.files;
    if (!files || !files.length) return;
    const images = Array.from(files).filter((f) => f.type.startsWith("image/"));
    if (!images.length) return;
    ev.preventDefault();
    addAttachments(files);
  });
}
bindAttachments();

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
  const images = state.pendingImages.slice();
  if (!text.trim() && !images.length) return;
  closePalette();
  closeMention();
  input.value = "";
  clearAttachments();
  state.promptNav = null; // a send ends the history walk (list moves on)
  autosize();
  renderSend();
  // An image-only turn has no slash command to run; a text turn may.
  if (text.trim() && runCommand(text)) return;
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
    queueFollowUp(text, images);
    return;
  }
  await sendText(text, state.activeId, null, images);
}

/* Park one follow-up (text plus any attachments) in the active chat's queue,
   or hand the content back when the line is full — nothing is dropped. */
function queueFollowUp(text, images) {
  const q = state.queue[state.activeId] || (state.queue[state.activeId] = []);
  if (q.length >= MAX_QUEUE) {
    input.value = text;
    autosize();
    state.pendingImages.push(...images.slice(0, MAX_ATTACHMENTS - state.pendingImages.length));
    renderAttachments();
    state.error = `Queue full (${MAX_QUEUE} messages waiting) — wait for the turn to finish or press Stop.`;
    renderAll();
    return;
  }
  q.push({ text, images });
  renderQueue();
}

/* Push the local (user, assistant) pair, then ask the backend to stream.
   On failure the pair is rolled back and the prompt restored to the input
   (active chat only — a flushed background follow-up never hijacks the
   composer). Returns whether the backend accepted the turn, so queue
   flushing can tell a real failure from a clean hand-off.

   `truncateFrom` (edit-and-resend) drops every local message from that
   index on before pushing the pair; the backend drops the same tail from
   the stored record, so a failed invoke must splice the saved rows back. */
async function sendText(text, sessionId = state.activeId, truncateFrom = null, images = []) {
  if (!state.activeWorkspace) {
    toast(tr("ws.none.chat", "Add a workspace first: a chat belongs to one"));
    return false;
  }
  if (!sessionId) {
    // The composer is the workspace's front door: the first message of an empty
    // workspace starts its chat here, at the moment something is actually asked,
    // rather than when the folder was added (rev 117).
    await newChat();
    sessionId = state.activeId;
    if (!sessionId) return false;
  }
  const msgs = state.cache[sessionId] || (state.cache[sessionId] = []);
  const removed = truncateFrom != null ? msgs.splice(truncateFrom) : null;
  msgs.push({ role: "User", text, reasoning: "", tools: [], images, error: null });
  msgs.push({ role: "Assistant", text: "", reasoning: "", tools: [], error: null });
  state.usage = null;
  state.toolCount = 0;
  state.error = null;
  renderAll();

  try {
    const streaming = await invoke("send_message", {
      sessionId,
      text,
      images,
      truncateFrom,
    });
    state.streams[sessionId] = streaming;
    renderStatus();
    renderSend();
    renderQueue();
    // The sidebar carries the stream state too (a working chat's marker, and its own
    // workspace's row), so it has to be told the turn started — no other repaint
    // reaches it, and the colour a user asked for would arrive late or not at all.
    renderSidebar();
    tick();
    return true;
  } catch (err) {
    msgs.pop();
    msgs.pop();
    if (removed) msgs.push(...removed); // restore the tail the edit dropped
    if (sessionId === state.activeId && !input.value.trim()) {
      input.value = text;
      autosize();
      // The attachments come back with the text: typed and pasted content is
      // never dropped silently, on a failure any more than on a full queue.
      state.pendingImages.push(
        ...images.slice(0, MAX_ATTACHMENTS - state.pendingImages.length),
      );
      renderAttachments();
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
  const { text, images } = q.shift();
  if (!q.length) delete state.queue[sessionId];
  renderQueue();
  const ok = await sendText(text, sessionId, null, images || []);
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
  msgs.push({
    role: "User",
    text: user.text,
    reasoning: "",
    tools: [],
    images: user.images || [],
    error: null,
  });
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
    renderSidebar();
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
  // Chrome counts the wrapped placeholder in scrollHeight, so pinning an empty
  // composer to it would leave it tall forever after one clear; an empty value
  // is one row by definition, which is the CSS min-height.
  input.style.height = "auto";
  input.style.height = input.value
    ? Math.min(input.scrollHeight, 180) + "px"
    : "";
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
  { name: "/files", hint: "Browse the workspace files (Settings → Files)", run: () => navTo("files-section") },
  { name: "/run", hint: "Run a shell command, output streams into the run panel", run: () => void setRunPanelMode("run") },
  {
    name: "/diff",
    hint: "Review the workspace's uncommitted changes (git diff)",
    run: reviewDiff,
  },
  { name: "/term", hint: "Attach an interactive shell for this workspace (survives a reload)", run: () => void attachTermSession() },
  { name: "/export", hint: "Keep this chat as a ZIP file", run: () => void openExportView() },
  { name: "/settings", hint: "Open settings & capabilities", run: () => openSettings() },
  { name: "/goal", hint: "Set or review the goal for this chat", run: () => openGoal() },
  {
    name: "/trajectory",
    hint: "Read this chat as steps: turns, tool calls, timings",
    run: () => setCenterTab("trajectory"),
  },
  { name: "/chat", hint: "Back to the conversation", run: () => setCenterTab("chat") },
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

/* /diff (codex parity): review what changed in the workspace. The stat goes
   first so the shape of the change is readable before the details; both run
   through the run panel, the same surface any git command shares. */
function reviewDiff() {
  state.runPanel.open = true;
  setRunMode("run");
  renderRunPanel();
  persistRunLines();
  runShellCommand("git --no-pager diff --stat && git --no-pager diff");
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
  capItems.textContent = "…";
  try {
    renderCapabilities(capItems, await invoke("list_capabilities", { session: state.activeId || null }));
  } catch (err) {
    capItems.textContent = String(err);
  }
}

/* What the connected MCP servers publish besides tools, read on demand.

   The bounds here are the backend's: the catalog arrives already row-capped and
   text-capped, so this code counts and names rather than clamping again — a
   second clamp would report a bound the backend never applied. A resource's
   text goes into the pane and nowhere else: reading it is a gesture, never a
   side effect of a server being connected. */
function catalogServerLine(view) {
  const v = view || {};
  if (!v.connected) return "not connected this pass";
  const resources = v.resources_supported
    ? `${v.resources_total} resource${v.resources_total === 1 ? "" : "s"}`
    : "no resources capability";
  const prompts = v.prompts_supported
    ? `${v.prompts_total} prompt${v.prompts_total === 1 ? "" : "s"}`
    : "no prompts capability";
  return `${resources} · ${prompts}`;
}

function catalogRowLine(row) {
  const bits = [];
  if (row.mime_type) bits.push(row.mime_type);
  if (row.clipped) bits.push("clipped");
  return bits.join(" · ");
}

function promptRowLine(prompt) {
  const args = Number(prompt.arguments) || 0;
  const required = Number(prompt.required) || 0;
  const shape =
    args === 0
      ? "no arguments"
      : `${args} argument${args === 1 ? "" : "s"}${required ? ` · ${required} required` : ""}`;
  return shape;
}

function readLine(read) {
  const r = read || {};
  const where = `${r.server}/${r.uri}`;
  if (r.empty) return `${where} · no text (${r.mime_type || "no type"})`;
  const size = r.truncated ? `${r.chars} of ${r.source_chars} chars` : `${r.chars} chars`;
  const mime = r.mime_type ? ` · ${r.mime_type}` : "";
  const parts = r.parts > 1 ? ` · ${r.parts} parts` : "";
  return `${where} · ${size}${mime}${parts}${r.truncated ? " · truncated" : ""}`;
}

/* One fetched prompt's receipt: server/name, message count, size, cut. */
function promptLine(view) {
  const v = view || {};
  const where = `${v.server}/${v.name}`;
  if (v.empty) return `${where} · no messages`;
  const size = v.truncated ? `${v.chars} of ${v.source_chars} chars` : `${v.chars} chars`;
  return `${where} · ${v.messages} message${v.messages === 1 ? "" : "s"} · ${size}${v.truncated ? " · truncated" : ""}`;
}

async function fetchPrompt(name, server) {
  const picker = $("mcp-read-server");
  const target = server || (picker ? picker.value : "");
  const out = $("mcp-read-out");
  out.textContent = "";
  out.hidden = true;
  if (!target) return panelNote("mcp-read-msg", "no connected MCP server to fetch from", true);
  if (!name) return panelNote("mcp-read-msg", "a prompt fetch needs a name", true);
  let view;
  try {
    view = await invoke("mcp_get_prompt", { server: target, name, nowMs: Date.now() });
  } catch (err) {
    panelNote("mcp-read-msg", String(err), true);
    return;
  }
  state.mcpRead = view;
  out.textContent = view.empty ? "" : view.text;
  out.hidden = !!view.empty;
  panelNote("mcp-read-msg", promptLine(view), false);
}

function renderCatalog(note) {
  const host = $("mcp-catalog");
  if (!host) return;
  const report = state.mcpCatalog || {};
  const servers = Array.isArray(report.servers) ? report.servers : [];
  const picker = $("mcp-read-server");
  if (picker) {
    const keep = picker.value;
    picker.textContent = "";
    for (const view of servers) {
      if (!view.connected) continue;
      const option = el("option", "", view.server);
      option.value = view.server;
      picker.appendChild(option);
    }
    if (keep && servers.some((v) => v.connected && v.server === keep)) picker.value = keep;
  }
  host.textContent = "";
  if (!servers.length) {
    host.appendChild(el("div", "cat-empty", "No MCP servers are configured."));
    return;
  }
  for (const view of servers) {
    const block = el("div", "cat-server");
    const head = el("div", "cat-head");
    head.appendChild(el("span", "cat-name", view.server));
    head.appendChild(el("span", "cat-why", catalogServerLine(view)));
    block.appendChild(head);
    if (view.error) block.appendChild(el("div", "cat-error", view.error));
    for (const row of view.resources || []) {
      const line = el("div", "cat-row");
      const use = el("button", "cat-use", row.name || row.uri);
      use.type = "button";
      use.title = row.uri;
      use.setAttribute("aria-label", `Read ${row.uri}`);
      use.addEventListener("click", () => void readResource(row.uri, view.server));
      line.appendChild(use);
      const why = catalogRowLine(row);
      if (why) line.appendChild(el("span", "cat-why", why));
      block.appendChild(line);
    }
    for (const prompt of view.prompts || []) {
      const line = el("div", "cat-row cat-prompt");
      const use = el("button", "cat-use", prompt.name);
      use.type = "button";
      use.title = prompt.description || prompt.name;
      use.setAttribute("aria-label", `Fetch prompt ${prompt.name}`);
      use.addEventListener("click", () => void fetchPrompt(prompt.name, view.server));
      line.appendChild(use);
      line.appendChild(el("span", "cat-why", promptRowLine(prompt)));
      if (prompt.description) line.appendChild(el("span", "cat-desc", prompt.description));
      block.appendChild(line);
    }
    const rows = (view.resources || []).length + (view.prompts || []).length;
    if (!view.error && rows === 0) {
      block.appendChild(
        el("div", "cat-empty", view.connected ? "Nothing published." : "Not connected."),
      );
    }
    if (view.resources_total > (view.resources || []).length) {
      block.appendChild(
        el("div", "cat-empty", `showing the first ${(view.resources || []).length} of ${view.resources_total}`),
      );
    }
    host.appendChild(block);
  }
  const bounds = `rows up to ${report.max_rows} · a read is cut at ${report.max_text_chars} chars`;
  panelNote("mcp-cat-msg", note ? `${note} · ${bounds}` : `${servers.length} server${servers.length === 1 ? "" : "s"} · ${bounds}`, false);
}

async function loadCatalog() {
  try {
    state.mcpCatalog = await invoke("mcp_catalog", { nowMs: Date.now() });
  } catch (err) {
    panelNote("mcp-cat-msg", String(err), true);
    return;
  }
  renderCatalog();
}

async function readResource(uri, server) {
  const picker = $("mcp-read-server");
  const target = server || (picker ? picker.value : "");
  const input = $("mcp-uri");
  const requested = String(uri == null ? input.value : uri).trim();
  // Every refusal and every failure clears the pane first: text left from an
  // earlier read would otherwise sit under an error as if it were this read's
  // result.
  const out = $("mcp-read-out");
  out.textContent = "";
  out.hidden = true;
  if (!target) return panelNote("mcp-read-msg", "no connected MCP server to read from", true);
  if (!requested) return panelNote("mcp-read-msg", "a resource read needs a URI", true);
  input.value = requested;
  let read;
  try {
    read = await invoke("mcp_read_resource", {
      server: target,
      uri: requested,
      nowMs: Date.now(),
    });
  } catch (err) {
    panelNote("mcp-read-msg", String(err), true);
    return;
  }
  state.mcpRead = read;
  out.textContent = read.empty ? "" : read.text;
  out.hidden = !!read.empty;
  panelNote("mcp-read-msg", readLine(read), false);
}

/* Skills the directory holds, and the form that writes one.

   The list is the loader's own reading — `instructions_chars` is the string the
   agent receives, not the file size — so a save the loader cannot parse shows up
   as a missing row instead of a success message next to a skill that will never
   load. Saving clears the palette's cache: a skill the palette cannot see is one
   the model cannot be asked to use. */
const SKILL_MAX_BODY = 12000;

function skillCapsLine(report) {
  const r = report || {};
  const skills = Array.isArray(r.skills) ? r.skills : [];
  const denied = skills.filter((s) => s.denied).length;
  const where = r.exists ? r.dir : `${r.dir} (missing — saving creates it)`;
  const off = denied ? ` · ${denied} off for this workspace` : "";
  return `${skills.length} skill${skills.length === 1 ? "" : "s"}${off} · ${where} · bodies up to ${r.max_body_chars || SKILL_MAX_BODY} chars`;
}

function skillLine(skill) {
  const chars = Number(skill.instructions_chars) || 0;
  const off = skill.denied ? " · off for this workspace" : "";
  return `${chars} chars of instruction${off} · ${skill.path}`;
}

function renderSkillNote(text, err) {
  panelNote("skill-note", text, err);
}

/* One translated string, with English as the fallback so a key the table has
   not grown yet still renders a sentence rather than a key. */
function tr(key, fallback) {
  const api = window.Polish;
  if (api && typeof api.t === "function") return api.t(key) || fallback;
  return fallback;
}

/* What this chat carries, in one line.

   The report belongs to one chat, so a report for a different chat is not this
   chat's answer: switching chats must not leave the previous chat's list on
   screen. That is why the session the report names is compared here rather than
   trusted. */
function skillScope(activation, openChat) {
  if (!openChat) return null;
  if (activation && activation.session === openChat) return activation;
  return undefined; // a chat is open and its list has not been read yet
}

function skillScopeLine(activation, openChat) {
  if (!openChat) return tr("skill.scopeNoChat", "Open a chat to switch a skill on for it.");
  const report = skillScope(activation, openChat);
  if (report === undefined) {
    return tr("skill.scopeReading", "Reading this chat's skills…");
  }
  if (!report) return tr("skill.scopeNoChat", "Open a chat to switch a skill on for it.");
  const head = report.explicit
    ? tr("skill.scopeOwn", "This chat carries its own list")
    : tr("skill.scopeInherit", "This chat follows the workspace");
  const active = Array.isArray(report.active) ? report.active.length : 0;
  const available = Array.isArray(report.available) ? report.available.length : 0;
  return `${head} · ${active} ${tr("skill.scopeOf", "of")} ${available} ${tr(
    "skill.scopeCount",
    "skills",
  )}`;
}

/* Draw the scope line and its one gesture. */
function renderSkillScope() {
  const line = $("skill-scope");
  const clear = $("skill-inherit");
  const openChat = state.activeId || "";
  const report = skillScope(state.skillActivation, openChat);
  if (line) line.textContent = skillScopeLine(state.skillActivation, openChat);
  if (clear) clear.hidden = !(report && report.explicit);
}

/* Redraw everything the activation decides: the scope line *and* the rows,
   because a toggle is a claim about the open chat and a stale one claims the
   wrong chat's state. Before the answer arrives the rows are drawn from no
   report at all, which disarms them rather than guessing. */
function renderSkillState() {
  if (state.skillsReport) renderSkills();
  else renderSkillScope();
}

/* A chat switch makes the report stale, and a stale report is not a smaller
   answer than no report: it is another chat's. Drop it, redraw what is left,
   and read the new chat. */
function forgetSkillScope() {
  state.skillActivation = null;
  renderSkillState();
  loadActivation().catch(() => {});
}

function renderSkills(note) {
  const host = $("skill-list");
  if (!host) return;
  const report = state.skillsReport || {};
  const skills = Array.isArray(report.skills) ? report.skills : [];
  const caps = skillCapsLine(report);
  renderSkillNote(note ? `${note} · ${caps}` : caps, false);
  renderSkillScope();
  host.textContent = "";
  if (!skills.length) {
    host.appendChild(el("div", "skill-empty", "No skills in this directory yet."));
    return;
  }
  const openChat = state.activeId || "";
  const activation = skillScope(state.skillActivation, openChat);
  const known = activation !== undefined && !!activation;
  const carried = known && Array.isArray(activation.active) ? activation.active : [];
  for (const skill of skills) {
    const row = el("div", "skill-row");
    row.appendChild(el("span", "skill-name", skill.name));
    row.appendChild(el("span", "skill-why", skillLine(skill)));
    if (skill.description) row.appendChild(el("span", "skill-desc", skill.description));
    const use = el("button", "btn skill-use", "Use");
    use.type = "button";
    use.setAttribute("aria-label", `Use the ${skill.name} skill`);
    use.addEventListener("click", () => useSkill(skill.name));
    row.appendChild(use);
    // The chat's own switch. A workspace that switched the skill off is a
    // floor, so the control is present and disabled rather than absent: the
    // reason is then readable instead of the row silently having no control.
    const on = carried.includes(skill.name);
    const toggle = el(
      "button",
      "btn btn-ghost skill-toggle",
      known
        ? on
          ? tr("skill.on", "On in this chat")
          : tr("skill.off", "Off in this chat")
        : tr("skill.onAria", "In this chat"),
    );
    toggle.type = "button";
    toggle.setAttribute("aria-pressed", on ? "true" : "false");
    toggle.setAttribute("aria-label", `${tr("skill.onAria", "In this chat")}: ${skill.name}`);
    toggle.disabled = !openChat || !known || !!skill.denied;
    toggle.title = !openChat
      ? tr("skill.scopeNoChat", "Open a chat to switch a skill on for it.")
      : !known
        ? tr("skill.scopeReading", "Reading this chat's skills…")
        : skill.denied
          ? tr("skill.toggleDenied", "This workspace switched the skill off.")
          : "";
    toggle.addEventListener("click", () => void setSkillActive(skill.name, !on));
    row.appendChild(toggle);
    host.appendChild(row);
  }
}

/* Switch one skill on or off for the open chat.

   The set the agent carries changes, so the palette's cached capability list is
   dropped: it is the same cache `saveSkill` drops, for the same reason. */
async function setSkillActive(name, on) {
  const session = state.activeId || "";
  if (!session) {
    renderSkillNote(tr("skill.scopeNoChat", "Open a chat to switch a skill on for it."), true);
    return;
  }
  try {
    state.skillActivation = await invoke("skill_set_active", {
      session,
      name,
      active: on,
      nowMs: Date.now(),
    });
  } catch (err) {
    renderSkillNote(String(err), true);
    return;
  }
  state.skills = [];
  renderSkills();
}

/* Forget the chat's own list, so it follows the workspace again. */
async function inheritSkills() {
  const session = state.activeId || "";
  if (!session) return;
  try {
    state.skillActivation = await invoke("skill_inherit", { session, nowMs: Date.now() });
  } catch (err) {
    renderSkillNote(String(err), true);
    return;
  }
  state.skills = [];
  renderSkills();
}

/* Ask what the open chat carries. No chat, no round trip: the command refuses
   an empty session on purpose, and the pane has its own answer for that. */
async function loadActivation() {
  const session = state.activeId || "";
  if (!session) {
    state.skillActivation = null;
    renderSkillState();
    return;
  }
  try {
    state.skillActivation = await invoke("skill_activation", { session, nowMs: Date.now() });
  } catch (err) {
    state.skillActivation = null;
    renderSkillState();
    renderSkillNote(String(err), true);
    return;
  }
  renderSkillState();
}

/* Prefill the composer the way the palette does, so invoking a skill is one
   gesture from the row that names it. */
function useSkill(name) {
  input.value = `Use the ${name} skill: `;
  input.focus();
  autosize();
  renderSend();
}

async function loadSkills() {
  try {
    state.skillsReport = await invoke("skill_list", { nowMs: Date.now() });
  } catch (err) {
    renderSkillNote(String(err), true);
    return;
  }
  await loadActivation();
}

async function saveSkill() {
  const name = $("skill-name").value;
  const description = $("skill-desc").value;
  const body = $("skill-body").value;
  if (!name.trim()) return renderSkillNote("a skill needs a name", true);
  if (!description.trim()) return renderSkillNote("a skill needs a one-line description", true);
  if (!body.trim()) return renderSkillNote("a skill needs a body — the instruction the agent follows", true);
  const overwrite = $("skill-overwrite").checked;
  try {
    state.skillsReport = await invoke("skill_author", {
      name,
      description,
      body,
      overwrite,
      nowMs: Date.now(),
    });
  } catch (err) {
    renderSkillNote(String(err), true);
    return;
  }
  state.skills = [];
  $("skill-name").value = "";
  $("skill-desc").value = "";
  $("skill-body").value = "";
  $("skill-overwrite").checked = false;
  // The save answer is this directory's new listing, but it says nothing about
  // what the open chat carries: without this read, a skill authored a moment
  // ago draws as "Off in this chat" (and the scope counts are the old ones),
  // because the activation on screen was measured against the older directory.
  await loadActivation();
  renderSkills(`saved ${name.trim()}`);
  // The pane's own capability list is a second reading of the same directory.
  void loadCapabilities();
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
    // refreshSessions updates the list but never repaints the sidebar by
    // design (that is reloadSessionList's job), so the restored row needs an
    // explicit render — the same treatment deleteSession gives its list.
    await refreshSessions();
    renderAll();
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
  void loadSchedule();
  void loadKnowledge();
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
    setWebTools.checked = s.web_tools_enabled !== false;
    setSearchUrl.value = s.search_api_url || "";
    setSearchKey.value = s.search_api_key || "";
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
  loadSkills();
}

function closeSettings() {
  modal.classList.add("hidden");
  // Hand focus back to whatever opened the dialog: leaving it on a hidden
  // control strands keyboard users at the top of the document.
  const back = state.settingsOpener;
  state.settingsOpener = null;
  if (back && typeof back.focus === "function" && document.contains(back)) back.focus();
  // The captured opener may never have taken focus — `focus()` on a
  // display:none affordance (the sidebar toggle is hidden above 720px) is a
  // silent no-op, and a programmatic open records `body`, which focus() does
  // not accept either. Probe: closing then stranded the keyboard on set-model,
  // a field that is itself gone. Fall back to an anchor that is really on
  // screen instead of leaving focus inside the hidden dialog.
  const active = document.activeElement;
  const stranded =
    !active ||
    active === document.body ||
    (typeof active.getClientRects === "function" && active.getClientRects().length === 0);
  if (stranded) {
    for (const id of ["open-settings", "sidebar-toggle"]) {
      const anchor = document.getElementById(id);
      if (anchor && anchor.getClientRects().length > 0) {
        anchor.focus();
        break;
      }
    }
  }
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
      webToolsEnabled: setWebTools.checked,
      searchApiUrl: setSearchUrl.value,
      searchApiKey: setSearchKey.value,
      mcpServers: state.mcpServers,
      providers: state.providers,
    });
    state.model = s.model;
    state.budget = s.context_budget ?? 32768;
    // A new workspace root makes every cached listing about the wrong
    // directory, so the tree is dropped and refetched from the new root.
    state.workspaceRoot = setWorkspace.value || "";
    fileTree.cache = {};
    // Only the Files pane reads these listings. Refetch on a root change while
    // it is the pane on screen; otherwise the next visit's `ensureFilesLoaded`
    // sees an empty cache and fetches the new root then.
    if ($("files-section").classList.contains("active")) void restoreFileTree();
    // The run output is stored per root, so the new root's history takes over.
    restoreRunLines();
    renderRunPanel();
    // So is the column geometry: each workspace keeps its own widths.
    Shell.setLayoutScope(state.workspaceRoot || "default");
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
      // The count is what the agent actually has, so a policy that removed a
      // tool has to show here too: "2 tool" would hide the rule that decided it.
      head.appendChild(el("span", "mcp-badge ok", mcpToolsLine(st)));
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

/* What one connected server contributes: the tools it kept, and — when the
   policy removed some — how many it advertised, so the badge cannot claim a
   server is fully attached while the policy pane says otherwise. */
function mcpToolsLine(status) {
  const names = status.tool_names || [];
  if (!names.length) return `${status.tools} tool${status.tools === 1 ? "" : "s"}`;
  return serverToolLine({
    attached: status.tools,
    denied: names.length - status.tools,
    tools: names,
  });
}

function mcpStatusLabel(rep) {
  if (!rep.servers || !rep.servers.length) return "";
  if (rep.connecting) return "connecting…";
  const enabled = rep.servers.filter((s) => s.enabled);
  const failed = enabled.filter((s) => !s.connected);
  if (failed.length) return `${enabled.length - failed.length}/${enabled.length} connected`;
  return `${enabled.length} connected`;
}

/* The per-tool policy. What it prints is the decision the backend made — the
   rule that decided a tool, or the default — so the pane never re-derives a
   meaning that could drift from the one being enforced. */
function effectLabel(effect) {
  if (effect === "deny") return "Deny";
  if (effect === "ask") return "Ask";
  return "Allow";
}

function policyReason(by) {
  return !by || by === "default" ? "follows the default" : `by rule ${by}`;
}

function policyLine(report) {
  if (!report) return "";
  const total = (report.servers || []).reduce((n, s) => n + (s.tools || []).length, 0);
  const parts = [
    `${report.attached} of ${total} tools attached`,
    `${report.denied} denied`,
    `${report.asked} ask`,
    `${report.rules.length} of ${report.max_rules} rules`,
    `default ${String(report.default)}`,
  ];
  if (report.file_version !== report.expected_version) {
    parts.push(
      `this file is version ${report.file_version}, this build writes ${report.expected_version}`,
    );
  }
  return parts.join(" · ");
}

function serverToolLine(server) {
  const total = (server.tools || []).length;
  return total === server.attached
    ? `${server.attached} tool${server.attached === 1 ? "" : "s"}`
    : `${server.attached} of ${total} tools · ${server.denied} denied`;
}

function policyNote(text, err) {
  const host = $("pol-msg");
  host.textContent = text;
  host.classList.toggle("err", Boolean(err));
}

async function setPolicy(server, tool, effect) {
  try {
    state.mcpPolicy = await invoke("mcp_policy_set", { server, tool, effect, nowMs: Date.now() });
    renderPolicy();
    // The server badge counts what is attached, so denying a tool has to be
    // visible there too — one decision, two surfaces that must agree.
    await refreshMcpStatus();
  } catch (err) {
    policyNote(String(err), true);
  }
}

async function clearPolicyRule(server, tool) {
  try {
    state.mcpPolicy = await invoke("mcp_policy_clear", { server, tool, nowMs: Date.now() });
    renderPolicy();
    await refreshMcpStatus();
  } catch (err) {
    policyNote(String(err), true);
  }
}

async function setPolicyDefault(effect) {
  try {
    state.mcpPolicy = await invoke("mcp_policy_default", { effect, nowMs: Date.now() });
    renderPolicy();
    await refreshMcpStatus();
  } catch (err) {
    policyNote(String(err), true);
  }
}

async function addPolicyRule() {
  const server = $("pol-server").value.trim();
  const tool = $("pol-tool").value.trim();
  if (!server || !tool) {
    policyNote("a rule needs a server and a tool (use * to match any)", true);
    return;
  }
  await setPolicy(server, tool, $("pol-effect").value);
  $("pol-server").value = "";
  $("pol-tool").value = "";
  policyNote(`Rule saved: ${server}/${tool} → ${effectLabel($("pol-effect").value)}.`, false);
}

function policyToolRow(server, tool) {
  const row = el("div", "pol-row");
  const head = el("div", "pol-head");
  head.appendChild(el("span", "pol-name", tool.name));
  head.appendChild(el("span", "pol-why", policyReason(tool.by)));
  row.appendChild(head);
  const group = el("div", "pol-eff");
  for (const effect of ["allow", "ask", "deny"]) {
    const button = el(
      "button",
      `btn pol-btn${tool.effect === effect ? " on" : ""}${effect === "deny" ? " danger" : ""}`,
      effectLabel(effect),
    );
    button.type = "button";
    button.setAttribute("aria-pressed", String(tool.effect === effect));
    button.setAttribute("aria-label", `${effectLabel(effect)} ${server}/${tool.name}`);
    button.addEventListener("click", () => void setPolicy(server, tool.name, effect));
    group.appendChild(button);
  }
  row.appendChild(group);
  return row;
}

function renderPolicy() {
  const servers = $("pol-servers");
  const rules = $("pol-rules");
  servers.textContent = "";
  rules.textContent = "";
  const report = state.mcpPolicy;
  if (!report) return;
  $("pol-default").value = report.default;
  policyNote(policyLine(report), false);

  for (const server of report.servers || []) {
    const block = el("div", "pol-server");
    const head = el("div", "pol-head");
    head.appendChild(el("span", "pol-name", server.name));
    head.appendChild(el("span", "pol-why", serverToolLine(server)));
    block.appendChild(head);
    for (const tool of server.tools || []) block.appendChild(policyToolRow(server.name, tool));
    servers.appendChild(block);
  }
  if (!(report.servers || []).length) {
    servers.appendChild(
      el("div", "pol-empty", "No server is connected, so there are no tools to decide about."),
    );
  }

  for (const rule of report.rules || []) {
    const row = el("div", "pol-rule");
    row.appendChild(el("span", "pol-name", `${rule.server}/${rule.tool}`));
    row.appendChild(el("span", "pol-why", effectLabel(rule.effect)));
    const clear = el("button", "btn pol-clear", "Clear");
    clear.type = "button";
    clear.setAttribute("aria-label", `Clear rule ${rule.server}/${rule.tool}`);
    clear.addEventListener("click", () => void clearPolicyRule(rule.server, rule.tool));
    row.appendChild(clear);
    rules.appendChild(row);
  }
  if (!(report.rules || []).length) {
    rules.appendChild(el("div", "pol-empty", "No rule yet: every tool follows the default."));
  }
}

async function loadPolicy() {
  try {
    state.mcpPolicy = await invoke("mcp_policy_status", { nowMs: Date.now() });
    renderPolicy();
  } catch (err) {
    policyNote(String(err), true);
  }
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

$("pol-add").addEventListener("click", () => void addPolicyRule());
$("pol-default").addEventListener("change", (ev) => void setPolicyDefault(ev.target.value));
for (const id of ["pol-server", "pol-tool"]) {
  $(id).addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      void addPolicyRule();
    }
  });
}
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
  if (e.key !== "Escape") return;
  // The menu first: it sits above the dialog it may have been opened from, and
  // one Escape should close one thing.
  if (closeRowMenu(true)) return;
  // Add-workspace sits above the sidebar but is a dialog of its own, and it is
  // checked before Settings so an Escape meant for it is not spent on a modal
  // that is not even open.
  if (addWorkspaceOpen()) return void closeAddWorkspace();
  if (!modal.classList.contains("hidden")) closeSettings();
});

/* A click anywhere else, or a scroll of the list, drops an open row menu: it is
   anchored to a row that may move under it, and a menu that outlives its anchor
   points at the wrong chat. */
document.addEventListener("click", (e) => {
  if (rowMenu && !rowMenu.menu.contains(e.target)) closeRowMenu(false);
});
sessionList.addEventListener("scroll", () => closeRowMenu(false), { passive: true });
window.addEventListener("resize", () => closeRowMenu(false));

/* A keystroke belongs to a field when it is text input there. The chords below
   are the reference sidebar's, and they name the *open* chat: there is nothing
   to rename, fork, archive or pin when no chat is open. */
function fieldOwnsKeystroke(target) {
  if (!target) return false;
  if (target.isContentEditable === true) return true;
  return ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

document.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
  if (fieldOwnsKeystroke(e.target)) return;
  const key = (e.key || "").toLowerCase();
  if (e.shiftKey && key === "a") {
    const s = activeSession();
    if (!s) return;
    e.preventDefault();
    void setArchived(s.id, !s.archived);
    return;
  }
  if (e.shiftKey) return;
  if (key === "n") {
    e.preventDefault();
    void newChat();
  } else if (key === "g") {
    const s = activeSession();
    if (!s) return;
    e.preventDefault();
    startRename(s.id);
  } else if (key === "f") {
    const s = activeSession();
    if (!s) return;
    e.preventDefault();
    void forkChat(s.id);
  }
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
    state.cache[p.session_id] = reuseIdentical(state.cache[p.session_id], fresh);
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
      const fresh = await invoke("load_session", { id: p.session_id });
      state.cache[p.session_id] = reuseIdentical(state.cache[p.session_id], fresh);
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

/* The sidebar's prominent action adds a workspace, not a chat (rev 111): with no
   workspace configured there is nothing for a chat to belong to, and a chat is
   started from a workspace row's "+", Cmd/Ctrl+N, or the palette's /new. It asks
   the system's folder panel directly (rev 114), because a workspace *is* a
   folder and the folder is therefore the first question — not a form about one. */
$("workspace-add-primary").addEventListener("click", () => void addWorkspaceFromPanel());
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
    // The status is an explanation, not the content: if it cannot load, the pane
    // still lists what is remembered, so this failure stays quiet while the list's
    // own failure stays loud.
    try {
      state.memoryStatus = await invoke("memory_status");
    } catch (err) {
      state.memoryStatus = null;
    }
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

function memoryStatusLine(status, keptCount) {
  if (!status) return "";
  if (!status.enabled) return "Memory is switched off for this workspace.";
  const shared =
    status.shared_workspaces > 1
      ? `shared by ${status.shared_workspaces} workspaces`
      : "its own file";
  // The count comes from the rows on screen, not from the status: the list is
  // what the user can check, and it is refreshed by every add and forget.
  const kept = `${keptCount} of ${status.cap} kept`;
  const full = keptCount >= status.cap ? " — at the cap, so the oldest memory goes next" : "";
  return `${status.path} · ${shared} · ${kept}${full}`;
}

function renderMemories() {
  const host = $("mem-items");
  if (!host) return;
  host.textContent = "";
  // Whose memory this is — and on the line below it, which file backs it, since
  // the default file is shared by every workspace rather than owned by one.
  const owner = state.workspaces.find((w) => w.id === state.activeWorkspace);
  host.appendChild(el("div", "ws-head", `Memory for ${owner ? owner.name : "this workspace"}`));
  // The head names the workspace; this names the *file* and who else lands on
  // it, which is the part a workspace label cannot be trusted to imply.
  host.appendChild(
    el("div", "ws-form-hint", memoryStatusLine(state.memoryStatus, state.memories.length)),
  );
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

/* ---- Schedule: durable reminders (M11) ----------------------------------
   The store is Rust-side and durable; this side owns the clock and the pane.
   A reminder is either "in N minutes, once" or "every N minutes", and the
   interval is the only number either needs. The tick runs whether or not the
   pane is open — a reminder that only fires while you are looking at its list
   is not a reminder — and a closed pane costs no DOM work per tick. */

/* Deliveries are checked this often. Far finer than the one-minute minimum a
   repeat may have, and one command per interval. */
const SCHEDULE_TICK_MS = 30000;

/* The due time inside a cadence, read the same way the report carries it. */
function scheduleDue(cadence) {
  if (!cadence) return 0;
  return cadence.kind === "once"
    ? Number(cadence.at_ms) || 0
    : Number(cadence.next_ms) || 0;
}

/* How often, in the fewest words that stay exact. */
function cadenceText(cadence) {
  if (!cadence) return "";
  if (cadence.kind === "once") return "once";
  const minutes = Number(cadence.every_minutes) || 0;
  return minutes === 1 ? "every minute" : `every ${minutes} min`;
}

/* How long until a due time. Past due says so: a reminder waiting for the next
   tick has not become "in 0 min", and a long gap uses the coarse unit. */
function untilText(dueMs, nowMs) {
  const delta = (Number(dueMs) || 0) - (Number(nowMs) || 0);
  if (delta <= 0) return "due now";
  const minutes = Math.round(delta / 60000);
  if (minutes < 1) return "due now";
  if (minutes < 60) return `in ${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `in ${hours} h`;
  return `in ${Math.round(hours / 24)} d`;
}

/* One reminder: what it says, how often, when next, and how often it has run. */
function scheduleLine(schedule, nowMs) {
  if (!schedule) return "";
  const parts = [schedule.text, cadenceText(schedule.cadence)];
  parts.push(untilText(scheduleDue(schedule.cadence), nowMs));
  if (schedule.fired) parts.push(`delivered ${schedule.fired}×`);
  return parts.filter(Boolean).join(" · ");
}

/* One delivery. `at_ms` is when it was due and `noticed_ms` when a tick caught
   it, so a late delivery says how late instead of pretending it was on time. */
function deliveryLine(delivery) {
  if (!delivery) return "";
  const at = new Date(Number(delivery.at_ms) || 0);
  const when = Number.isNaN(at.getTime()) ? "" : at.toLocaleString();
  const late = (Number(delivery.noticed_ms) || 0) - (Number(delivery.at_ms) || 0);
  const lateText = late >= 60000 ? ` · ${Math.round(late / 60000)} min late` : "";
  return `${delivery.text} · ${when}${lateText}`;
}

/* The bounds, stated where they apply — the same reasoning as the memory pane's
   status line: a cap nobody can see is a cap that surprises someone. */
function scheduleCapsLine(report) {
  const caps = (report && report.caps) || null;
  if (!caps) return "";
  const reminders = ((report && report.schedules) || []).length;
  const kept = ((report && report.history) || []).length;
  const atCap = reminders >= caps.max_schedules ? " — at the cap, cancel one first" : "";
  return `${reminders} of ${caps.max_schedules} reminders${atCap} · ${kept} of ${
    caps.max_history
  } deliveries kept · a repeat needs at least ${caps.min_every_minutes} min`;
}

/* Whether the pane is on screen: open Settings, on the Schedule tab. */
function schedulePaneOpen() {
  const pane = $("schedule-section");
  return !!pane && pane.classList.contains("active") && !pane.closest("[hidden]");
}

function renderScheduleNote(text) {
  panelNote("sched-note", text, true);
}

function renderSchedule() {
  const list = $("sched-items");
  const history = $("sched-history");
  if (!list || !history) return;
  const report = state.schedule;
  const nowMs = Number((report && report.now_ms) || 0) || Date.now();
  const schedules = (report && report.schedules) || [];
  const deliveries = (report && report.history) || [];
  list.textContent = "";
  history.textContent = "";
  const count = $("sched-count");
  if (count) count.textContent = schedules.length ? `${schedules.length} scheduled` : "";
  if (!schedules.length) {
    list.appendChild(el("li", "cap-note", "Nothing scheduled yet."));
  }
  for (const schedule of schedules) {
    const row = el("li", "rule-row");
    row.appendChild(el("span", "sched-text", scheduleLine(schedule, nowMs)));
    row.appendChild(el("span", "sched-id", schedule.id));
    const cancel = el("button", "btn btn-ghost", "Cancel");
    cancel.type = "button";
    cancel.addEventListener("click", (e) => {
      e.preventDefault();
      cancelSchedule(schedule.id);
    });
    row.appendChild(cancel);
    list.appendChild(row);
  }
  if (!deliveries.length) {
    history.appendChild(el("li", "cap-note", "Nothing delivered yet."));
  }
  for (const delivery of deliveries) {
    const row = el("li", "rule-row");
    row.appendChild(el("span", "sched-text", deliveryLine(delivery)));
    history.appendChild(row);
  }
  panelNote("sched-note", scheduleCapsLine(report), false);
}

async function loadSchedule() {
  try {
    state.schedule = await invoke("schedule_list", { nowMs: Date.now() });
    renderSchedule();
  } catch (err) {
    renderScheduleNote(String(err));
  }
}

/* Deliver what is due. `report.fired` is exactly what this tick delivered, so a
   reminder that was overdue while the app was closed is announced once — and
   the 200 deliveries already in the store are history, not a backlog of
   notifications. */
async function tickSchedule() {
  try {
    const report = await invoke("schedule_tick", { nowMs: Date.now() });
    for (const delivery of (report && report.fired) || []) {
      toast(`Reminder: ${delivery.text}`);
    }
    state.schedule = report;
    if (schedulePaneOpen()) renderSchedule();
  } catch (_) {
    // A tick that cannot reach the store is not worth a message every interval;
    // opening the pane reports the same failure where there is room to read it.
  }
}

/* The interval, rounded, and refused here so the round trip is not spent on a
   value the store will reject. The store enforces it too. */
function reminderMinutes() {
  const field = $("sched-every");
  const minutes = Math.round(Number(field && field.value));
  if (!Number.isFinite(minutes) || minutes < 1) {
    renderScheduleNote("a repeating reminder needs at least 1 minute between deliveries");
    return 0;
  }
  return minutes;
}

async function addSchedule() {
  const field = $("sched-text");
  const text = field ? field.value : "";
  if (!text.trim()) {
    renderScheduleNote("a reminder needs some text");
    return;
  }
  const minutes = reminderMinutes();
  if (!minutes) return;
  const once = ($("sched-mode") || {}).value === "once";
  const nowMs = Date.now();
  try {
    state.schedule = await invoke("schedule_add", {
      text,
      everyMinutes: once ? null : minutes,
      atMs: once ? nowMs + minutes * 60000 : null,
      nowMs,
    });
    if (field) field.value = "";
    renderSchedule();
  } catch (err) {
    renderScheduleNote(String(err));
  }
}

async function cancelSchedule(id) {
  try {
    state.schedule = await invoke("schedule_cancel", { id, nowMs: Date.now() });
    renderSchedule();
  } catch (err) {
    renderScheduleNote(String(err));
  }
}

async function clearScheduleHistory() {
  try {
    state.schedule = await invoke("schedule_clear_history", { nowMs: Date.now() });
    renderSchedule();
  } catch (err) {
    renderScheduleNote(String(err));
  }
}

/* The knowledge pane (§21). The store is durable and the agent reaches it
   through a tool, so this pane is the curator: search the notes, add one, edit
   an entry in place, delete one. Every command answers with the entries as they
   stand, so one render draws them all — and the bounds come from the backend,
   because a cap nobody can see surprises someone. */
const KB_SEARCH_LIMIT = 20;

function sinceText(ms, nowMs) {
  const delta = Number(nowMs) - Number(ms);
  if (!Number.isFinite(delta) || delta < 60000) return "just now";
  const minutes = Math.round(delta / 60000);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}

function kbBodyPreview(body) {
  const text = String(body || "").replace(/\s+/g, " ").trim();
  return text.length > 200 ? `${text.slice(0, 199)}…` : text;
}

function kbCapsLine(report) {
  const caps = (report && report.caps) || null;
  if (!caps) return "";
  const total = Number(report.total) || 0;
  const full = total >= caps.max_entries ? " — at the cap, delete one first" : "";
  return `${total} of ${caps.max_entries} entries${full} · titles ${caps.max_title_chars} chars · bodies ${caps.max_body_chars} · search returns ${Math.min(
    KB_SEARCH_LIMIT,
    caps.tool_max_limit,
  )} at most`;
}

function renderKnowledgeNote(text, err) {
  panelNote("kb-note", text, err);
}

function renderKnowledge() {
  const host = $("kb-items");
  if (!host) return;
  host.textContent = "";
  const report = state.knowledge || {};
  const matches = Array.isArray(report.matches) ? report.matches : [];
  const nowMs = Date.now();
  renderKnowledgeNote(kbCapsLine(report), false);
  if (!matches.length) {
    const query = String(report.query || "").trim();
    host.appendChild(
      el(
        "div",
        "ws-form-hint",
        query ? `Nothing matches “${query}”.` : "No knowledge yet — add the first note below.",
      ),
    );
    return;
  }
  for (const entry of matches) {
    const row = el("div", "kb-row");
    row.dataset.id = entry.id;
    const head = el("div", "kb-head");
    head.appendChild(el("span", "kb-title", entry.title));
    const tags = (entry.tags || []).join(" · ");
    if (tags) head.appendChild(el("span", "kb-tagline", tags));
    head.appendChild(el("span", "kb-when", `edited ${sinceText(entry.updated_ms, nowMs)}`));
    const edit = el("button", "btn kb-edit", "Edit");
    edit.type = "button";
    edit.addEventListener("click", () => startKnowledgeEdit(entry));
    const del = el("button", "btn kb-del", "Delete");
    del.type = "button";
    del.addEventListener("click", () => void deleteKnowledge(entry.id));
    head.appendChild(edit);
    head.appendChild(del);
    row.appendChild(head);
    const body = el("div", "kb-body-view", kbBodyPreview(entry.body));
    if (entry.body) body.title = entry.body;
    row.appendChild(body);
    host.appendChild(row);
  }
}

async function loadKnowledge() {
  const field = $("kb-query");
  const query = field ? field.value.trim() : "";
  try {
    state.knowledge = query
      ? await invoke("knowledge_search", { query, limit: KB_SEARCH_LIMIT, nowMs: Date.now() })
      : await invoke("knowledge_list", { nowMs: Date.now() });
    renderKnowledge();
  } catch (err) {
    renderKnowledgeNote(String(err), true);
  }
}

function startKnowledgeEdit(entry) {
  state.kbEditing = entry.id;
  $("kb-title").value = entry.title;
  $("kb-tags").value = (entry.tags || []).join(", ");
  $("kb-body").value = entry.body || "";
  const save = $("kb-save");
  save.textContent = "Save";
  save.dataset.i18n = "kb.save";
  $("kb-cancel").hidden = false;
  renderKnowledgeNote(`Editing “${entry.title}”.`, false);
}

function cancelKnowledgeEdit() {
  state.kbEditing = null;
  $("kb-title").value = "";
  $("kb-tags").value = "";
  $("kb-body").value = "";
  const save = $("kb-save");
  save.textContent = "Add";
  save.dataset.i18n = "kb.add";
  $("kb-cancel").hidden = true;
  renderKnowledgeNote("", false);
}

async function saveKnowledge() {
  const title = $("kb-title").value;
  const tags = $("kb-tags").value;
  const body = $("kb-body").value;
  if (!title.trim()) {
    renderKnowledgeNote("a knowledge entry needs a title", true);
    return;
  }
  const editing = state.kbEditing;
  try {
    const args = { title, tags, body, nowMs: Date.now() };
    const report = editing
      ? await invoke("knowledge_update", { id: editing, ...args })
      : await invoke("knowledge_add", args);
    state.knowledge = report;
    cancelKnowledgeEdit();
    if (editing) await loadKnowledge();
    else renderKnowledge();
  } catch (err) {
    renderKnowledgeNote(String(err), true);
  }
}

async function deleteKnowledge(id) {
  try {
    state.knowledge = await invoke("knowledge_delete", { id, nowMs: Date.now() });
    if (state.kbEditing === id) cancelKnowledgeEdit();
    renderKnowledge();
  } catch (err) {
    renderKnowledgeNote(String(err), true);
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
  // Settings shows one pane at a time, so "focusing" a section means selecting
  // it: the rail's tab and the pane are two views of the same choice.
  let target = null;
  document.querySelectorAll(".set-pane").forEach((pane) => {
    const on = pane.id === sectionId;
    pane.classList.toggle("active", on);
    if (on) target = pane;
  });
  // The Files pane is the one section whose content is a fetch rather than a
  // render of state already in hand, so arriving at it is what starts the
  // fetch. It happens on the way in, before the pane is scrolled into view, so
  // the first paint lands in a pane the user is already looking at.
  if (sectionId === "files-section") void ensureFilesLoaded();
  document.querySelectorAll(".set-tab").forEach((tab) => {
    const on = tab.dataset.section === sectionId;
    tab.classList.toggle("active", on);
    tab.setAttribute("aria-selected", String(on));
    tab.tabIndex = on ? 0 : -1;
  });
  if (target) {
    target.scrollIntoView({ block: "start" });
    return;
  }
  // A fixture (or older markup) may still hand over a plain element.
  const plain = document.getElementById(sectionId);
  if (plain) plain.scrollIntoView({ block: "start" });
}

function navTo(sectionId) {
  Promise.resolve(openSettings())
    .catch(() => {})
    .then(() => focusSection(sectionId));
}

document.querySelectorAll(".set-tab").forEach((tab) => {
  tab.addEventListener("click", () => focusSection(tab.dataset.section));
});

// Fixtures render a bare subset of the markup, so both bindings are optional:
// a missing element must not take the whole module down with it.
const settingsClose = $("settings-close");
if (settingsClose) settingsClose.addEventListener("click", closeSettings);

// The rail is a vertical tablist: arrows walk it, Home/End jump to the ends.
const settingsNav = document.querySelector(".set-nav");
if (settingsNav) settingsNav.addEventListener("keydown", (ev) => {
  const steps = { ArrowDown: 1, ArrowUp: -1 };
  const tabs = [...document.querySelectorAll(".set-tab")];
  const at = tabs.findIndex((tab) => tab.classList.contains("active"));
  if (tabs.length === 0) return;
  if (ev.key === "Home") {
    ev.preventDefault();
    focusSection(tabs[0].dataset.section);
    tabs[0].focus();
    return;
  }
  if (ev.key === "End") {
    ev.preventDefault();
    const last = tabs[tabs.length - 1];
    focusSection(last.dataset.section);
    last.focus();
    return;
  }
  if (!(ev.key in steps)) return;
  const next = at < 0 ? 0 : (at + steps[ev.key] + tabs.length) % tabs.length;
  ev.preventDefault();
  focusSection(tabs[next].dataset.section);
  tabs[next].focus();
});

/* The Add-a-workspace dialog's own controls. The folder browser starts at home
   when it has no path, which is what an empty `list_folder` argument means. */
const wsAddModal = $("ws-add");
if (wsAddModal) {
  $("ws-add-create").addEventListener("click", () => void submitAddWorkspace());
  $("ws-add-cancel").addEventListener("click", () => closeAddWorkspace());
  $("ws-add-name").addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      $("ws-add-root").focus();
    }
  });
  $("ws-add-root").addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void submitAddWorkspace();
    }
  });
  $("ws-add-browse").addEventListener("click", () => void chooseFolderNative());
  $("ws-add-browse-manual").addEventListener("click", () => {
    const host = $("ws-add-browser");
    const reveal = host.classList.contains("hidden");
    if (reveal) {
      showInAppBrowser();
      void browseFolder($("ws-add-root").value.trim());
    } else {
      hideInAppBrowser();
    }
  });
  $("ws-browse-up").addEventListener("click", () => {
    if (state.wsBrowse && state.wsBrowse.parent) void browseFolder(state.wsBrowse.parent);
  });
  $("ws-browse-use").addEventListener("click", () => {
    if (!state.wsBrowse || !state.wsBrowse.path) return;
    adoptRoot(state.wsBrowse.path);
  });
  // A click on the backdrop dismisses, the same gesture the other dialogs take.
  wsAddModal.addEventListener("click", (e) => {
    if (e.target === wsAddModal) closeAddWorkspace();
  });
}

// The Workspaces header: reveal the search field, re-read the chat list, or go
// add a workspace. The search field stays out of the way until it is asked for.
$("sessions-search").addEventListener("click", () => {
  const field = $("session-search");
  const reveal = field.hidden;
  field.hidden = !reveal;
  $("side-workspaces").classList.toggle("searching", reveal);
  $("sessions-search").setAttribute("aria-expanded", String(reveal));
  if (reveal) field.focus();
});
// The head's sliders: one menu for how the chats are grouped, ordered and
// filtered, instead of three glyph controls whose meaning has to be guessed. The
// click is stopped at the button — the document closes any menu a click lands
// outside of, so without this the menu opened and shut in the same gesture
// (which is exactly what the page guard caught it doing).
$("session-view").addEventListener("click", (e) => {
  e.stopPropagation();
  const btn = $("session-view");
  if (rowMenu && rowMenu.anchor === btn) return void closeRowMenu(true);
  sessionViewMenu(btn);
});
// The folder-plus in the sidebar's head row is the one door to the flow that
// *creates* a workspace. It deliberately does not jump into Settings: choosing a
// folder and the things that may run in it is the sidebar's business, and
// Settings only inspects and edits what exists.

// Narrow windows turn the sidebar into a drawer (see the 720px rules): the
// button opens it, and picking a chat closes it, because the point of that tap is
// the chat rather than the list it came from.
// The ☰ belongs to the shell now: it folds the column where there is room for that
// and lets it out over the centre where there is not (Shell.toggleSidebar), and the
// shell writes the class and the aria that describe it. A second handler here is
// what made one click both open the drawer *and* arm a rail the user then could not
// leave once the window widened (rev 120).
const sidebarEl = $("sidebar");
if (sidebarEl) {
  sidebarEl.addEventListener("click", (event) => {
    const row = event.target.closest ? event.target.closest(".session-row") : null;
    if (row) Shell.closeDrawer();
  });
}

const addMemoryButton = $("mem-add");
if (addMemoryButton) addMemoryButton.addEventListener("click", () => addMemory());

// Guarded: the settings section exists in the app shell, and the harness page
// loads this file without it.
const addWorkspace = $("ws-manage-add");
if (addWorkspace) {
  // Settings lists and edits, it does not add: the one Add-workspace flow lives
  // on the sidebar, so this is a doorway to it rather than a second form — and it
  // is the same door, panel first, so there is only ever one answer to "what does
  // adding a workspace do".
  addWorkspace.addEventListener("click", () => {
    closeSettings();
    void addWorkspaceFromPanel();
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
const fileTree = { cache: {} };

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

/* Load the listing the Files section shows, once: selecting the section is the
   gesture that needs it, and an already-filled cache needs nothing. Visibility
   belongs to the pane (`focusSection`), so there is no open/closed flag and no
   second place that could disagree about whether the tree is showing. */
async function ensureFilesLoaded() {
  if (!Object.keys(fileTree.cache).length) await restoreFileTree();
}

function refreshFileTree() {
  fileTree.cache = {};
  void restoreFileTree();
}

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

/** One live shell per workspace root; the Rust side keeps it across reloads. */
function runPanelSessionId() {
  return "bos-pty-" + (state.workspaceRoot || "default");
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

/** Backend chunks may split a line mid-way: hold the tail until its newline. */
function pushTermChunk(text) {
  const pieces = (state.runPanel.partial + text).split("\n");
  state.runPanel.partial = pieces.pop();
  for (const piece of pieces) pushRunLine("out", piece);
}

function setRunChip(running, label) {
  const el = $("run-chip");
  if (!el) return;
  el.textContent = label || (running ? "running" : "idle");
  el.className = "chip " + (running ? "streaming" : "idle");
}

/** The honesty note swaps with the mode: a terminal must say it is one. */
function setRunMode(mode) {
  state.runPanel.mode = mode === "term" ? "term" : "run";
  state.runPanel.partial = "";
  // The class carries the mode to CSS (the Kill control only exists in term
  // mode); the note carries it to the eye.
  const panel = $("run-panel");
  if (panel) panel.classList.toggle("term", state.runPanel.mode === "term");
  const note = $("run-note");
  if (note) {
    note.textContent =
      state.runPanel.mode === "term"
        ? "interactive shell · ANSI stripped"
        : "plain text, not a terminal";
  }
  const field = $("run-input");
  if (field) {
    field.placeholder =
      state.runPanel.mode === "term"
        ? "Type a command — Enter sends it to the shell"
        : "Run a shell command in the workspace\u200B — Enter to run, output streams in above";
  }
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
    localStorage.setItem(
      runPanelKey(),
      JSON.stringify({
        open: state.runPanel.open,
        mode: state.runPanel.mode,
        lines: state.runPanel.lines,
      }),
    );
  } catch (_) {
    /* private mode or a full quota: the panel still works, it just forgets */
  }
}

/* What the blob may look like on disk, as a pure function the harness can
   pin: the open flag arrived after the bare array, the mode after that, and
   both older shapes still load — nobody's panel should be lost to a tidier
   format. Garbage for a field means the safe default, never a crash. */
function parseRunBlob(raw) {
  if (Array.isArray(raw)) return { open: false, mode: "run", lines: raw };
  if (raw && typeof raw === "object" && Array.isArray(raw.lines)) {
    return {
      open: raw.open === true,
      mode: raw.mode === "term" ? "term" : "run",
      lines: raw.lines,
    };
  }
  return { open: false, mode: "run", lines: [] };
}

/** Load the lines (and open state) the last session showed for this root. */
function restoreRunLines() {
  let parsed = { open: false, mode: "run", lines: [] };
  try {
    parsed = parseRunBlob(JSON.parse(localStorage.getItem(runPanelKey()) || "null"));
  } catch (_) {
    parsed = { open: false, mode: "run", lines: [] };
  }
  setRunMode(parsed.mode);
  state.runPanel.open = parsed.open;
  state.runPanel.lines = parsed.lines.filter(
    (l) => l && typeof l.text === "string" && typeof l.stream === "string"
  );
  // Term mode follows the shell, not the snapshot: the backend replays the
  // live tail (it holds far more than the blob kept), and a workspace switch
  // re-attaches to that root's session instead of this one's history.
  if (state.runPanel.mode === "term") void attachTermSession();
}

async function toggleRunPanel() {
  state.runPanel.open = !state.runPanel.open;
  renderRunPanel();
  persistRunLines();
  if (state.runPanel.open) {
    const field = $("run-input");
    if (field) field.focus();
  }
}

/** `/run`: show the panel in one-shot mode. A term session stays alive in the
    background — switching back to it with the Shell button re-attaches. */
function setRunPanelMode(mode) {
  if (mode === "term") return attachTermSession();
  state.runPanel.open = true;
  setRunMode("run");
  renderRunPanel();
  persistRunLines();
  const field = $("run-input");
  if (field) field.focus();
}

/**
 * Attach the panel to this workspace's shell: `/term` opens it, and a reload
 * or a workspace switch re-attaches to the same live process with its tail
 * replayed. The old run lines are shed here on purpose — they belong to the
 * run-mode history, and the backend's tail is the fuller truth anyway.
 */
async function attachTermSession() {
  state.runPanel.open = true;
  setRunMode("term");
  state.runPanel.lines = [];
  renderRunPanel();
  persistRunLines();
  const field = $("run-input");
  if (field) field.focus();
  try {
    const status = await invoke("pty_start", {
      sessionId: runPanelSessionId(),
      shell: null,
    });
    for (const chunk of status.tail) pushTermChunk(chunk);
    if (state.runPanel.partial) pushRunLine("out", state.runPanel.partial);
    setRunChip(true, status.fresh ? "shell" : "attached");
    pushRunLine(
      "note",
      `[${status.shell} · ${status.fresh ? "started" : "re-attached"}]`,
    );
    scheduleRunPaint();
    persistRunLines();
    syncTermSize();
  } catch (err) {
    setRunChip(false, "no shell");
    pushRunLine("note", `[shell unavailable: ${err}]`);
    scheduleRunPaint();
    persistRunLines();
  }
}

$("term-open").addEventListener("click", () => void attachTermSession());

$("run-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const field = $("run-input");
  const raw = field.value || "";
  if (state.runPanel.mode === "term") {
    // The PTY echoes what is typed; echoing here too would print it twice.
    // An empty line is a bare Enter — legal at a shell prompt.
    field.value = "";
    try {
      await invoke("pty_write", {
        sessionId: runPanelSessionId(),
        data: raw + "\n",
      });
    } catch (err) {
      pushRunLine("note", `[refused: ${err}]`);
      scheduleRunPaint();
    }
    return;
  }
  const command = raw.trim();
  // One run at a time: a second command would interleave its lines with the
  // first, and the panel has no way to tell them apart on screen.
  if (!command || state.runPanel.activeId) return;
  runShellCommand(command);
});

/** Run one command through the run panel and stream its output in. `/diff`
    takes this path with a git command, so the review is the same surface. */
function runShellCommand(command) {
  const field = $("run-input");
  if (field) field.value = "";
  const runId = `run-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
  state.runPanel.activeId = runId;
  setRunChip(true);
  pushRunLine("note", "$ " + command);
  scheduleRunPaint();
  invoke("run_command", { runId, command }).catch((err) => {
    state.runPanel.activeId = null;
    setRunChip(false);
    pushRunLine("note", `[refused: ${err}]`);
    scheduleRunPaint();
    persistRunLines();
  });
}

$("run-clear").addEventListener("click", () => {
  state.runPanel.lines = [];
  renderRunPanel();
  persistRunLines();
});

$("run-close").addEventListener("click", () => {
  state.runPanel.open = false;
  renderRunPanel();
  persistRunLines(); // a closed panel stays closed across a reload
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

/* The shell outlives the webview: events for this root's session keep
   flowing as long as the panel is in term mode, and only that one. */
listen("pty-output", (event) => {
  const p = event.payload;
  if (state.runPanel.mode !== "term" || p.session_id !== runPanelSessionId()) return;
  pushTermChunk(p.text);
  scheduleRunPaint();
});

listen("pty-exit", (event) => {
  const p = event.payload;
  if (state.runPanel.mode !== "term" || p.session_id !== runPanelSessionId()) return;
  const how = p.exit_code === null || p.exit_code === undefined ? "killed" : `exit ${p.exit_code}`;
  setRunChip(false, how);
  pushRunLine("note", `[session ended · ${how} · the Shell button starts a new one]`);
  scheduleRunPaint();
  persistRunLines();
});

$("term-kill").addEventListener("click", async () => {
  if (state.runPanel.mode !== "term") return;
  try {
    await invoke("pty_kill", { sessionId: runPanelSessionId() });
    // The exit event narrates the end; nothing else to say here.
  } catch (err) {
    pushRunLine("note", `[kill refused: ${err}]`);
    scheduleRunPaint();
  }
});

/* A program that lays out columns (`ls`, `htop`) needs the real window, so
   the shell hears the panel's size — measured in characters off the panel's
   own monospace font, then kept in step while the window changes. */
let termCharW = 0;
let termCharH = 0;
function syncTermSize() {
  if (state.runPanel.mode !== "term") return;
  const out = $("run-output");
  if (!out || !out.clientWidth || !out.clientHeight) return;
  if (!termCharW) {
    const probe = document.createElement("span");
    probe.textContent = "0".repeat(100);
    probe.style.cssText = "position:absolute;visibility:hidden;white-space:pre;";
    const cs = getComputedStyle(out);
    probe.style.font = cs.font || `${cs.fontSize} ${cs.fontFamily}`;
    out.appendChild(probe);
    termCharW = probe.offsetWidth / 100;
    termCharH = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.45 || 16;
    probe.remove();
    if (!termCharW) return;
  }
  const cols = Math.max(20, Math.min(500, Math.floor(out.clientWidth / termCharW) - 1));
  const rows = Math.max(5, Math.min(200, Math.floor(out.clientHeight / termCharH)));
  invoke("pty_resize", { sessionId: runPanelSessionId(), cols, rows }).catch(() => {});
}
window.addEventListener("resize", () => {
  if (state.runPanel.mode === "term") syncTermSize();
});

/* ---------- init ---------- */

async function init() {
  try {
    // Settings come first on purpose: which workspace is active decides which
    // chat the session sync opens, and a selection made before the active
    // workspace is known could land on a chat another workspace owns.
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
    // How this person reads the list is remembered before the first paint, so a
    // restart opens on the view they left, not on the default and a jump.
    restoreSessionView();
    await refreshSessions();
    const streams = await invoke("current_stream");
    for (const s of streams) {
      state.streams[s.session_id] = s;
      // Warm each streaming chat's cache so switching to it renders live.
      ensureSessionCache(s.session_id);
    }
    // The run panel keeps what it showed: reload that history for this root.
    restoreRunLines();
    renderRunPanel();
    // Column geometry is per workspace too; this swaps in the root's widths.
    Shell.setLayoutScope(state.workspaceRoot || "default");
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

/* ===================== run surfaces: M4 · M6 · M7 · M10 · M11 · M14 =========
   One small panel per capability, all inside Settings, each driven by the
   command that owns the state. Every render function takes (host, data) so a
   guard can drive it with a fixture rather than a live backend, and every
   refresh funnels its failure into the panel's own note instead of throwing
   into a handler nobody sees. */

function fmtBytes(n) {
  const v = Number(n) || 0;
  if (v < 1024) return `${v} B`;
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KiB`;
  return `${(v / (1024 * 1024)).toFixed(1)} MiB`;
}

function fmtMs(ms) {
  if (ms === null || ms === undefined) return "—";
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
}

function meterRow(host, label, value) {
  host.appendChild(el("span", "meter-key", label));
  host.appendChild(el("span", "meter-value", String(value)));
}

function panelNote(id, text, err) {
  const host = $(id);
  if (!host) return;
  host.textContent = text || "";
  host.classList.toggle("err", !!err);
}

/* ---------- M7: background jobs ---------- */

function jobStateChip(job) {
  const state = typeof job.state === "string" ? job.state : String(job.state);
  return el("span", "job-state s-" + state, state);
}

function renderJobs(host, jobs) {
  host.textContent = "";
  if (!jobs || !jobs.length) {
    host.appendChild(el("div", "cap-note", "No background jobs yet — a shell command or a streaming run appears here while it is alive."));
    return 0;
  }
  for (const job of jobs) {
    const row = el("div", "job-row");
    row.setAttribute("role", "listitem");
    row.tabIndex = 0;
    row.appendChild(el("span", "job-label", clip(job.label || job.id, 34)));
    row.appendChild(el("span", "job-kind", job.kind || ""));
    row.appendChild(jobStateChip(job));
    row.appendChild(el("span", "job-bytes", fmtBytes(job.bytes)));
    if (job.finished_at) {
      row.appendChild(el("span", "job-time", fmtMs((job.finished_at - job.started_at) * 1000)));
    }
    if (job.error) row.appendChild(el("span", "job-error", clip(job.error, 48)));
    row.addEventListener("click", () => openJob(job.id));
    row.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") openJob(job.id);
    });
    host.appendChild(row);
  }
  return jobs.length;
}

function renderJobOutput(host, out) {
  host.textContent = "";
  host.classList.remove("hidden");
  const head = el("div", "job-detail-head");
  head.appendChild(el("span", "job-label", out.id));
  head.appendChild(jobStateChip({ state: out.state }));
  head.appendChild(
    el(
      "span",
      "job-bytes",
      `${fmtBytes(out.bytes)} of ${fmtBytes(out.total_bytes)}${out.truncated ? " (tail)" : ""}`,
    ),
  );
  if (out.dropped_bytes) {
    head.appendChild(el("span", "job-error", `${fmtBytes(out.dropped_bytes)} dropped`));
  }
  const stop = el("button", "btn btn-ghost", "Stop");
  stop.type = "button";
  stop.addEventListener("click", () => interruptJob(out.id));
  head.appendChild(stop);
  host.appendChild(head);
  host.appendChild(el("pre", "job-output", out.text || "(no output yet)"));
}

async function openJob(id) {
  try {
    renderJobOutput($("job-detail"), await invoke("job_output", { id, max_bytes: 16384 }));
  } catch (err) {
    panelNote("jobs-count", `Could not read job ${id}: ${err}`, true);
  }
}

async function interruptJob(id) {
  try {
    await invoke("job_interrupt", { id });
    await refreshJobs();
  } catch (err) {
    panelNote("jobs-count", `Could not stop ${id}: ${err}`, true);
  }
}

async function refreshJobs() {
  const host = $("jobs-list");
  try {
    const jobs = await invoke("jobs_list");
    const n = renderJobs(host, jobs);
    panelNote("jobs-count", n ? `${n} shown` : "");
    return n;
  } catch (err) {
    panelNote("jobs-count", `Could not read jobs: ${err}`, true);
    return 0;
  }
}

/* ---------- M6: context, compaction and spill ---------- */

function renderTokenMeter(host, r) {
  host.textContent = "";
  meterRow(host, "messages", r.messages);
  meterRow(host, "estimated", `${r.estimated} tok`);
  meterRow(host, "folded", `${r.archived_estimated} tok`);
  meterRow(host, "provider", r.provider === null || r.provider === undefined ? "—" : `${r.provider} tok`);
  meterRow(
    host,
    "delta",
    r.delta === null || r.delta === undefined ? "—" : `${r.delta > 0 ? "+" : ""}${r.delta} tok`,
  );
  meterRow(
    host,
    "within tolerance",
    r.within_tolerance === null || r.within_tolerance === undefined
      ? "—"
      : r.within_tolerance
        ? "yes"
        : "no",
  );
}

function renderCompaction(host, s) {
  host.textContent = "";
  meterRow(host, "live", s.live_messages);
  meterRow(host, "folded", s.archived_messages);
  meterRow(host, "estimated", `${s.estimated_tokens} tok`);
  meterRow(host, "budget", s.budget_tokens ? `${s.budget_tokens} tok` : "off");
  meterRow(host, "over budget", s.over_budget ? "yes" : "no");
  meterRow(host, "can compact", s.can_compact ? "yes" : "no");
  if (s.compacting) meterRow(host, "compacting", "yes");
  if (s.refusal) meterRow(host, "refusal", s.refusal);
}

function renderSpills(host, list) {
  host.textContent = "";
  if (!list || !list.length) {
    host.appendChild(
      el(
        "div",
        "cap-note",
        "Nothing has spilled — an over-large tool output is written here instead of being kept inline.",
      ),
    );
    return 0;
  }
  for (const entry of list) {
    const li = el("li", "spill-row");
    li.tabIndex = 0;
    li.appendChild(el("span", "spill-id", entry.id));
    li.appendChild(el("span", "job-bytes", fmtBytes(entry.bytes)));
    li.appendChild(el("span", "spill-preview", clip(entry.preview || entry.file, 64)));
    li.addEventListener("click", () => openSpill(entry.id));
    host.appendChild(li);
  }
  return list.length;
}

async function openSpill(id) {
  try {
    const read = await invoke("spill_read", { id, max_bytes: 8192 });
    const pre = $("spill-detail");
    pre.classList.remove("hidden");
    pre.textContent = `${read.file}${read.truncated ? " (truncated)" : ""}\n\n${read.text}`;
  } catch (err) {
    panelNote("ctx-note", `Could not read ${id}: ${err}`, true);
  }
}

async function refreshContext() {
  const id = state.activeId;
  if (!id) {
    panelNote("ctx-note", "Open a chat first — the meter describes one conversation.", true);
    return;
  }
  try {
    renderTokenMeter(
      $("token-meter"),
      await invoke("token_meter", { session_id: id, provider_tokens: null }),
    );
    renderCompaction($("compact-status"), await invoke("compaction_status", { session_id: id }));
    renderSpills($("spill-list"), await invoke("spill_list"));
    panelNote("ctx-note", "");
  } catch (err) {
    panelNote("ctx-note", String(err), true);
  }
}

async function compactNow() {
  if (!state.activeId) return panelNote("ctx-note", "Open a chat first.", true);
  try {
    const status = await invoke("compact_now", { session_id: state.activeId });
    // The refresh clears the note as it redraws, so the outcome goes in after.
    await refreshContext();
    panelNote(
      "ctx-note",
      status.refusal
        ? `Not compacted: ${status.refusal}`
        : `Folded ${status.archived_messages} turn(s); ${status.live_messages} live, ${status.estimated_tokens} tok estimated.`,
    );
  } catch (err) {
    panelNote("ctx-note", String(err), true);
  }
}

async function restoreFolded() {
  if (!state.activeId) return panelNote("ctx-note", "Open a chat first.", true);
  try {
    const restored = await invoke("restore_compacted", { session_id: state.activeId });
    await refreshContext();
    panelNote("ctx-note", `Restored ${restored} folded turn(s).`);
  } catch (err) {
    panelNote("ctx-note", String(err), true);
  }
}

/* ---------- M4: scoped approval rules and the audit ring ---------- */

function ruleArgText(rule) {
  if (!rule.arg) return "any args";
  const kind = rule.arg.match_kind || rule.arg.kind || "match";
  return `${rule.arg.key} ${kind} "${rule.arg.value}"`;
}

function renderRules(host, report) {
  host.textContent = "";
  const rules = (report && report.rules) || [];
  const warnings = (report && report.warnings) || [];
  panelNote(
    "rules-warnings",
    warnings.length ? `${warnings.length} problem(s) in the rules file: ${warnings.join("; ")}` : "",
    warnings.length > 0,
  );
  if (!rules.length) {
    host.appendChild(
      el("li", "cap-note", "No scoped rules yet — a rule is how “always allow” stops being a per-call question."),
    );
    return 0;
  }
  for (const rule of rules) {
    const li = el("li", "rule-row");
    li.appendChild(el("span", "rule-effect e-" + rule.effect, rule.effect));
    li.appendChild(el("span", "rule-tool", rule.tool));
    li.appendChild(
      el("span", "rule-scope", typeof rule.scope === "string" ? rule.scope : String(rule.scope)),
    );
    li.appendChild(el("span", "rule-arg", ruleArgText(rule)));
    const revoke = el("button", "btn btn-ghost", "Revoke");
    revoke.type = "button";
    revoke.addEventListener("click", () => revokeRule(rule.id));
    li.appendChild(revoke);
    host.appendChild(li);
  }
  return rules.length;
}

function renderAudit(host, entries) {
  host.textContent = "";
  if (!entries || !entries.length) {
    host.appendChild(el("li", "cap-note", "No decisions recorded yet."));
    return 0;
  }
  for (const entry of entries) {
    const li = el("li", "audit-row");
    li.appendChild(el("span", "audit-when", fmtTime(entry.at)));
    li.appendChild(el("span", "rule-effect e-" + entry.decision, entry.decision));
    li.appendChild(el("span", "rule-tool", entry.tool));
    li.appendChild(el("span", "audit-args", clip(entry.args || "", 56)));
    if (entry.rule_id) li.appendChild(el("span", "rule-scope", entry.rule_id));
    host.appendChild(li);
  }
  return entries.length;
}

async function refreshRules() {
  try {
    renderRules($("rules-list"), await invoke("approval_rules_list"));
    renderAudit($("audit-list"), await invoke("approval_audit", { limit: 20 }));
  } catch (err) {
    panelNote("rules-warnings", String(err), true);
  }
}

async function addRule() {
  const tool = ($("rule-tool").value || "").trim();
  const key = ($("rule-key").value || "").trim();
  const match = $("rule-match").value;
  const value = ($("rule-value").value || "").trim();
  if (!tool) return panelNote("rules-warnings", "A rule needs a tool name.", true);
  if (!!key !== !!match) {
    return panelNote("rules-warnings", "An argument predicate needs a key and a match kind together.", true);
  }
  if (key && !value) return panelNote("rules-warnings", "An argument predicate needs a value.", true);
  const scope = $("rule-scope").value;
  try {
    await invoke("approval_rule_add", {
      tool,
      scope,
      effect: $("rule-effect").value,
      arg_key: key || null,
      arg_match: match || null,
      arg_value: value || null,
      workspace: null,
      session_id: scope === "session" ? state.activeId || null : null,
    });
    $("rule-tool").value = "";
    $("rule-key").value = "";
    $("rule-value").value = "";
    $("rule-match").value = "";
    // The refresh clears the note as it redraws the book, so the confirmation
    // goes in after it — the same ordering compactNow needs.
    await refreshRules();
    panelNote("rules-warnings", `Added: ${tool} (${scope}).`);
  } catch (err) {
    panelNote("rules-warnings", String(err), true);
  }
}

async function revokeRule(id) {
  try {
    renderRules($("rules-list"), await invoke("approval_rule_revoke", { id }));
    renderAudit($("audit-list"), await invoke("approval_audit", { limit: 20 }));
  } catch (err) {
    panelNote("rules-warnings", String(err), true);
  }
}

/* ---------- M10 · M11 · M14: replay, self-inspection, bundle ---------- */

function renderInspection(host, i) {
  host.textContent = "";
  const rows = [
    ["tools", i.tools],
    ["async tools", i.async_tools],
    ["skills", i.skills],
    ["plugins", i.plugins],
    ["hook events", i.hook_events],
    ["MCP servers", i.mcp_servers],
    ["MCP tools", i.mcp_tools],
    ["memories", i.memories],
    ["approval rules", i.approval_rules],
  ];
  for (const [label, value] of rows) meterRow(host, label, value);
  meterRow(host, "model", i.model || "—");
  meterRow(host, "workspace", i.workspace || "—");
  meterRow(host, "bash", i.bash_enabled ? "on" : "off");
  meterRow(host, "file tools", i.file_tools_enabled ? "on" : "off");
  if (i.active_goal) {
    const cap = Number(i.goal_rounds || 0) + Number(i.goal_remaining || 0);
    meterRow(host, "goal", `${i.goal_state || "?"} · ${i.goal_rounds || 0}/${cap || "?"}`);
  }
  meterRow(host, "skills dir", i.skills_dir || "—");
}

function renderScore(host, s) {
  host.textContent = "";
  host.classList.remove("hidden");
  host.appendChild(el("div", "score-head", `${s.grade} · ${s.score}`));
  const ul = el("ul", "score-reasons");
  const reasons = s.reasons || [];
  if (!reasons.length) ul.appendChild(el("li", "", "No problems were found in this transcript."));
  for (const reason of reasons) ul.appendChild(el("li", "", reason));
  host.appendChild(ul);
}

function renderReplay(host, r) {
  host.textContent = "";
  host.classList.remove("hidden");
  host.appendChild(el("div", "score-head", `Replay of ${r.title || r.session_id}`));
  const m = r.metrics || {};
  const grid = el("div", "meter-grid");
  const rows = [
    ["turns", m.turns],
    ["user", m.user_turns],
    ["assistant", m.assistant_turns],
    ["folded", m.archived_turns],
    ["tool calls", m.tool_calls],
    ["tool failures", m.tool_failures],
    ["tool time", fmtMs(m.tool_ms_total)],
    ["errors", m.errors],
    ["blank turns", m.blank_turns],
    ["estimated", `${m.estimated_tokens} tok`],
  ];
  for (const [label, value] of rows) meterRow(grid, label, value);
  host.appendChild(grid);
  const ul = el("ul", "replay-entries");
  for (const entry of (r.entries || []).slice(0, 20)) {
    const li = el("li", entry.archived ? "replay-row folded" : "replay-row");
    li.appendChild(
      el("span", "rule-scope", `${entry.order}. ${entry.role}${entry.archived ? " (folded)" : ""}`),
    );
    li.appendChild(el("span", "replay-text", clip(entry.text || entry.reasoning || "", 76)));
    const tools = entry.tools || [];
    if (tools.length) li.appendChild(el("span", "job-bytes", `${tools.length} tool`));
    if (entry.error) li.appendChild(el("span", "job-error", clip(entry.error, 36)));
    ul.appendChild(li);
  }
  host.appendChild(ul);
}

async function refreshInspect() {
  try {
    renderInspection(
      $("insight-grid"),
      await invoke("self_inspect", { session_id: state.activeId || null }),
    );
  } catch (err) {
    panelNote("bundle-result", String(err), true);
  }
}

async function evaluateChat() {
  if (!state.activeId) return panelNote("bundle-result", "Open a chat first.", true);
  try {
    renderScore($("insight-score"), await invoke("evaluate_transcript", { session_id: state.activeId }));
    panelNote("bundle-result", "");
  } catch (err) {
    panelNote("bundle-result", String(err), true);
  }
}

async function runReplay() {
  if (!state.activeId) return panelNote("bundle-result", "Open a chat first.", true);
  try {
    renderReplay($("replay-report"), await invoke("replay_session", { session_id: state.activeId }));
    panelNote("bundle-result", "");
  } catch (err) {
    panelNote("bundle-result", String(err), true);
  }
}

async function exportBundle() {
  if (!state.activeId) return panelNote("bundle-result", "Open a chat first.", true);
  try {
    const report = await invoke("export_bundle", { session_id: state.activeId });
    panelNote(
      "bundle-result",
      `Wrote ${report.path} — ${fmtBytes(report.bytes)}, ${report.entries} entr${
        report.entries === 1 ? "y" : "ies"
      }, schema v${report.schema_version}.`,
    );
  } catch (err) {
    panelNote("bundle-result", String(err), true);
  }
}

/* ---------- wiring ---------- */

const SECTION_REFRESH = {
  "jobs-section": refreshJobs,
  "context-section": refreshContext,
  "rules-section": refreshRules,
  "insight-section": refreshInspect,
  "set-mcp": () => Promise.all([refreshMcpStatus(), loadPolicy(), loadCatalog()]),
  "schedule-section": loadSchedule,
  "knowledge-section": loadKnowledge,
  "cap-list": () => Promise.all([loadCapabilities(), loadSkills()]),
  "set-prov": loadSkills,
};

function refreshSection(sectionId) {
  const refresh = SECTION_REFRESH[sectionId];
  if (refresh) Promise.resolve(refresh()).catch(() => {});
}

$("jobs-refresh").addEventListener("click", () => refreshJobs().catch(() => {}));
$("ctx-refresh").addEventListener("click", () => refreshContext().catch(() => {}));
$("ctx-compact").addEventListener("click", () => compactNow().catch(() => {}));
$("ctx-restore").addEventListener("click", () => restoreFolded().catch(() => {}));
$("rules-refresh").addEventListener("click", () => refreshRules().catch(() => {}));
$("rule-add").addEventListener("click", () => addRule().catch(() => {}));
$("insight-refresh").addEventListener("click", () => refreshInspect().catch(() => {}));
$("sched-refresh").addEventListener("click", () => loadSchedule().catch(() => {}));
$("sched-add").addEventListener("click", () => addSchedule().catch(() => {}));
$("sched-clear").addEventListener("click", () => clearScheduleHistory().catch(() => {}));
$("insight-evaluate").addEventListener("click", () => evaluateChat().catch(() => {}));
$("replay-run").addEventListener("click", () => runReplay().catch(() => {}));
$("bundle-export").addEventListener("click", () => exportBundle().catch(() => {}));
$("kb-search").addEventListener("click", () => void loadKnowledge());
$("kb-all").addEventListener("click", () => {
  $("kb-query").value = "";
  void loadKnowledge();
});
$("kb-query").addEventListener("keydown", (ev) => {
  if (ev.key === "Enter") {
    ev.preventDefault();
    void loadKnowledge();
  }
});
$("kb-save").addEventListener("click", () => void saveKnowledge());
$("skill-save").addEventListener("click", () => void saveSkill());
$("skill-inherit").addEventListener("click", () => void inheritSkills());
$("mcp-read-btn").addEventListener("click", () => void readResource());
$("mcp-uri").addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    void readResource();
  }
});
$("kb-cancel").addEventListener("click", () => cancelKnowledgeEdit());
document.querySelectorAll(".set-tab").forEach((tab) => {
  tab.addEventListener("click", () => refreshSection(tab.dataset.section));
});

/* Deliver anything that came due while the app was closed, then keep checking.
   This runs before the first paint, so an overdue reminder is announced with
   the window. */
void tickSchedule();
setInterval(() => void tickSchedule(), SCHEDULE_TICK_MS);

init();
