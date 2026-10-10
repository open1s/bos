/* BOS workbench shell: three-panel layout, design tokens, task state and the
   document panel. Deliberately free of Tauri and of agent state, so the shell
   can be tested on its own and a task's lifetime never depends on a view.

   Part II §15 of docs/gui-requirements.md: UI components must not own agent
   scheduling, model requests or persistence of business data. This module owns
   only presentation, layout and UI-local preferences. */

"use strict";

const Shell = (() => {
  const LAYOUT_KEY = "bos.layout.v1";
  const UI_KEY = "bos.ui.v1";

  const SIDEBAR_MIN = 264;
  const SIDEBAR_MAX = 420;
  const SIDEBAR_DEFAULT = 280;
  const SIDEBAR_RAIL = 56;
  /* Right-column and frame geometry, cloned from DSH's ui-layout contract (see
     docs/dsh-ui-reference/dsh-client-ui-layout/README.md): the sidebar spans
     264-420px at a 280px default with a 56px rail, the right column opens at 45%
     of the viewport and never exceeds 70%, the centre keeps 400px, and below
     1024px the sidebar auto-collapses. */
  const DOC_DEFAULT_RATIO = 0.45;
  const DOC_MAX_RATIO = 0.7;
  const DOC_MIN = 280;
  /* DSH reduces the right column to 300px before it reports that there is no
     room at all; a narrower column is where the occupant closes instead. */
  const DOC_STEP = 300;
  const CENTER_MIN = 400;
  /* One breakpoint, both rules: below it the right column is not offered and the
     sidebar auto-collapses (DSH couples the two). */
  const COMPACT_AT = 1024;

  const STATUSES = [
    "Idle",
    "Running",
    "WaitingForApproval",
    "Completed",
    "Failed",
    "Cancelled",
  ];
  const TERMINAL = new Set(["Completed", "Failed", "Cancelled"]);

  const APPEARANCES = ["light", "dark", "system"];
  const WORK_DETAILS = ["compact", "standard", "detailed", "verbose"];
  const PERMISSIONS = ["read-only", "workspace-write", "danger-full-access"];

  /* ---------- storage (UI-local; a failure here must never break the UI) --- */

  /* UI-local preferences live in localStorage where the origin provides it (the
     app's webview does). Where storage is unavailable — a sandboxed frame, a
     private window, a locked-down embed — the same contract is served from
     memory, so the shell keeps working and only cross-restart persistence is
     lost. One seam, probed once. */
  const memoryStore = new Map();
  const durable = (() => {
    try {
      window.localStorage.setItem("bos.storage.probe", "1");
      window.localStorage.removeItem("bos.storage.probe");
      return true;
    } catch (err) {
      return false;
    }
  })();

  const storage = {
    durable,
    get(key) {
      if (memoryStore.has(key)) return memoryStore.get(key);
      if (!durable) return null;
      try {
        return window.localStorage.getItem(key);
      } catch (err) {
        return null;
      }
    },
    set(key, value) {
      memoryStore.set(key, value);
      if (!durable) return false;
      try {
        window.localStorage.setItem(key, value);
        return true;
      } catch (err) {
        return false;
      }
    },
  };

  const readJson = (key) => {
    try {
      return JSON.parse(storage.get(key) || "null") || {};
    } catch (err) {
      return {};
    }
  };
  const writeJson = (key, value) => storage.set(key, JSON.stringify(value));

  const clamp = (n, lo, hi) => Math.min(hi, Math.max(lo, n));
  const isOneOf = (v, allowed, fallback) =>
    allowed.indexOf(v) >= 0 ? v : fallback;

  /* ---------- state ------------------------------------------------------ */

  const savedLayout = readJson(LAYOUT_KEY);
  const savedUi = readJson(UI_KEY);

  const state = {
    sidebar: SIDEBAR_DEFAULT,
    doc: 0, // 0 = derive from the ratio
    sidebarCollapsed: false,
    docOpen: true,
    appearance: isOneOf(savedUi.appearance, APPEARANCES, "system"),
    fontSize: clamp(Number(savedUi.fontSize) || 14, 10, 22),
    workDetails: isOneOf(savedUi.workDetails, WORK_DETAILS, "detailed"),
    codingView: savedUi.codingView !== false,
    language: isOneOf(savedUi.language, ["en", "zh"], "en"),
    permission: isOneOf(savedUi.permission, PERMISSIONS, "workspace-write"),
    task: { taskId: null, status: "Idle", seq: -1, startedAt: 0, endedAt: 0, phase: "" },
    context: { workspace: "", model: "", session: "", detail: "" },
  };

  /* One clamping policy for geometry, whether it arrives at boot or when the
     workspace scope swaps under it. */
  const adoptLayout = (saved) => {
    state.sidebar = clamp(Number(saved.sidebar) || SIDEBAR_DEFAULT, SIDEBAR_MIN, SIDEBAR_MAX);
    state.doc = Number(saved.doc) || 0;
    state.sidebarCollapsed = saved.sidebarCollapsed === true;
    state.docOpen = saved.docOpen !== false;
  };
  adoptLayout(savedLayout);

  const listeners = new Map();
  const emit = (name, payload) => {
    for (const fn of listeners.get(name) || []) {
      try {
        fn(payload);
      } catch (err) {
        /* a listener must not break the shell */
      }
    }
  };

  let app = null;
  let sidebar = null;
  let main = null;
  let docPanel = null;
  let docBody = null;
  let docTabs = null;
  let docPath = null;
  const documents = [];
  let activeDoc = null;

  /* ---------- workbench state machine (spec §12.3, GUI-05) --------------- */

  const setTaskStatus = (status) => {
    if (STATUSES.indexOf(status) < 0 || status === state.task.status) return;
    state.task.status = status;
    renderTaskStatus();
    emit("task", { ...state.task });
  };

  /* Applies one backend event. `seq` is monotonically increasing per task, so
     duplicates and out-of-order arrivals cannot regress state, and terminal
     states stay terminal until a new task starts. */
  const applyTaskEvent = (ev) => {
    if (!ev || typeof ev.seq !== "number") return false;
    const t = state.task;
    if (ev.taskId && ev.taskId !== t.taskId) {
      t.taskId = ev.taskId;
      t.seq = -1;
      t.status = "Idle";
    }
    if (ev.seq <= t.seq) return false; // duplicate or stale
    const next = {
      started: "Running",
      progress: "Running",
      approval: "WaitingForApproval",
      completed: "Completed",
      failed: "Failed",
      cancelled: "Cancelled",
    }[ev.type];
    if (!next) return false;
    if (TERMINAL.has(t.status) && ev.type !== "started") return false;
    if (t.status === "Idle" && ev.type === "started") t.startedAt = ev.timestamp || 0;
    if (TERMINAL.has(next)) t.endedAt = ev.timestamp || 0;
    t.seq = ev.seq;
    t.phase = ev.phase || "";
    if (next !== t.status) setTaskStatus(next);
    else emit("task", { ...t });
    return true;
  };

  /* ---------- theme, typography, work-details --------------------------- */

  const systemDark = () =>
    !!window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;

  const resolvedAppearance = () =>
    state.appearance === "system" ? (systemDark() ? "dark" : "light") : state.appearance;

  const applyUi = () => {
    const root = document.documentElement;
    const scheme = resolvedAppearance();
    root.dataset.theme = scheme;
    root.dataset.appearance = state.appearance;
    root.dataset.workDetails = state.workDetails;
    root.dataset.codingView = state.codingView ? "on" : "off";
    root.lang = state.language === "zh" ? "zh-CN" : "en";
    root.style.setProperty("--content-font-size", `${state.fontSize}px`);
    /* Colour-scheme and the dark marker are what the platform chrome (scrollbars,
       form controls, `color-scheme` UA styles) reads; DSH's theme presenter
       writes both, and the marker is the name DSH's own styles select on. */
    root.style.colorScheme = scheme;
    if (document.body) {
      if (scheme === "dark") document.body.dataset.dsDarkTheme = "";
      else delete document.body.dataset.dsDarkTheme;
    }
    writeJson(UI_KEY, {
      appearance: state.appearance,
      fontSize: state.fontSize,
      workDetails: state.workDetails,
      codingView: state.codingView,
      language: state.language,
      permission: state.permission,
    });
    syncControls();
    syncThemeColor();
    emit("ui", { ...state });
  };

  /* One owned `<meta name="theme-color">` whose content follows the landed body
     background — the resolved theme is the single colour authority, so the
     meta is read from the rendered result rather than recomputed (DSH ui-layout
     theme presenter). */
  const syncThemeColor = () => {
    if (!document.body) return;
    let meta = document.querySelector('meta[name="theme-color"]');
    if (!meta) {
      meta = document.createElement("meta");
      meta.setAttribute("name", "theme-color");
      document.head.appendChild(meta);
    }
    const bg = getComputedStyle(document.body).backgroundColor;
    if (bg) meta.setAttribute("content", bg);
  };

  const setAppearance = (v) => {
    state.appearance = isOneOf(v, APPEARANCES, state.appearance);
    applyUi();
  };
  const setFontSize = (n) => {
    const size = clamp(Math.round(Number(n) || state.fontSize), 10, 22);
    if (size === state.fontSize) return;
    state.fontSize = size;
    applyUi();
  };
  const setWorkDetails = (v) => {
    state.workDetails = isOneOf(v, WORK_DETAILS, state.workDetails);
    applyUi();
  };
  const setCodingView = (on) => {
    state.codingView = !!on;
    applyUi();
  };
  const setLanguage = (v) => {
    state.language = isOneOf(v, ["en", "zh"], state.language);
    applyUi();
  };
  const setPermission = (v) => {
    state.permission = isOneOf(v, PERMISSIONS, state.permission);
    applyUi();
  };

  /* ---------- layout ---------------------------------------------------- */

  /* The geometry is per workspace: two projects on two monitors should not
     inherit each other's column widths. The shell stays Tauri-free, so the
     app tells it the scope (the workspace root) and the shell keeps the
     storage seam to itself; the unscoped key stays the "default" scope, so
     whatever was saved before scoping existed is still the layout you get
     when no workspace is configured. */
  let layoutScope = "";
  const layoutKey = () => (layoutScope ? `${LAYOUT_KEY}:${layoutScope}` : LAYOUT_KEY);

  const appWidth = () => (app ? app.clientWidth : 0);
  const sidebarWidth = () =>
    state.sidebarCollapsed ? Math.min(SIDEBAR_RAIL, state.sidebar) : state.sidebar;

  /* The right column's width, and whether it fits at all. The stored width is a
     preference, not a right: when it would squeeze the centre below its floor
     the column steps down to 300px, and when even that does not fit the caller
     is told there is no room (DSH's deterministic close, never an automatic
     reopen on a wider window). */
  const docCap = () =>
    Math.max(
      DOC_MIN,
      Math.min(appWidth() * DOC_MAX_RATIO, appWidth() - sidebarWidth() - CENTER_MIN - 16),
    );

  const docGeometry = () => {
    const width = appWidth();
    const want = state.doc || Math.round(width * DOC_DEFAULT_RATIO);
    const cap = docCap();
    if (want <= cap) return { width: Math.round(clamp(want, DOC_MIN, cap)), canShow: true };
    if (cap >= DOC_STEP) return { width: Math.round(cap), canShow: true };
    return { width: 0, canShow: false };
  };
  const docWidthFor = () => docGeometry().width;

  const persistLayout = () =>
    writeJson(layoutKey(), {
      sidebar: state.sidebar,
      doc: state.doc,
      sidebarCollapsed: state.sidebarCollapsed,
      docOpen: state.docOpen,
    });

  const applyLayout = () => {
    if (!app) return;
    const width = appWidth();
    const geo = docGeometry();
    /* Not enough room for the column at any width: it closes deterministically,
       and widening the window later does not reopen it (DSH ui-layout). */
    if (state.docOpen && width >= COMPACT_AT && !geo.canShow) {
      state.docOpen = false;
      persistLayout();
    }
    const showDoc = state.docOpen && width >= COMPACT_AT && geo.canShow;
    const collapse = state.sidebarCollapsed || width < COMPACT_AT;
    app.dataset.doc = showDoc ? "open" : "closed";
    app.dataset.sidebar = collapse ? "rail" : "expanded";
    app.style.setProperty("--sidebar-w", `${collapse ? SIDEBAR_RAIL : state.sidebar}px`);
    app.style.setProperty("--doc-w", `${showDoc ? geo.width : 0}px`);
    const left = document.getElementById("divider-left");
    const right = document.getElementById("divider-right");
    if (left) {
      left.setAttribute("aria-valuenow", String(collapse ? SIDEBAR_RAIL : state.sidebar));
      left.hidden = collapse;
    }
    if (right) {
      right.setAttribute("aria-valuenow", String(showDoc ? geo.width : 0));
      right.hidden = !showDoc;
    }
    if (docPanel) docPanel.hidden = !showDoc;
    const docToggle = document.getElementById("doc-toggle");
    if (docToggle) docToggle.setAttribute("aria-pressed", String(showDoc));
    const railToggle = document.getElementById("sidebar-toggle");
    if (railToggle) railToggle.setAttribute("aria-pressed", String(!collapse));
    emit("layout", { sidebar: state.sidebar, doc: showDoc ? geo.width : 0, open: showDoc, collapse, tight: !geo.canShow });
  };

  const setSidebar = (px) => {
    state.sidebar = clamp(Math.round(px), SIDEBAR_MIN, SIDEBAR_MAX);
    persistLayout();
    applyLayout();
  };
  const setDoc = (px) => {
    state.doc = Math.round(clamp(px, DOC_MIN, docCap()));
    persistLayout();
    applyLayout();
  };
  const toggleSidebar = () => {
    state.sidebarCollapsed = !state.sidebarCollapsed;
    persistLayout();
    applyLayout();
  };
  const toggleDoc = () => {
    state.docOpen = !state.docOpen;
    openRightColumn();
    persistLayout();
    applyLayout();
  };
  /* DSH: opening the right column concedes the space from the sidebar first, so
     a manually expanded sidebar collapses rather than squeezing the centre. */
  const openRightColumn = () => {
    if (state.docOpen && !state.sidebarCollapsed) state.sidebarCollapsed = true;
  };

  /* Swap to another workspace's geometry: the geometry on screen still
     belongs to the scope being left, so it is written back there first, then
     the new scope's saved layout is adopted (defaults when it has none). */
  const setLayoutScope = (scope) => {
    const next = String(scope || "");
    if (next === layoutScope) return;
    persistLayout();
    layoutScope = next;
    adoptLayout(readJson(layoutKey()));
    applyLayout();
  };

  const drag = (divider, onMove, onEnd) => {
    divider.addEventListener("pointerdown", (ev) => {
      ev.preventDefault();
      divider.setPointerCapture(ev.pointerId);
      divider.classList.add("dragging");
      document.body.classList.add("resizing");
      const move = (e) => onMove(e);
      const up = (e) => {
        divider.releasePointerCapture?.(e.pointerId);
        divider.classList.remove("dragging");
        document.body.classList.remove("resizing");
        divider.removeEventListener("pointermove", move);
        divider.removeEventListener("pointerup", up);
        divider.removeEventListener("pointercancel", up);
        persistLayout();
        onEnd?.();
      };
      divider.addEventListener("pointermove", move);
      divider.addEventListener("pointerup", up);
      divider.addEventListener("pointercancel", up);
    });
    divider.addEventListener("keydown", (ev) => {
      const step = ev.shiftKey ? 48 : 16;
      if (ev.key === "ArrowLeft") {
        ev.preventDefault();
        onMove({ clientX: divider.getBoundingClientRect().left - step });
      } else if (ev.key === "ArrowRight") {
        ev.preventDefault();
        onMove({ clientX: divider.getBoundingClientRect().left + step });
      } else if (ev.key === "Home") {
        ev.preventDefault();
        onEnd?.("reset");
      }
    });
  };

  /* ---------- document panel -------------------------------------------- */

  const renderDocTabs = () => {
    if (!docTabs) return;
    docTabs.textContent = "";
    const all = [{ id: "start", title: "Start", path: "" }, ...documents];
    all.forEach((doc) => {
      const tab = document.createElement("button");
      tab.type = "button";
      tab.className = "doc-tab" + ((activeDoc || "start") === doc.id ? " active" : "");
      tab.setAttribute("role", "tab");
      tab.setAttribute("aria-selected", String((activeDoc || "start") === doc.id));
      tab.appendChild(document.createTextNode(doc.title || doc.id));
      if (doc.id !== "start") {
        const close = document.createElement("span");
        close.className = "doc-close";
        close.textContent = "\u00d7";
        close.setAttribute("role", "button");
        close.setAttribute("aria-label", `Close ${doc.title || doc.id}`);
        close.addEventListener("click", (ev) => {
          ev.stopPropagation();
          closeDocument(doc.id);
        });
        tab.appendChild(close);
      }
      tab.addEventListener("click", () => {
        activeDoc = doc.id === "start" ? null : doc.id;
        renderDoc();
      });
      docTabs.appendChild(tab);
    });
  };

  const renderStart = () => {
    const box = document.createElement("div");
    box.className = "doc-start";
    const add = (label, value) => {
      const row = document.createElement("div");
      row.className = "doc-row";
      const k = document.createElement("span");
      k.className = "doc-key";
      k.textContent = label;
      const v = document.createElement("span");
      v.className = "doc-value";
      // Untrusted values are inserted as text, never as markup.
      v.textContent = value || "\u2014";
      row.appendChild(k);
      row.appendChild(v);
      box.appendChild(row);
    };
    add("Workspace", state.context.workspace);
    add("Session", state.context.session);
    add("Model", state.context.model);
    add("Task", state.task.status + (state.task.phase ? ` \u00b7 ${state.task.phase}` : ""));
    const hint = document.createElement("p");
    hint.className = "doc-hint";
    hint.textContent =
      "No document open. Open one from a tool call, a file mention or the file list.";
    box.appendChild(hint);
    return box;
  };

  const renderDoc = () => {
    renderDocTabs();
    if (!docBody) return;
    docBody.textContent = "";
    const doc = documents.find((d) => d.id === activeDoc);
    if (!doc) {
      if (docPath) docPath.textContent = "";
      docBody.appendChild(renderStart());
      return;
    }
    if (docPath) docPath.textContent = doc.path || "";
    if (doc.error) {
      const err = document.createElement("div");
      err.className = "doc-error";
      err.textContent = doc.error;
      docBody.appendChild(err);
      return;
    }
    const pre = document.createElement("pre");
    pre.className = "doc-text";
    pre.textContent = doc.text || ""; // inert: document text is untrusted
    docBody.appendChild(pre);
  };

  const openDocument = (doc) => {
    if (!doc || !doc.id) return;
    const entry = {
      id: doc.id,
      title: doc.title || doc.path || doc.id,
      path: doc.path || "",
      text: doc.text || "",
      error: doc.error || "",
    };
    const at = documents.findIndex((d) => d.id === entry.id);
    if (at >= 0) documents[at] = entry;
    else documents.push(entry);
    activeDoc = entry.id;
    state.docOpen = true;
    openRightColumn();
    applyLayout();
    renderDoc();
    emit("document", { action: "open", id: entry.id });
  };

  const closeDocument = (id) => {
    const at = documents.findIndex((d) => d.id === id);
    if (at < 0) return;
    documents.splice(at, 1);
    if (activeDoc === id) activeDoc = documents.length ? documents[documents.length - 1].id : null;
    renderDoc();
    emit("document", { action: "close", id });
  };

  /* ---------- rendering of shell-owned chrome --------------------------- */

  const renderTaskStatus = () => {
    const chip = document.getElementById("stream-chip");
    if (!chip) return;
    const t = state.task;
    const labels = { Idle: "idle", Running: "running", WaitingForApproval: "approval" };
    const label = labels[t.status] || t.status.toLowerCase();
    // The running detail (elapsed seconds, other streaming chats) is supplied
    // by the application; terminal states show themselves.
    chip.textContent = t.status === "Running" && t.detail ? t.detail : label;
    chip.className =
      "chip " + (t.status === "Running" ? "streaming" : t.status.toLowerCase());
  };

  /* Live detail for the running state, e.g. "streaming \u00b7 12s". */
  const setTaskDetail = (text) => {
    state.task.detail = text || "";
    if (state.task.status === "Running") renderTaskStatus();
  };

  const mount = () => {
    app = document.getElementById("app");
    sidebar = document.getElementById("sidebar");
    main = document.getElementById("main");
    if (!app || !sidebar || !main) return false;

    // Dividers sit between the panes; the document panel joins after #main.
    const left = document.createElement("div");
    left.id = "divider-left";
    left.className = "divider";
    left.setAttribute("role", "separator");
    left.setAttribute("aria-orientation", "vertical");
    left.setAttribute("aria-label", "Resize sidebar");
    left.tabIndex = 0;
    sidebar.insertAdjacentElement("afterend", left);

    const right = document.createElement("div");
    right.id = "divider-right";
    right.className = "divider";
    right.setAttribute("role", "separator");
    right.setAttribute("aria-orientation", "vertical");
    right.setAttribute("aria-label", "Resize document panel");
    right.tabIndex = 0;
    main.insertAdjacentElement("afterend", right);

    docPanel = document.createElement("aside");
    docPanel.id = "doc-panel";
    docPanel.className = "doc-panel";
    docPanel.setAttribute("aria-label", "Documents");
    const head = document.createElement("header");
    head.className = "doc-head";
    docTabs = document.createElement("div");
    docTabs.id = "doc-tabs";
    docTabs.className = "doc-tabs";
    docTabs.setAttribute("role", "tablist");
    const actions = document.createElement("div");
    actions.className = "doc-actions";
    const refresh = document.createElement("button");
    refresh.id = "doc-refresh";
    refresh.type = "button";
    refresh.className = "btn btn-ghost";
    refresh.title = "Refresh document";
    refresh.textContent = "\u21bb";
    refresh.addEventListener("click", () => emit("document", { action: "refresh", id: activeDoc }));
    const hide = document.createElement("button");
    hide.id = "doc-collapse";
    hide.type = "button";
    hide.className = "btn btn-ghost";
    hide.title = "Hide document panel";
    hide.textContent = "\u00bb";
    hide.addEventListener("click", toggleDoc);
    actions.appendChild(refresh);
    actions.appendChild(hide);
    head.appendChild(docTabs);
    head.appendChild(actions);
    docPath = document.createElement("div");
    docPath.id = "doc-path";
    docPath.className = "doc-path";
    docBody = document.createElement("div");
    docBody.id = "doc-body";
    docBody.className = "doc-body";
    docPanel.appendChild(head);
    docPanel.appendChild(docPath);
    docPanel.appendChild(docBody);
    right.insertAdjacentElement("afterend", docPanel);

    // Pane toggles live in the centre toolbar so a narrow window never hides
    // navigation permanently (Part II §12.1 / GUI-01).
    const head0 = document.getElementById("chat-head");
    if (head0) {
      const railToggle = document.createElement("button");
      railToggle.id = "sidebar-toggle";
      railToggle.type = "button";
      railToggle.className = "btn btn-ghost head-toggle";
      railToggle.title = "Toggle sidebar";
      railToggle.textContent = "\u2630";
      railToggle.addEventListener("click", toggleSidebar);
      const docToggle = document.createElement("button");
      docToggle.id = "doc-toggle";
      docToggle.type = "button";
      docToggle.className = "btn btn-ghost head-toggle";
      docToggle.title = "Toggle document panel";
      docToggle.textContent = "\u25a4";
      docToggle.addEventListener("click", toggleDoc);
      head0.insertBefore(railToggle, head0.firstChild);
      head0.appendChild(docToggle);
    }

    drag(left, (ev) => setSidebar(ev.clientX - app.getBoundingClientRect().left), (why) => {
      if (why === "reset") setSidebar(SIDEBAR_DEFAULT);
    });
    drag(right, (ev) => setDoc(app.getBoundingClientRect().right - ev.clientX), (why) => {
      if (why === "reset") {
        state.doc = 0;
        applyLayout();
      }
    });

    window.addEventListener("resize", applyLayout);
    if (window.matchMedia) {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener?.("change", () => {
        if (state.appearance === "system") applyUi();
      });
    }
    renderDoc();
    return true;
  };

  /* ---------- general settings rows (spec §13.3) ------------------------ */

  const control = (tag, attrs, options, value, onChange) => {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (k === "text") node.textContent = v;
      else node.setAttribute(k, v);
    }
    if (tag === "select") {
      for (const opt of options) {
        const o = document.createElement("option");
        o.value = opt.value;
        o.textContent = opt.label;
        node.appendChild(o);
      }
      node.value = value;
      node.addEventListener("change", () => onChange(node.value));
    }
    return node;
  };

  let controlsBound = false;
  const syncControls = () => {
    if (!controlsBound) return;
    const appearance = document.getElementById("ui-appearance");
    const fontSize = document.getElementById("ui-font-size");
    const details = document.getElementById("ui-work-details");
    const coding = document.getElementById("ui-coding-view");
    const language = document.getElementById("ui-language");
    const permission = document.getElementById("ui-permission");
    if (appearance) appearance.value = state.appearance;
    if (fontSize) fontSize.value = String(state.fontSize);
    if (details) details.value = state.workDetails;
    if (coding) coding.checked = state.codingView;
    if (language) language.value = state.language;
    if (permission) permission.value = state.permission;
  };

  const mountSettings = () => {
    const form = document.getElementById("settings-form");
    if (!form || form.querySelector(".ui-general")) return false;
    const section = document.createElement("div");
    section.className = "cap-section ui-general";
    const title = document.createElement("div");
    title.className = "cap-title";
    title.textContent = "General";
    section.appendChild(title);

    const row = (label, hint, node, id) => {
      const wrap = document.createElement("label");
      wrap.className = "row ui-row";
      const text = document.createElement("span");
      text.className = "ui-row-label";
      text.textContent = label;
      if (hint) {
        const small = document.createElement("small");
        small.className = "ui-row-hint";
        small.textContent = hint;
        text.appendChild(small);
      }
      wrap.appendChild(text);
      if (id) node.id = id;
      wrap.appendChild(node);
      section.appendChild(wrap);
      return wrap;
    };

    row("Appearance", "Light, dark or follow the system", control("select", {}, [
      { value: "light", label: "Light" },
      { value: "dark", label: "Dark" },
      { value: "system", label: "System" },
    ], state.appearance, setAppearance), "ui-appearance");

    const size = control("input", { type: "number", min: "10", max: "22", step: "1" }, [], null, () => {});
    size.value = String(state.fontSize);
    size.addEventListener("change", () => setFontSize(size.value));
    row("Font size", "Conversation content only, 10\u201322 px", size, "ui-font-size");

    row("Work details", "How much tool-call detail the transcript shows", control("select", {}, [
      { value: "compact", label: "Compact" },
      { value: "standard", label: "Standard" },
      { value: "detailed", label: "Detailed" },
      { value: "verbose", label: "Verbose" },
    ], state.workDetails, setWorkDetails), "ui-work-details");

    const coding = control("input", { type: "checkbox" }, [], null, () => {});
    coding.checked = state.codingView;
    coding.addEventListener("change", () => setCodingView(coding.checked));
    row("Show coding view", "Diffs and tool output in the transcript", coding, "ui-coding-view");

    row("Language", "Interface language", control("select", {}, [
      { value: "en", label: "English" },
      { value: "zh", label: "\u4e2d\u6587" },
    ], state.language, setLanguage), "ui-language");

    row("Permission", "Default mode for new sessions", control("select", {}, [
      { value: "read-only", label: "Read only" },
      { value: "workspace-write", label: "Workspace write" },
      { value: "danger-full-access", label: "Full access" },
    ], state.permission, setPermission), "ui-permission");

    const shortcuts = document.createElement("button");
    shortcuts.type = "button";
    shortcuts.className = "btn btn-ghost";
    shortcuts.textContent = "Edit shortcuts";
    shortcuts.disabled = true;
    shortcuts.title = "Shortcut editing is not implemented yet";
    row("Keyboard shortcuts", "Not implemented yet", shortcuts, "ui-shortcuts");

    form.insertBefore(section, form.firstChild);
    controlsBound = true;
    syncControls();
    return true;
  };

  /* ---------- public surface -------------------------------------------- */

  const on = (name, fn) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name).add(fn);
    return () => listeners.get(name).delete(fn);
  };

  const setContext = (ctx) => {
    Object.assign(state.context, ctx || {});
    if (!activeDoc) renderDoc();
  };

  let mounted = false;
  const init = () => {
    if (mounted) return true;
    mounted = mount();
    if (!mounted) return false;
    applyUi();
    applyLayout();
    mountSettings();
    renderTaskStatus();
    return true;
  };

  return {
    init,
    on,
    state,
    storage,
    STATUSES,
    APPEARANCES,
    WORK_DETAILS,
    PERMISSIONS,
    applyTaskEvent,
    setTaskStatus,
    setTaskDetail,
    setContext,
    setAppearance,
    setFontSize,
    setWorkDetails,
    setCodingView,
    setLanguage,
    setPermission,
    setSidebar,
    setDoc,
    toggleSidebar,
    toggleDoc,
    setLayoutScope,
    openDocument,
    closeDocument,
    documents,
    get layout() {
      return {
        sidebar: sidebarWidth(),
        doc: docWidthFor(),
        docOpen: state.docOpen,
        collapsed: state.sidebarCollapsed || appWidth() < COMPACT_AT,
      };
    },
    resolvedAppearance,
  };
})();

Shell.init();
