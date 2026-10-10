/* M13 polish: it makes the interface's language setting mean something, gives
 * the theme a one-click control next to the goal chip, turns the shortcut stub
 * into a real reference, and adds the affordances an accessibility pass needs.
 *
 * Why a separate file: `shell.js` owns layout and preferences and `app.js` owns
 * the conversation, and both are large enough that a translation pass threaded
 * through them would touch every render path. This module instead translates
 * *marks* — an element opts in with `data-i18n` (or `-placeholder`, `-aria`,
 * `-title`) — so the strings live in one table and no render path changes.
 *
 * One source of truth: `shell.js` already owns both preferences (`state.language`
 * and `state.appearance`, persisted in `bos.ui.v1`, applied to `<html lang>` and
 * `data-theme`/`data-appearance`). This module never writes its own copy — it
 * *watches* those attributes and calls the shell's public setters, so the
 * settings rows and the header toggles cannot disagree.
 */
(function () {
  "use strict";

  /* ---------- strings ---------------------------------------------------- */

  /* Every entry carries both languages so a missing translation falls back to
   * English instead of rendering the key. */
  const STRINGS = {
    "tab.chat": { en: "Chat", zh: "对话" },
    "tab.trajectory": { en: "Trajectory", zh: "轨迹" },
    "tabs.label": { en: "Conversation views", zh: "会话视图" },
    "nav.files": { en: "Files", zh: "文件" },
    "nav.plugins": { en: "Plugins & skills", zh: "插件与技能" },
    "nav.mcp": { en: "MCP connectors", zh: "MCP 连接器" },
    "nav.automation": { en: "Automation tasks", zh: "自动化任务" },
    "btn.new": { en: "New session", zh: "新建会话" },
    "btn.settings": { en: "Settings", zh: "设置" },
    "btn.send": { en: "Send", zh: "发送" },
    "brand.pill": { en: "HARNESS", zh: "HARNESS" },
    "side.workspaces": { en: "Workspaces", zh: "工作区" },
    "side.local": { en: "Local workspace", zh: "本地工作区" },
    "sec.jobs": { en: "Jobs", zh: "任务" },
    "sec.context": { en: "Context", zh: "上下文" },
    "sec.rules": { en: "Rules", zh: "规则" },
    "sec.inspect": { en: "Inspect", zh: "自检" },
    "sec.schedule": { en: "Schedule", zh: "定时" },
    "schedule.hint": {
      en: "Durable reminders. They outlive a restart, and every delivery is kept.",
      zh: "持久提醒：重启后依然存在，每次投递都会保留。",
    },
    "schedule.placeholder": { en: "Water the plants", zh: "给花浇水" },
    "schedule.add": { en: "Remind me", zh: "提醒我" },
    "schedule.once": { en: "in min, once", zh: "分钟后，一次" },
    "schedule.repeat": { en: "every min", zh: "分钟一次" },
    "schedule.history": { en: "Delivered", zh: "已投递" },
    "schedule.clear": { en: "Clear", zh: "清空" },
    "sec.knowledge": { en: "Knowledge", zh: "知识库" },
    "kb.hint": {
      en: "Notes you curate — conventions, decisions, setup. The agent searches them on demand instead of carrying them in every prompt.",
      zh: "由你维护的笔记：约定、决策、环境配置。智能体按需检索，而不是把它们塞进每次提示。",
    },
    "kb.searchPlaceholder": { en: "Search titles, tags and bodies", zh: "搜索标题、标签与正文" },
    "kb.search": { en: "Search", zh: "搜索" },
    "kb.all": { en: "All", zh: "全部" },
    "kb.titlePlaceholder": { en: "Title", zh: "标题" },
    "kb.tagsPlaceholder": { en: "tags, comma, separated", zh: "标签，逗号，分隔" },
    "kb.bodyPlaceholder": { en: "What the agent should know", zh: "希望智能体了解的内容" },
    "kb.add": { en: "Add", zh: "新增" },
    "kb.save": { en: "Save", zh: "保存" },
    "kb.cancel": { en: "Cancel edit", zh: "取消编辑" },
    "mcp.catalogTitle": { en: "Resources and prompts", zh: "资源与提示词" },
    "mcp.catalogHint": {
      en: "What the connected servers publish besides tools. A resource is read on demand — its text never enters a prompt unless you paste it — and a prompt is listed for reference.",
      zh: "已连接服务器除工具外发布的内容。资源按需读取——除非你粘贴，其正文不会进入提示词；提示词只列出供参考。",
    },
    "mcp.read": { en: "Read", zh: "读取" },
    "mcp.uriPlaceholder": { en: "uri://… (a resource a server did not list)", zh: "uri://…（服务器未列出的资源）" },
    "skill.title": { en: "Author a skill", zh: "编写技能" },
    "skill.hint": {
      en: "A skill is a folder with a SKILL.md whose body the agent reads — an instruction it can follow, discovered from the skills directory below and rebuilt into every open session when you save.",
      zh: "技能是一个含 SKILL.md 的文件夹，其正文就是智能体读取的指令；保存后会被下方技能目录发现，并重建进所有已打开的会话。",
    },
    "skill.namePlaceholder": { en: "name-with-no-spaces", zh: "名称-不含空格" },
    "skill.descPlaceholder": { en: "One line: what it does", zh: "一行说明：它做什么" },
    "skill.bodyPlaceholder": {
      en: "The instruction the agent follows",
      zh: "智能体要遵循的指令",
    },
    "skill.save": { en: "Save skill", zh: "保存技能" },
    "skill.replace": { en: "Replace it if it already exists", zh: "若已存在则替换" },
    "mcp.policyTitle": { en: "Per-tool policy", zh: "按工具的策略" },
    "mcp.policyHint": {
      en: "A denied tool is never attached, so the model cannot see or call it; an asked tool stays but every call waits for your approval, even in a workspace that approves nothing else. Deny beats ask beats allow, and either half of a rule may be *.",
      zh: "被拒绝的工具根本不会挂载，模型看不到也调不到；被标记为“询问”的工具会保留，但每次调用都要你批准，即使该工作区不做其它审批。拒绝 > 询问 > 允许，规则的任一侧都可以是 *。",
    },
    "mcp.policyDefault": { en: "Anything else", zh: "其余情况" },
    "mcp.effectAllow": { en: "Allow", zh: "允许" },
    "mcp.effectAsk": { en: "Ask", zh: "询问" },
    "mcp.effectDeny": { en: "Deny", zh: "拒绝" },
    "mcp.policyServer": { en: "server or *", zh: "服务器或 *" },
    "mcp.policyTool": { en: "tool or *", zh: "工具或 *" },
    "mcp.policyAdd": { en: "Add rule", zh: "新增规则" },
    "search.placeholder": { en: "Search chats…", zh: "搜索会话…" },
    "search.aria": { en: "Search chats", zh: "搜索会话" },
    "composer.placeholder": {
      en: "Send a message\n/ = commands & skills · @ = file mentions · Enter = send (queued while streaming) · Shift+Enter = newline",
      zh: "发送消息\n/ = 命令与技能 · @ = 文件提及 · Enter = 发送（流式输出时排队） · Shift+Enter = 换行",
    },
    "session.pin": { en: "Pin session", zh: "置顶会话" },
    "session.unpin": { en: "Unpin session", zh: "取消置顶" },
    "session.rename": { en: "Rename", zh: "重命名" },
    "session.name": { en: "Chat name", zh: "会话名称" },
    "session.fork": { en: "Fork session", zh: "分叉会话" },
    "session.forkEmpty": {
      en: "this chat has no turns to fork yet",
      zh: "本会话还没有可复制的对话轮次",
    },
    "session.archive": { en: "Archive session", zh: "归档会话" },
    "session.unarchive": { en: "Restore session", zh: "恢复会话" },
    "session.restore": { en: "Restore", zh: "恢复" },
    "session.archived": { en: "Archived", zh: "已归档" },
    "session.delete": { en: "Delete session", zh: "删除会话" },
    "session.actions": { en: "Chat actions", zh: "会话操作" },
    "ws.new": { en: "New session", zh: "新建会话" },
    "ws.rename": { en: "Rename workspace", zh: "重命名工作区" },
    "ws.delete": { en: "Delete workspace", zh: "删除工作区" },
    "ws.actions": { en: "Workspace actions", zh: "工作区操作" },
    "skill.inherit": { en: "Follow the workspace", zh: "跟随工作区" },
    "skill.scopeOwn": { en: "This chat carries its own list", zh: "本会话使用自己的列表" },
    "skill.scopeReading": { en: "Reading this chat's skills…", zh: "正在读取本会话的技能…" },
    "skill.scopeInherit": { en: "This chat follows the workspace", zh: "本会话跟随工作区" },
    "skill.scopeNoChat": {
      en: "Open a chat to switch a skill on for it.",
      zh: "打开一个会话，才能为它单独开关技能。",
    },
    "skill.scopeCount": { en: "skills", zh: "个技能" },
    "skill.scopeOf": { en: "of", zh: "/" },
    "skill.on": { en: "On in this chat", zh: "本会话已启用" },
    "skill.off": { en: "Off in this chat", zh: "本会话未启用" },
    "skill.onAria": { en: "In this chat", zh: "在本会话中" },
    "skill.toggleDenied": {
      en: "This workspace switched the skill off.",
      zh: "工作区已关闭该技能。",
    },
    "shortcuts.title": { en: "Keyboard shortcuts", zh: "键盘快捷键" },
    "shortcuts.open": { en: "View shortcuts", zh: "查看快捷键" },
    "shortcuts.row": { en: "Keyboard shortcuts", zh: "键盘快捷键" },
    "shortcuts.rowHint": {
      en: "Every key this interface listens for",
      zh: "界面实际响应的全部按键",
    },
    "shortcuts.note": {
      en: "Shortcuts apply where they make sense: the composer keeps Enter, and pane resizing needs a focused divider.",
      zh: "快捷键只在适用处生效：输入框保留 Enter，分隔条需先获得焦点才可调整。",
    },
    "shortcuts.close": { en: "Close", zh: "关闭" },
    "theme.system": { en: "Theme: follow system", zh: "主题：跟随系统" },
    "theme.light": { en: "Theme: light", zh: "主题：浅色" },
    "theme.dark": { en: "Theme: dark", zh: "主题：深色" },
    "lang.toggle": { en: "Switch to Chinese", zh: "切换到英文" },
  };

  function currentLang() {
    const marked = document.documentElement.lang || "";
    if (/^zh/i.test(marked)) return "zh";
    return "en";
  }

  /* Translate one key. A key with no Chinese entry returns its English text, and
   * an unknown key returns "" so callers can keep what they already have. */
  function t(key) {
    const entry = STRINGS[key];
    if (!entry) return "";
    return entry[currentLang()] || entry.en || "";
  }

  const MARKS = [
    ["data-i18n", (el, text) => { if (text) el.textContent = text; }],
    ["data-i18n-placeholder", (el, text) => { if (text) el.placeholder = text; }],
    ["data-i18n-aria", (el, text) => { if (text) el.setAttribute("aria-label", text); }],
    ["data-i18n-title", (el, text) => { if (text) el.title = text; }],
  ];

  function readShipped(el, attr) {
    if (attr === "data-i18n") return el.textContent;
    if (attr === "data-i18n-placeholder") return el.placeholder;
    if (attr === "data-i18n-aria") return el.getAttribute("aria-label") || "";
    return el.title;
  }

  /* The first pass remembers each element's shipped text, so a key that has no
   * translation in the active language restores English rather than the key. */
  const shipped = new WeakMap();
  function applyLang() {
    for (const [attr, write] of MARKS) {
      for (const el of document.querySelectorAll("[" + attr + "]")) {
        const key = el.getAttribute(attr);
        if (!key) continue;
        let memory = shipped.get(el);
        if (!memory) {
          memory = {};
          shipped.set(el, memory);
        }
        if (!(attr in memory)) memory[attr] = readShipped(el, attr);
        write(el, t(key) || memory[attr]);
      }
    }
    document.documentElement.dataset.lang = currentLang();
    syncToggles();
    return currentLang();
  }

  /* `shell.js` publishes `const Shell`. A top-level `const` in a classic script
   * creates a global *lexical* binding: every other classic script can read the
   * bare identifier, but it is deliberately not a property of `window`. Both
   * are accepted so this module also works where a harness wraps or re-exports
   * it — and getting this wrong is silent, because the fallback below still
   * paints the right colours while bypassing the shell's persistence. */
  function shellApi() {
    if (typeof Shell !== "undefined" && Shell) return Shell;
    return window.Shell || null;
  }

  function shellHas(method) {
    const api = shellApi();
    return !!api && typeof api[method] === "function";
  }

  /* ---------- appearance and language ------------------------------------ */

  const THEME_MODES = ["light", "dark", "system"];
  const THEME_GLYPH = { system: "◐", light: "☀", dark: "☾" };

  /* The shell writes `data-appearance`; reading it means the header button and
   * the Settings select can never hold different values. */
  function themeMode() {
    const mode = document.documentElement.dataset.appearance;
    return THEME_MODES.indexOf(mode) >= 0 ? mode : "system";
  }

  function setTheme(mode) {
    const chosen = THEME_MODES.indexOf(mode) >= 0 ? mode : "system";
    if (shellHas("setAppearance")) {
      shellApi().setAppearance(chosen);
    } else {
      /* Loaded without the shell (a fixture): still honour the choice for this
       * session rather than silently doing nothing. */
      const root = document.documentElement;
      root.dataset.appearance = chosen;
      const dark =
        chosen === "system"
          ? !!window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches
          : chosen === "dark";
      root.dataset.theme = dark ? "dark" : "light";
    }
    syncToggles();
    return chosen;
  }

  function cycleTheme() {
    const next = THEME_MODES[(THEME_MODES.indexOf(themeMode()) + 1) % THEME_MODES.length];
    return setTheme(next);
  }

  function switchLanguage() {
    const next = currentLang() === "zh" ? "en" : "zh";
    if (shellHas("setLanguage")) {
      shellApi().setLanguage(next);
    } else {
      document.documentElement.lang = next === "zh" ? "zh-CN" : "en";
    }
    /* Apply immediately as well: the observer below covers the shell's own
     * path, and this covers a fixture with no shell loaded. */
    applyLang();
    return next;
  }

  function syncToggles() {
    const themeButton = document.getElementById("theme-toggle");
    if (themeButton) {
      const mode = themeMode();
      themeButton.textContent = THEME_GLYPH[mode];
      themeButton.setAttribute("data-theme-mode", mode);
      const label = t("theme." + mode) || mode;
      themeButton.setAttribute("aria-label", label);
      themeButton.title = label;
    }
    const langButton = document.getElementById("lang-toggle");
    if (langButton) {
      const lang = currentLang();
      langButton.textContent = lang === "zh" ? "中" : "EN";
      langButton.setAttribute("data-lang", lang);
      const label = t("lang.toggle") || "Language";
      langButton.setAttribute("aria-label", label);
      langButton.title = label;
    }
  }

  /* ---------- shortcut reference ----------------------------------------- */

  /* Only keys the interface actually listens for. Each entry was read off the
   * implementation (composer, palette, tabs, dividers, sidebar), so this list
   * cannot drift into fiction the way a hand-written cheat sheet does. */
  const SHORTCUTS = [
    { group: "Composer", keys: "Enter", action: "Send the message" },
    { group: "Composer", keys: "Shift+Enter", action: "Insert a newline" },
    { group: "Composer", keys: "↑ / ↓", action: "Walk back through previous prompts" },
    { group: "Composer", keys: "Cmd/Ctrl+Enter", action: "Save an edit in the message editor" },
    { group: "Commands & mentions", keys: "↑ / ↓", action: "Move through the open list" },
    { group: "Commands & mentions", keys: "Enter or Tab", action: "Accept the highlighted entry" },
    { group: "Commands & mentions", keys: "Escape", action: "Dismiss the list" },
    { group: "Views", keys: "← / →", action: "Move between Chat and Trajectory" },
    { group: "Views", keys: "Home / End", action: "First or last view" },
    { group: "Views", keys: "Cmd/Ctrl+B", action: "Show or hide the sidebar" },
    { group: "Sidebar", keys: "Cmd/Ctrl+N", action: "New chat" },
    { group: "Sidebar", keys: "Cmd/Ctrl+G", action: "Rename the open chat" },
    { group: "Sidebar", keys: "Cmd/Ctrl+F", action: "Fork the open chat" },
    { group: "Sidebar", keys: "Cmd/Ctrl+Shift+A", action: "Archive the open chat" },
    { group: "Views", keys: "Escape", action: "Close Settings" },
    { group: "Plan", keys: "Alt+↑ / Alt+↓", action: "Move the step being edited" },
    { group: "Panes", keys: "← / →", action: "Resize the focused divider" },
    { group: "Panes", keys: "Shift+← / Shift+→", action: "Resize by a larger step" },
    { group: "Panes", keys: "Home", action: "Reset the pane to its default width" },
    { group: "Help", keys: "?", action: "Open this reference" },
    { group: "Help", keys: "Escape", action: "Close this reference" },
  ];

  let opener = null;
  let booted = false;

  function buildShortcuts() {
    let modal = document.getElementById("shortcuts-modal");
    if (modal) return modal;
    modal = document.createElement("div");
    modal.id = "shortcuts-modal";
    modal.className = "modal hidden";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-labelledby", "shortcuts-title");

    const card = document.createElement("div");
    card.className = "modal-card shortcuts-card";

    const head = document.createElement("div");
    head.className = "shortcuts-head";
    const title = document.createElement("h2");
    title.id = "shortcuts-title";
    title.setAttribute("data-i18n", "shortcuts.title");
    title.textContent = t("shortcuts.title") || "Keyboard shortcuts";
    const close = document.createElement("button");
    close.type = "button";
    close.id = "shortcuts-close";
    close.className = "mini-btn";
    close.textContent = "✕";
    close.setAttribute("data-i18n-aria", "shortcuts.close");
    close.setAttribute("aria-label", t("shortcuts.close") || "Close");
    close.addEventListener("click", () => closeShortcuts());
    head.appendChild(title);
    head.appendChild(close);

    const body = document.createElement("div");
    body.id = "shortcuts-body";
    body.className = "shortcuts-body";
    let group = "";
    for (const entry of SHORTCUTS) {
      if (entry.group !== group) {
        group = entry.group;
        const heading = document.createElement("h3");
        heading.className = "shortcuts-group";
        heading.textContent = group;
        body.appendChild(heading);
      }
      const line = document.createElement("div");
      line.className = "shortcut-row";
      const keys = document.createElement("span");
      keys.className = "shortcut-keys";
      keys.textContent = entry.keys;
      const action = document.createElement("span");
      action.className = "shortcut-action";
      action.textContent = entry.action;
      line.appendChild(keys);
      line.appendChild(action);
      body.appendChild(line);
    }

    const note = document.createElement("p");
    note.className = "shortcuts-note";
    note.setAttribute("data-i18n", "shortcuts.note");
    note.textContent = t("shortcuts.note") || "";

    card.appendChild(head);
    card.appendChild(body);
    card.appendChild(note);
    modal.appendChild(card);
    modal.addEventListener("click", (event) => {
      if (event.target === modal) closeShortcuts();
    });
    document.body.appendChild(modal);
    return modal;
  }

  function shortcutsOpen() {
    const modal = document.getElementById("shortcuts-modal");
    return !!modal && !modal.classList.contains("hidden");
  }

  function openShortcuts() {
    const modal = buildShortcuts();
    applyLang();
    if (!shortcutsOpen()) {
      /* Remember what opened it so closing hands the keyboard back, which is
       * the same contract the settings dialog keeps. */
      opener =
        document.activeElement && document.activeElement !== document.body
          ? document.activeElement
          : document.getElementById("open-settings");
    }
    modal.classList.remove("hidden");
    const close = modal.querySelector("#shortcuts-close");
    if (close) close.focus();
    return modal;
  }

  function closeShortcuts() {
    const modal = document.getElementById("shortcuts-modal");
    if (!modal || modal.classList.contains("hidden")) return false;
    modal.classList.add("hidden");
    const back =
      opener && opener.getClientRects && opener.getClientRects().length > 0 ? opener : null;
    if (back && typeof back.focus === "function") back.focus();
    opener = null;
    return true;
  }

  /* A keystroke belongs to a field when it would otherwise be text input. */
  function inField(target) {
    if (!target) return false;
    if (target.isContentEditable === true) return true;
    return ["INPUT", "TEXTAREA", "SELECT"].indexOf(target.tagName) >= 0;
  }

  /* ---------- boot ------------------------------------------------------- */

  function boot() {
    applyLang();
    if (booted) {
      /* A second call (after DOMContentLoaded) only re-syncs: the listeners
       * below must not be installed twice, or `?` would open and immediately
       * close the reference. */
      syncToggles();
      return;
    }
    booted = true;

    const themeButton = document.getElementById("theme-toggle");
    if (themeButton && !themeButton.dataset.bound) {
      themeButton.dataset.bound = "1";
      themeButton.addEventListener("click", () => cycleTheme());
    }
    const langButton = document.getElementById("lang-toggle");
    if (langButton && !langButton.dataset.bound) {
      langButton.dataset.bound = "1";
      langButton.addEventListener("click", () => switchLanguage());
    }

    /* The shell owns both preferences; watching the attributes it writes means
     * its Settings rows and these header buttons can never disagree. */
    if (window.MutationObserver) {
      new MutationObserver(() => applyLang()).observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["lang", "data-appearance", "data-theme"],
      });
    }

    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && shortcutsOpen()) {
        event.preventDefault();
        closeShortcuts();
        return;
      }
      const isHelp = event.key === "?" || (event.key === "/" && event.shiftKey);
      if (!isHelp) return;
      if (inField(event.target) || event.metaKey || event.ctrlKey || event.altKey) return;
      /* Never stack a second dialog on top of an open one: a reference card is
         helpful, a reference card over Settings is two Escape targets and no
         clear way back. The event is left alone so the open dialog can act. */
      if (document.querySelector(".modal:not(.hidden):not(#shortcuts-modal)")) return;
      event.preventDefault();
      if (shortcutsOpen()) closeShortcuts();
      else openShortcuts();
    });

    /* A live region for the stream state: the chip already says idle/streaming,
     * so announcing it is free, and the transcript itself stays unannounced
     * (a screen reader reading every token would be unusable). */
    const chip = document.getElementById("stream-chip");
    if (chip && !chip.hasAttribute("aria-live")) chip.setAttribute("aria-live", "polite");

    syncToggles();
  }

  window.Polish = {
    t,
    lang: currentLang,
    setLang(code) {
      const next = code === "zh" ? "zh" : "en";
      if (shellHas("setLanguage")) {
        shellApi().setLanguage(next);
        applyLang();
      } else {
        document.documentElement.lang = next === "zh" ? "zh-CN" : "en";
        applyLang();
      }
      return next;
    },
    applyLang,
    t,
    theme: themeMode,
    setTheme,
    cycleTheme,
    openShortcuts,
    closeShortcuts,
    shortcutsOpen,
    SHORTCUTS,
    STRINGS,
  };

  /* The script is the last element in the body, so everything it binds already
   * exists when it runs — booting immediately (rather than waiting for
   * DOMContentLoaded) is what keeps a load-time harness able to see the
   * controls work. The listener is a safety net for a page that moves this
   * script into the head, and boot() is idempotent. */
  boot();
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  }
})();
