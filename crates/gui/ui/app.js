/* BOS chat frontend: bridges Tauri IPC (invoke + events) to the DOM. */
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/* ---------- state ---------- */

const state = {
  sessions: [],          // [{id, title, updated_at}] newest first
  activeId: null,
  cache: {},             // sessionId -> [ChatMessage]
  streaming: null,       // {session_id, gen, started_at} | null
  usage: null,           // {prompt, completion} | null
  toolCount: 0,
  error: null,
  model: "",
  approvals: [],         // queued approval-request payloads
  mcpServers: [],        // edit buffer from the settings dialog
  mcpStatus: null,       // {connecting, servers} from the mcp_status command
};

/* ---------- DOM ---------- */

const $ = (id) => document.getElementById(id);
const sessionList = $("session-list");
const messagesEl = $("messages");
const chatTitle = $("chat-title");
const chip = $("stream-chip");
const input = $("input");
const sendBtn = $("send");
const statusLeft = $("status-left");
const statusRight = $("status-right");
const modal = $("settings-modal");
const settingsForm = $("settings-form");
const setModel = $("set-model");
const setBaseUrl = $("set-base-url");
const setApiKey = $("set-api-key");
const setSystem = $("set-system");
const setTemp = $("set-temperature");
const setEffort = $("set-effort");
const setBash = $("set-bash");
const setFiles = $("set-files");
const setApproval = $("set-approval");
const setWorkspace = $("set-workspace");
const setSkillsDir = $("set-skills-dir");
const capList = $("cap-list");
const mcpList = $("mcp-list");
const mcpName = $("mcp-name");
const mcpTransport = $("mcp-transport");
const mcpCmd = $("mcp-cmd");
const mcpUrl = $("mcp-url");
const mcpAddBtn = $("mcp-add-btn");
const mcpMsg = $("mcp-msg");
const mcpConn = $("mcp-conn");
const approvalModal = $("approval-modal");
const approvalTool = $("approval-tool");
const approvalArgs = $("approval-args");

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

/* ---------- rendering ---------- */

function activeMsgs() {
  return state.cache[state.activeId] || [];
}

function renderSidebar() {
  sessionList.textContent = "";
  for (const s of state.sessions) {
    const row = el("div", "session-row" + (s.id === state.activeId ? " active" : ""));
    row.appendChild(el("span", "title", clip(s.title || "New chat", 24)));
    row.appendChild(el("span", "time", fmtTime(s.updated_at)));
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
  chatTitle.textContent = active ? clip(active.title || "New chat", 48) : "BOS";
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

function buildBubble(m, showCaret, opts) {
  const bubble = el("div", "bubble");
  if (m.reasoning) bubble.appendChild(el("div", "reasoning", m.reasoning));
  for (const t of m.tools || []) {
    bubble.appendChild(el("div", "tool", `${t.name}(${t.args})`));
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
  if (m.role !== "User" && m.text) {
    const acts = el("div", "bubble-actions");
    const copyBtn = el("button", "mini-btn", "Copy");
    copyBtn.type = "button";
    copyBtn.addEventListener("click", async () => {
      const ok = await copyText(m.text);
      flash(copyBtn, ok ? "Copied" : "Copy failed");
    });
    acts.appendChild(copyBtn);
    if (opts && opts.regen) {
      const regenBtn = el("button", "mini-btn", "Regenerate");
      regenBtn.type = "button";
      regenBtn.addEventListener("click", () => regenerate());
      acts.appendChild(regenBtn);
    }
    bubble.appendChild(acts);
  }
  return bubble;
}

function renderMessages() {
  const msgs = activeMsgs();
  const nearBottom =
    messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight < 80;
  const prevTop = messagesEl.scrollTop;

  messagesEl.textContent = "";
  if (msgs.length === 0) {
    const hint = el("div", "empty-hint");
    hint.appendChild(el("div", "", "New conversation"));
    hint.appendChild(el("div", "", "Type a message below to begin."));
    messagesEl.appendChild(hint);
    return;
  }

  const streamingHere =
    state.streaming && state.streaming.session_id === state.activeId;

  // Regenerate applies to the last assistant reply once the turn is idle.
  let lastAssistant = -1;
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === "Assistant") {
      lastAssistant = i;
      break;
    }
  }

  msgs.forEach((m, i) => {
    const row = el("div", m.role === "User" ? "user" : "assistant");
    row.classList.add("msg");
    row.appendChild(
      buildBubble(m, streamingHere && i === msgs.length - 1, {
        regen: !state.streaming && i === lastAssistant && !!m.text,
      })
    );
    messagesEl.appendChild(row);
  });

  if (nearBottom) messagesEl.scrollTop = messagesEl.scrollHeight;
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
  if (state.streaming) {
    sendBtn.textContent = "■ Stop";
    sendBtn.classList.add("stop");
    sendBtn.disabled = false;
  } else {
    sendBtn.textContent = "Send";
    sendBtn.classList.remove("stop");
    sendBtn.disabled = !input.value.trim();
  }
}

function tick() {
  if (state.streaming && state.streaming.session_id === state.activeId) {
    const secs = Math.max(0, Math.floor(Date.now() / 1000) - state.streaming.started_at);
    chip.textContent = `streaming · ${secs}s`;
    chip.className = "chip streaming";
  } else if (state.streaming) {
    chip.textContent = "another chat is streaming";
    chip.className = "chip";
  } else {
    chip.textContent = "idle";
    chip.className = "chip idle";
  }
}

function renderAll() {
  renderSidebar();
  renderTitle();
  renderMessages();
  renderStatus();
  renderSend();
  tick();
}

/* ---------- session management ---------- */

async function refreshSessions() {
  let list = await invoke("list_sessions");
  if (list.length === 0) list = [await invoke("create_session")];
  state.sessions = list;
  if (!list.some((s) => s.id === state.activeId)) state.activeId = list[0].id;
  if (!state.cache[state.activeId]) await loadActive();
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
  if (!state.cache[id]) await loadActive();
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
  renderAll();
  input.focus();
}

async function deleteSession(id) {
  if (!window.confirm("Delete this chat?")) return;
  await invoke("delete_session", { id }).catch(() => {});
  delete state.cache[id];
  if (state.streaming && state.streaming.session_id === id) state.streaming = null;
  await refreshSessions();
  renderAll();
}

/* ---------- sending ---------- */

async function send() {
  if (state.streaming) {
    await invoke("stop_streaming").catch(() => {});
    // stop_streaming denies every pending approval server-side.
    clearApprovals();
    return;
  }
  const text = input.value;
  if (!text.trim()) return;
  input.value = "";
  autosize();
  await sendText(text);
}

/* Push the local (user, assistant) pair, then ask the backend to stream.
   On failure the pair is rolled back and the prompt restored to the input. */
async function sendText(text) {
  const msgs = state.cache[state.activeId] || (state.cache[state.activeId] = []);
  msgs.push({ role: "User", text, reasoning: "", tools: [], error: null });
  msgs.push({ role: "Assistant", text: "", reasoning: "", tools: [], error: null });
  state.usage = null;
  state.toolCount = 0;
  state.error = null;
  renderAll();

  try {
    const streaming = await invoke("send_message", {
      sessionId: state.activeId,
      text,
    });
    state.streaming = streaming;
    renderStatus();
    renderSend();
    tick();
  } catch (err) {
    msgs.pop();
    msgs.pop();
    input.value = text;
    autosize();
    state.error = String(err);
    renderAll();
  }
}

/* Re-run the last exchange. The backend peels the stored pair and re-saves,
   so the local cache mirrors the same pop/push and resyncs on failure. */
async function regenerate() {
  if (state.streaming) return;
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
    state.streaming = streaming;
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
    setTemp.value = String(s.temperature);
    setEffort.value = s.reasoning_effort || "";
    setBash.checked = !!s.bash_enabled;
    setFiles.checked = !!s.file_tools_enabled;
    setApproval.checked = s.require_approval !== false;
    setWorkspace.value = s.bash_workspace || "";
    setSkillsDir.value = s.skills_dir || "";
    state.mcpServers = Array.isArray(s.mcp_servers) ? s.mcp_servers : [];
    mcpMsg.textContent = "";
    renderMcp();
    refreshMcpStatus();
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
      temperature: Number.isNaN(temp) ? 0.7 : temp,
      reasoningEffort: setEffort.value || null,
      bashEnabled: setBash.checked,
      fileToolsEnabled: setFiles.checked,
      bashWorkspace: setWorkspace.value,
      skillsDir: setSkillsDir.value,
      requireApproval: setApproval.checked,
      mcpServers: state.mcpServers,
    });
    state.model = s.model;
    state.mcpServers = Array.isArray(s.mcp_servers) ? s.mcp_servers : [];
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

/* Follow a connect pass: poll until the backend stops reporting "connecting". */
async function pollMcpStatus() {
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
$("mcp-reconnect-btn").addEventListener("click", async () => {
  mcpMsg.textContent = "Reconnecting…";
  try {
    await invoke("reconnect_mcp");
    await pollMcpStatus();
    mcpMsg.textContent = state.mcpStatus
      ? `Connection updated: ${mcpStatusLabel(state.mcpStatus)}`
      : "";
  } catch (err) {
    mcpMsg.textContent = String(err);
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
  approvalArgs.textContent = currentApproval.args || "{}";
  approvalModal.classList.remove("hidden");
}

function clearApprovals() {
  state.approvals = [];
  currentApproval = null;
  approvalModal.classList.add("hidden");
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

listen("agent-event", (event) => {
  const p = event.payload;
  const msgs = ensureSessionCache(p.session_id);
  if (!msgs) return;
  const last = msgs[msgs.length - 1];
  switch (p.kind) {
    case "text":
      if (last) last.text += p.text;
      break;
    case "reasoning":
      if (last) last.reasoning += p.text;
      break;
    case "tool":
      if (last) last.tools.push({ name: p.name, args: p.args });
      state.toolCount += 1;
      break;
    case "usage":
      state.usage = { prompt: p.prompt, completion: p.completion };
      break;
    case "error":
      if (last) last.error = p.message;
      state.error = p.message;
      break;
    default:
      break;
  }
  if (p.session_id === state.activeId) renderMessages();
  renderStatus();
});

listen("stream-finished", async (event) => {
  const p = event.payload;
  // The server denied any pending approvals when the stream ended.
  clearApprovals();
  try {
    const fresh = await invoke("load_session", { id: p.session_id });
    state.cache[p.session_id] = fresh;
  } catch (_) {
    delete state.cache[p.session_id];
  }
  if (state.streaming && state.streaming.gen === p.gen) state.streaming = null;
  await refreshSessions();
  renderAll();
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
sendBtn.addEventListener("click", () => send());
input.addEventListener("input", () => {
  autosize();
  renderSend();
});
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    send();
  }
});

/* ---------- init ---------- */

async function init() {
  try {
    await refreshSessions();
    const current = await invoke("current_stream");
    if (current) {
      state.streaming = current;
      ensureSessionCache(current.session_id);
    }
    const settings = await invoke("get_settings");
    state.model = settings.model;
  } catch (err) {
    state.error = String(err);
  }
  renderAll();
  setInterval(tick, 1000);
}

init();
