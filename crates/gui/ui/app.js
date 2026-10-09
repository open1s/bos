/* BOS chat frontend: bridges Tauri IPC (invoke + events) to the DOM. */
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/* ---------- state ---------- */

const state = {
  sessions: [],          // [{id, title, updated_at}] newest first
  activeId: null,
  cache: {},             // sessionId -> [ChatMessage]
  streams: {},           // sessionId -> {session_id, gen, started_at} (many may stream)
  usage: null,           // {prompt, completion} | null
  toolCount: 0,
  error: null,
  model: "",
  budget: 32768,        // context-budget meter ceiling (tokens; 0 = off)
  approvals: [],         // queued approval-request payloads
  queue: {},             // sessionId -> [text] follow-ups waiting for the stream
  compacting: {},        // sessionId -> true while /compact summarizes in the background
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

function renderSidebar() {
  sessionList.textContent = "";
  const q = state.search.trim().toLowerCase();
  for (const s of state.sessions) {
    const title = s.title || "New chat";
    if (q && !title.toLowerCase().includes(q)) continue;
    const row = el("div", "session-row" + (s.id === state.activeId ? " active" : ""));
    row.appendChild(el("span", "title", clip(title, 24)));
    if (streamOf(s.id)) row.appendChild(el("span", "dot", "●"));
    const age = el("span", "time", fmtAge(s.updated_at));
    age.title = fmtTime(s.updated_at);
    row.appendChild(age);
    const del = el("button", "del", "✕");
    del.title = "Delete chat";
    del.addEventListener("click", (e) => {
      e.stopPropagation();
      deleteSession(s.id);
    });
    row.appendChild(del);
    row.addEventListener("click", () => switchSession(s.id));
    sessionList.appendChild(row);
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
  const sameShape =
    msgs === lastMsgsRef &&
    msgs.length === lastMsgsLen &&
    rowCache.size === msgs.length &&
    messagesEl.childElementCount === msgs.length &&
    (msgs.length === 0 || msgs[msgs.length - 1] === lastTailRef);
  const from = sameShape ? Math.min(dirtyFrom, msgs.length) : 0;

  if (sameShape && from >= msgs.length) {
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
  for (let i = from; i < msgs.length; i++) {
    const m = msgs[i];
    const showCaret = streamingHere && i === msgs.length - 1;
    const editingHere = !!(editing && editing.index === i);
    const opts = {
      index: i,
      regen: idle && i === lastAssistant && !!m.text,
      edit: idle && m.role === "User" && !!m.text,
      del: idle,
      branch: idle && i > 0,
    };
    const key = msgKey(m);
    if (seen) seen.add(key);
    const sig = messageSignature(m, { ...opts, showCaret }, editingHere);
    let entry = rowCache.get(key);
    if (!entry || entry.sig !== sig) {
      const row = el("div", m.role === "User" ? "user" : "assistant");
      row.classList.add("msg");
      if (editingHere) {
        row.appendChild(buildEditor(m, i));
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
  lastTailRef = msgs.length ? msgs[msgs.length - 1] : null;
  dirtyFrom = msgs.length;

  if (followTail) stickToBottom();
  else messagesEl.scrollTop = prevTop;
}

function renderStatus() {
  statusLeft.textContent = "";
  statusLeft.appendChild(el("span", "", state.model || "no model"));
  statusLeft.appendChild(
    el("span", "", `${state.sessions.length} chat${state.sessions.length === 1 ? "" : "s"}`)
  );

  statusRight.textContent = "";
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
  if (!window.confirm("Delete this chat?")) return;
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
    return;
  }
  p.entries.forEach((entry, i) => {
    const li = el("li", "cmd-item" + (i === p.index ? " sel" : ""));
    li.setAttribute("role", "option");
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
async function loadCapabilities() {
  capList.textContent = "loading…";
  try {
    const caps = await invoke("list_capabilities");
    capList.textContent = "";
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
      capList.appendChild(head);
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
        capList.appendChild(row);
      }
    }
    if (!any) {
      const empty = document.createElement("div");
      empty.className = "cap-empty";
      empty.textContent = "No capabilities registered yet.";
      capList.appendChild(empty);
    }
  } catch (err) {
    capList.textContent = String(err);
  }
}

async function openSettings() {
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
    setWorkspace.value = s.bash_workspace || "";
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
}

listen("approval-request", (event) => {
  state.approvals.push(event.payload);
  showNextApproval();
});

$("approval-deny").addEventListener("click", () => answerApproval(false));
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
$("plan-edit").addEventListener("click", () => startPlanEdit());
sendBtn.addEventListener("click", () =>
  streamOf(state.activeId) ? stopStream() : send(),
);
sessionSearch.addEventListener("input", () => {
  state.search = sessionSearch.value;
  renderSidebar();
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
