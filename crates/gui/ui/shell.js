/* BOS workbench shell: three-panel layout, design tokens, task state and the
   document panel. Deliberately free of Tauri and of agent state, so the shell
   can be tested on its own and a task's lifetime never depends on a view.

   Part II §15 of docs/gui-requirements.md: UI components must not own agent
   scheduling, model requests or persistence of business data. This module owns
   only presentation, layout and UI-local preferences. */

"use strict";

const Shell = (() => {
  const LAYOUT_KEY = "bos.layout.v1";
  /* Layout schema: v2 stops the right column from railing an expanded sidebar
     (rev 85), so a record without `v` is a pre-v2 one whose rail — written by
     the old concession — is healed once on adopt. */
  const LAYOUT_V = 2;
  const UI_KEY = "bos.ui.v1";

  const SIDEBAR_MIN = 264;
  const SIDEBAR_MAX = 420;
  const SIDEBAR_DEFAULT = 348;
  const SIDEBAR_RAIL = 56;
  /* Right-column and frame geometry, cloned from DSH's ui-layout contract (see
     docs/dsh-ui-reference/dsh-client-ui-layout/README.md): the sidebar spans
     264-420px at a 348px default with a 56px rail, the right column opens at 45%
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
    /* Whether the folded column is let out over the centre. Transient on purpose:
       it belongs to the window that cannot hold the column in flow, not to the
       user's layout, so it is neither persisted nor restored. */
    drawerOpen: false,
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
    /* A rail only survives from a v2 record — i.e. one the user chose with the
       toggle — never from the pre-v2 concession that railed the sidebar the
       moment the right column opened. Idempotent: v2 records persist with `v`,
       so a manual rail after the upgrade is kept. */
    const legacy = saved.v !== LAYOUT_V;
    state.sidebarCollapsed = saved.sidebarCollapsed === true && !legacy;
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
      v: LAYOUT_V,
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
    /* A drawer is only ever a folded column, and only where the column cannot be in
       flow: a window with room says what it means with the rail, so a flag left over
       from a narrow window cannot resurrect an overlay on a wide one. */
    if (width >= COMPACT_AT && state.drawerOpen) state.drawerOpen = false;
    const drawer = state.drawerOpen && width < COMPACT_AT;
    app.dataset.doc = showDoc ? "open" : "closed";
    app.dataset.sidebar = drawer ? "drawer" : collapse ? "rail" : "expanded";
    /* The drawer's visible state, written in one place: the class, the attribute and
       the button's aria all describe the same fact, and one function owns all three. */
    const sbEl = document.getElementById("sidebar");
    if (sbEl) sbEl.classList.toggle("open", drawer);
    /* A column that is gone cannot still own the active surface. */
    if (activePanel === "document" && !showDoc) activePanel = null;
    app.dataset.panel = activePanelId();
    app.style.setProperty(
      "--sidebar-w",
      `${drawer || !collapse ? state.sidebar : SIDEBAR_RAIL}px`,
    );
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
    if (docPanel) docPanel.hidden = !(showDoc && (!activePanel || activePanel === DOCUMENT));
    /* The column docks one occupant at a time: the documents panel when nothing
       else is selected, otherwise the active panel's own surface. */
    const hostShown = showDoc && !!activePanel && activePanel !== DOCUMENT;
    if (panelHost) panelHost.hidden = !hostShown;
    for (const [key, host] of surfaces) host.hidden = !(hostShown && key === activePanel);
    const docToggle = document.getElementById("doc-toggle");
    if (docToggle) docToggle.setAttribute("aria-pressed", String(showDoc));
    const railToggle = document.getElementById("sidebar-toggle");
    if (railToggle) {
      railToggle.setAttribute("aria-pressed", String(!collapse));
      /* The ☰ is the drawer's control where there is a drawer and the rail's where
         there is not, so it reports the one that is actually in play. */
      railToggle.setAttribute("aria-expanded", String(drawer));
    }
    emit("layout", {
      sidebar: state.sidebar,
      doc: showDoc ? geo.width : 0,
      open: showDoc,
      collapse,
      drawer,
      tight: !geo.canShow,
    });
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
  /* One request — "show me the column" — answered two ways, because a window can
     answer it two ways. With room, the column folds to the rail and back. Without
     room (under COMPACT_AT) it cannot be in flow at all, so the same request lets it
     out over the centre. Flipping the persisted rail flag there was the bug: the flag
     outlives the window that justified it, so the strip followed the user to a wide
     screen — and a rail hides the chat list, which left no way back to it (rev 120). */
  const toggleSidebar = () => {
    if (appWidth() < COMPACT_AT) {
      state.drawerOpen = !state.drawerOpen;
    } else {
      state.sidebarCollapsed = !state.sidebarCollapsed;
      state.drawerOpen = false;
      persistLayout();
    }
    applyLayout();
  };
  /* Picking a chat is what the drawer was for: the list is the means, not the end. */
  const closeDrawer = () => {
    if (!state.drawerOpen) return;
    state.drawerOpen = false;
    applyLayout();
  };
  const toggleDoc = () => {
    state.docOpen = !state.docOpen;
    openRightColumn();
    persistLayout();
    applyLayout();
  };
  /* DSH's frame takes the right column's room from the sidebar first, but the
     reference concedes only a *manually expanded* sidebar (see
     docs/dsh-ui-reference/dsh-client-ui-layout/README.md), and a railed session
     list was unreadable in practice (rev 85). We concede width instead of the
     column: opening narrows an over-wide sidebar just far enough for the
     column's 300px floor, never below 264px. The column's own step-down rule
     then decides whether it opens at all — below the 1024px threshold
     it is not offered, and an explicit rail (the ☰ toggle) is respected as the
     user's own choice. */
  const openRightColumn = () => {
    if (!state.docOpen || state.sidebarCollapsed) return;
    if (appWidth() < COMPACT_AT) return;
    if (appWidth() - sidebarWidth() - CENTER_MIN - 16 >= DOC_STEP) return;
    const next = clamp(appWidth() - CENTER_MIN - 16 - DOC_STEP, SIDEBAR_MIN, SIDEBAR_MAX);
    if (next !== state.sidebar) {
      state.sidebar = next;
      persistLayout();
    }
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

  /* ---------- panel registry --------------------------------------------- */

  /* DSH ui-layout: the root column is a keyed `main` slot whose `conversation`
     key is reserved for the product, every other surface is a panel selected by
     id, and `null` means Conversation. The registry keeps that seam real here
     instead of decorative: the built-in document panel goes through the same
     API a future surface would, an unknown id changes nothing, and the reserved
     key cannot be registered or displaced. */
  const CONVERSATION = "conversation";
  const DOCUMENT = "document";
  const panels = new Map();
  const surfaces = new Map();
  let activePanel = null;
  let panelHost = null;

  const panelIds = () => [CONVERSATION, ...panels.keys()];
  const activePanelId = () => activePanel || CONVERSATION;

  const registerPanel = ({ id, title = "", kind = "right", render = null } = {}) => {
    const key = String(id || "");
    if (!key || key === CONVERSATION || panels.has(key)) return false;
    if (render !== null && typeof render !== "function") return false;
    panels.set(key, { id: key, title, kind, render });
    emit("panel", { action: "register", id: key, panels: panelIds() });
    return true;
  };

  /* A registered surface is a mount point, not just an identity: its host element
     is created the first time the panel becomes active and handed to the panel's
     own render function exactly once, so a re-selected panel reuses its DOM
     instead of rebuilding under the reader. Until then nothing is rendered, which
     is what keeps an unopened panel free. */
  const surfaceFor = (key) => {
    if (key === DOCUMENT) return docPanel;
    const entry = panels.get(key);
    if (!entry) return null;
    let host = surfaces.get(key);
    if (!host) {
      host = document.createElement("section");
      host.className = "panel-surface";
      host.dataset.panelId = key;
      if (entry.title) host.setAttribute("aria-label", entry.title);
      surfaces.set(key, host);
      if (panelHost) panelHost.appendChild(host);
    }
    return host;
  };

  const renderSurface = (key) => {
    const entry = panels.get(key);
    if (!entry || !entry.render) return;
    const host = surfaceFor(key);
    if (!host || host.dataset.rendered === "1") return;
    host.dataset.rendered = "1";
    entry.render(host);
  };

  const unregisterPanel = (id) => {
    const key = String(id || "");
    if (!panels.delete(key)) return false;
    const host = surfaces.get(key);
    if (host) host.remove();
    surfaces.delete(key);
    if (activePanel === key) selectPanel(null);
    emit("panel", { action: "unregister", id: key, panels: panelIds() });
    return true;
  };

  const selectPanel = (id) => {
    const key = id == null || id === CONVERSATION ? null : String(id);
    if (key && !panels.has(key)) return false;
    activePanel = key;
    /* Selecting a right-docked surface surfaces it: the column opens through the
       same concession path a plugin's panel and the built-in documents panel
       both use. */
    const entry = key ? panels.get(key) : null;
    if (key === DOCUMENT || (entry && entry.kind === "right")) {
      state.docOpen = true;
      openRightColumn();
    }
    if (key) renderSurface(key);
    if (app) app.dataset.panel = activePanelId();
    persistLayout();
    applyLayout();
    emit("panel", { action: "select", active: activePanelId(), panels: panelIds() });
    return true;
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
      // Start is the column's name, not one of its documents: it carries the
      // compass and no tab chrome, the way the reference's header reads.
      if (doc.id === "start") {
        tab.classList.add("doc-title");
        tab.appendChild(panelIcon("compass"));
      }
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

  /* The column's icons: the same 24-grid outline the rest of the chrome draws
     (createElementNS, stroked in currentColor), written here because this set —
     a compass, the dock glyphs, a folder, a terminal, a globe — belongs to this
     surface. */
  const PANEL_ICONS = {
    compass: [
      "M12 3.4a8.6 8.6 0 1 1 0 17.2 8.6 8.6 0 0 1 0-17.2z",
      "M15.2 8.8 10.5 10.5 8.8 15.2l4.7-1.7z",
    ],
    dockLeft: [
      "M4.6 5.4h14.8A1.6 1.6 0 0 1 21 7v10a1.6 1.6 0 0 1-1.6 1.6H4.6A1.6 1.6 0 0 1 3 17V7a1.6 1.6 0 0 1 1.6-1.6z",
      "M9.6 5.4v13.2",
    ],
    dockRight: [
      "M4.6 5.4h14.8A1.6 1.6 0 0 1 21 7v10a1.6 1.6 0 0 1-1.6 1.6H4.6A1.6 1.6 0 0 1 3 17V7a1.6 1.6 0 0 1 1.6-1.6z",
      "M14.4 5.4v13.2",
    ],
    expand: [
      "M9.4 4.6H4.6V9.4",
      "M14.6 19.4h4.8v-4.8",
      "M19.4 9.4V4.6h-4.8",
      "M4.6 14.6v4.8h4.8",
    ],
    reload: ["M19.4 12a7.4 7.4 0 1 1-2.2-5.2", "M19.6 4.6v4.8h-4.8"],
    folder: [
      "M3 7.6A1.6 1.6 0 0 1 4.6 6h3.6L10.4 8.2h9A1.6 1.6 0 0 1 21 9.8v7.6A1.6 1.6 0 0 1 19.4 19H4.6A1.6 1.6 0 0 1 3 17.4z",
    ],
    terminal: [
      "M4 6.4h16A1.6 1.6 0 0 1 21.6 8v8a1.6 1.6 0 0 1-1.6 1.6H4A1.6 1.6 0 0 1 2.4 16V8A1.6 1.6 0 0 1 4 6.4z",
      "M6.4 10.4l2.4 2-2.4 2",
      "M12.4 14.4h5.2",
    ],
    globe: [
      "M12 3.6a8.4 8.4 0 1 1 0 16.8 8.4 8.4 0 0 1 0-16.8z",
      "M3.6 12h16.8",
      "M12 3.6c2.2 2.3 3.3 5.1 3.3 8.4S14.2 18.1 12 20.4c-2.2-2.3-3.3-5.1-3.3-8.4S9.8 5.9 12 3.6z",
    ],
  };
  const panelIcon = (name, cls) => {
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("class", "ico" + (cls ? " " + cls : ""));
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("focusable", "false");
    for (const d of PANEL_ICONS[name] || []) {
      const p = document.createElementNS(ns, "path");
      p.setAttribute("d", d);
      svg.appendChild(p);
    }
    return svg;
  };

  /* The column's arrangement. Docking is the order of a flex sibling, so it is
     an attribute on the frame and nothing moves in the DOM; expanding keeps the
     width it replaced instead of recomputing what the width should have been. */
  let docDock = "right";
  let docWide = false;
  let docWidePrev = 0;

  const markDock = () => {
    const pairs = [
      ["doc-dock-left", docDock === "left"],
      ["doc-dock-right", docDock === "right"],
      ["doc-wide", docWide],
    ];
    for (const [id, on] of pairs) {
      const el = document.getElementById(id);
      if (el) el.setAttribute("aria-pressed", String(on));
    }
  };

  const setDocDock = (side) => {
    docDock = side === "left" ? "left" : "right";
    if (app) app.setAttribute("data-doc-dock", docDock);
    markDock();
  };

  const toggleDocWide = () => {
    if (docWide) {
      docWide = false;
      setDoc(docWidePrev || Math.round(appWidth() * 0.32));
    } else {
      docWidePrev = docWidthFor();
      docWide = true;
      setDoc(Math.max(Math.round(appWidth() * 0.44), 420));
    }
    markDock();
  };

  /* The Browser card's second door: from the keyboard the column may be closed
     or holding a document, and either one is in the way of a field that has to
     be typed into. */
  const promptBrowser = () => {
    if (!state.docOpen) toggleDoc();
    if (activeDoc) {
      activeDoc = null;
      renderDoc();
    }
    if (revealBrowser) revealBrowser();
  };

  /* One launcher card: an icon, what it opens, the line that says what that
     buys, and the chord that reaches it. The card is the button, so the whole
     row is the target rather than the words inside it. */
  const startCard = (action, icon, title, sub, keys, kind) => {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "start-card";
    card.dataset.act = action;
    card.appendChild(panelIcon(icon, kind ? "start-card-ico " + kind : "start-card-ico"));
    const body = document.createElement("span");
    body.className = "start-card-body";
    const t = document.createElement("span");
    t.className = "start-card-title";
    t.textContent = title;
    const s = document.createElement("span");
    s.className = "start-card-sub";
    s.textContent = sub;
    body.appendChild(t);
    body.appendChild(s);
    card.appendChild(body);
    if (keys) {
      const k = document.createElement("span");
      k.className = "start-card-key";
      k.textContent = keys;
      card.appendChild(k);
    }
    return card;
  };

  /* The Browser card asks for a page before it can open one, so it carries its
     own one-line field: the card is still the whole target, and the field is
     what the card was missing. */
  const browserField = (card, emit_) => {
    const form = document.createElement("form");
    form.className = "start-url";
    const input = document.createElement("input");
    input.type = "text";
    input.className = "start-url-input";
    input.spellcheck = false;
    input.autocomplete = "off";
    input.setAttribute("aria-label", "Web address");
    input.placeholder = "example.com";
    const go = document.createElement("button");
    go.type = "submit";
    go.className = "start-url-go";
    go.textContent = "Open";
    form.appendChild(input);
    form.appendChild(go);
    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      const url = input.value.trim();
      if (!url) return;
      input.value = "";
      emit_({ action: "browser", url });
      card.classList.remove("open");
    });
    form.addEventListener("click", (ev) => ev.stopPropagation());
    card.appendChild(form);
    card.classList.add("has-url");
    const open = () => {
      card.classList.add("open");
      input.focus();
    };
    card.revealUrl = open;
    card.addEventListener("click", open);
    return card;
  };

  /* The empty document column is the launcher: a compass where a document would
     be, and the three ways to put something in the column. What a door *does*
     is the app's business, so the card hands over an action and never a command
     name (Part II §15). */
  let revealBrowser = null;
  const renderStart = () => {
    const box = document.createElement("div");
    box.className = "doc-start";
    const hero = document.createElement("div");
    hero.className = "start-hero";
    hero.appendChild(panelIcon("compass", "start-compass"));
    box.appendChild(hero);
    const cards = document.createElement("div");
    cards.className = "start-cards";
    cards.appendChild(
      startCard("files", "folder", "Workspace files",
        "Browse files in this session's workspace", "\u2318 P", "files")
    );
    cards.appendChild(
      startCard("terminal", "terminal", "New terminal",
        "Run commands in the session workspace", "", "term")
    );
    const browser = startCard("browser", "globe", "Browser", "Browse web pages", "\u2318 T", "web");
    browserField(browser, emit);
    revealBrowser = () => browser.revealUrl && browser.revealUrl();
    cards.appendChild(browser);
    box.appendChild(cards);
    return box;
  };

  const renderDoc = () => {
    renderDocTabs();
    if (!docBody) return;
    docBody.textContent = "";
    // Reload only means something over a document; the launcher has none.
    const reload = document.getElementById("doc-refresh");
    if (reload) reload.hidden = !activeDoc;
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
    if (doc.img) {
      // T7: a picture opened from a transcript renders here at the panel's width. The
      // source is our own payload (a data URL carried by the message) and the alt is the
      // name the row shows, so the panel states what the bubble stated.
      const pic = document.createElement("img");
      pic.className = "doc-img";
      pic.src = doc.img;
      pic.alt = doc.title;
      docBody.appendChild(pic);
    } else {
      pre.textContent = doc.text || ""; // inert: document text is untrusted
      docBody.appendChild(pre);
    }
  };

  const openDocument = (doc) => {
    if (!doc || !doc.id) return;
    const entry = {
      id: doc.id,
      title: doc.title || doc.path || doc.id,
      path: doc.path || "",
      text: doc.text || "",
      img: doc.img || "",
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
    /* The column's own controls, in the reference's three shapes — where the
       column sits and whether it takes the middle — because the arrangement is
       what a header of a docked column is for. Reload stays (a document can
       change on disk) and hides while the launcher is up, where there is
       nothing to reload. */
    const act = (id, icon, label, run) => {
      const b = document.createElement("button");
      b.id = id;
      b.type = "button";
      b.className = "btn btn-ghost doc-act";
      b.title = label;
      b.setAttribute("aria-label", label);
      b.appendChild(panelIcon(icon));
      b.addEventListener("click", run);
      actions.appendChild(b);
      return b;
    };
    act("doc-refresh", "reload", "Reload the document",
      () => emit("document", { action: "refresh", id: activeDoc }));
    act("doc-dock-left", "dockLeft", "Dock the document panel on the left",
      () => setDocDock("left"));
    act("doc-wide", "expand", "Expand the document panel", toggleDocWide);
    act("doc-dock-right", "dockRight", "Dock the document panel on the right",
      () => setDocDock("right"));
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
    /* The docking host for registered surfaces sits beside the built-in document
       panel and takes the column's geometry when a panel is selected. */
    panelHost = document.createElement("aside");
    panelHost.id = "panel-host";
    panelHost.className = "doc-panel panel-host";
    panelHost.setAttribute("aria-label", "Panel");
    panelHost.hidden = true;
    docPanel.insertAdjacentElement("afterend", panelHost);
    /* The built-in surface registers itself through the public seam, so the
       registry is exercised by the product rather than only by tests. */
    registerPanel({ id: "document", title: "Documents", kind: "right" });
    // The default arrangement is written once at mount, so the frame always
    // carries the attribute the stylesheet reads rather than relying on a
    // missing attribute meaning "right".
    setDocDock(docDock);

    /* DSH ui-layout: Mod+B toggles the sidebar. It yields to a control that has
       its own meaning for the chord — an editable field, or the terminal, where
       Ctrl+B belongs to the program on the other end of the PTY. */
    const ownsKeystroke = (el) =>
      !!el &&
      (el.isContentEditable === true ||
        ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName) ||
        (typeof el.closest === "function" && !!el.closest(".run-panel")));
    document.addEventListener("keydown", (ev) => {
      if (ev.key !== "b" && ev.key !== "B") return;
      if (!(ev.metaKey || ev.ctrlKey) || ev.altKey) return;
      if (ownsKeystroke(ev.target)) return;
      ev.preventDefault();
      toggleSidebar();
    });

    // Pane toggles live in the centre toolbar so a narrow window never hides
    // navigation permanently (Part II §12.1 / GUI-01).
    const head0 = document.getElementById("chat-head");
    if (head0) {
      /* Adopt the button the markup already ships rather than minting a second
         one. Two elements claiming id="sidebar-toggle" made the id selector
         show both under 720 px and left the static one dead, because app.js had
         bound its drawer behaviour to whichever element it found first — so one
         visible button did nothing at all. */
      let railToggle = document.getElementById("sidebar-toggle");
      if (!railToggle) {
        railToggle = document.createElement("button");
        railToggle.id = "sidebar-toggle";
        railToggle.type = "button";
        railToggle.title = "Toggle sidebar";
        railToggle.textContent = "\u2630";
        head0.insertBefore(railToggle, head0.firstChild);
      }
      railToggle.classList.add("head-toggle");
      if (!railToggle.getAttribute("aria-label")) {
        railToggle.setAttribute("aria-label", "Toggle sidebar");
      }
      /* The Mod+B binding is not clickable, so announce it (M13 polish). */
      railToggle.setAttribute("aria-keyshortcuts", "Meta+B Control+B");
      railToggle.addEventListener("click", toggleSidebar);
      const docToggle = document.createElement("button");
      docToggle.id = "doc-toggle";
      docToggle.type = "button";
      docToggle.className = "mini-btn head-toggle";
      docToggle.title = "Toggle document panel";
      docToggle.textContent = "\u25a4";
      docToggle.addEventListener("click", toggleDoc);
      head0.insertBefore(railToggle, head0.firstChild);
      /* In the row of controls at the top right of the conversation, beside the
         theme and language buttons. Appending it to the header itself made it a
         full-width block of its own, under the tabs, instead of a control. */
      const headRow = head0.querySelector(".head-row");
      (headRow || head0).appendChild(docToggle);
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
    // The appearance/typography card belongs to the General section of the rail,
    // not above it: the dialog shows one pane at a time.
    const pane = document.getElementById("set-general");
    const section = document.createElement("div");
    section.className = "cap-section ui-general";
    const title = document.createElement("div");
    title.className = "cap-title";
    title.textContent = "General";
    section.appendChild(title);

    const row = (label, hint, node, id, key) => {
      const wrap = document.createElement("label");
      wrap.className = "row ui-row";
      const text = document.createElement("span");
      text.className = "ui-row-label";
      /* The visible label lives in its own span so a translation can replace the
         text without destroying the hint beside it (M13 polish). */
      const visible = document.createElement("span");
      visible.className = "ui-row-text";
      visible.textContent = label;
      if (key) visible.setAttribute("data-i18n", key);
      text.appendChild(visible);
      if (hint) {
        const small = document.createElement("small");
        small.className = "ui-row-hint";
        small.textContent = hint;
        if (key) small.setAttribute("data-i18n", key + "Hint");
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
    shortcuts.setAttribute("data-i18n", "shortcuts.open");
    shortcuts.textContent = "View shortcuts";
    shortcuts.title = "Every key this interface listens for";
    shortcuts.addEventListener("click", () => {
      if (window.Polish && typeof window.Polish.openShortcuts === "function") {
        window.Polish.openShortcuts();
      }
    });
    row(
      "Keyboard shortcuts",
      "Every key this interface listens for",
      shortcuts,
      "ui-shortcuts",
      "shortcuts.row",
    );

    (pane || form).insertBefore(section, (pane || form).firstChild);
    controlsBound = true;
    syncControls();
    return true;
  };

  /* ---------- status-line item registry ---------------------------------- */

  /* The footer is not just the token meter's: any surface can contribute an
     item without knowing how the footer is laid out. The registry keeps one
     node per id and insertion order, so a panel that refreshes every second
     updates its value instead of stacking duplicate spans — and the container
     is a polite live region, because a status line that only changes visually
     tells a screen-reader user nothing. */
  const statusRegistry = new Map();

  const statusHost = () => {
    /* The footer itself, not the notices span: `renderStatus` clears the span it
       hands to `renderArchived` and then to the notice list on every repaint, so a
       host mounted inside it was created, seen once and wiped by the next frame.
       Mounting on the footer keeps a registered item where it was put. */
    const bar = document.getElementById("statusbar");
    if (!bar) return null;
    let host = document.getElementById("status-items");
    if (!host) {
      host = document.createElement("span");
      host.id = "status-items";
      host.className = "status-items";
      host.setAttribute("aria-live", "polite");
      /* First, so the interface's own furniture keeps the outermost slot. */
      bar.insertBefore(host, bar.firstChild);
    }
    return host;
  };

  const paintStatusItem = (id) => {
    const host = statusHost();
    if (!host) return null;
    const item = statusRegistry.get(id);
    const existing = host.querySelector(`[data-status-item="${CSS.escape(id)}"]`);
    if (!item) {
      if (existing) existing.remove();
      return null;
    }
    let node = existing;
    if (!node) {
      node = document.createElement("span");
      node.className = "status-item";
      node.setAttribute("data-status-item", id);
      const label = document.createElement("span");
      label.className = "status-item-label";
      const value = document.createElement("span");
      value.className = "status-item-value";
      node.appendChild(label);
      node.appendChild(value);
      host.appendChild(node);
    }
    node.querySelector(".status-item-label").textContent = item.label || "";
    node.querySelector(".status-item-value").textContent =
      item.value === null || item.value === undefined ? "" : String(item.value);
    if (item.title) node.title = item.title;
    else node.removeAttribute("title");
    return node;
  };

  /* Add or replace an item. Re-adding an id is how a poller refreshes in place. */
  const addStatusItem = (id, item) => {
    if (!id || typeof id !== "string") return false;
    statusRegistry.set(id, {
      label: (item && item.label) || "",
      value: item ? item.value : "",
      title: (item && item.title) || "",
    });
    paintStatusItem(id);
    return true;
  };

  /* Patch an item without restating it: `Shell.updateStatusItem("jobs", {value: 3})`. */
  const updateStatusItem = (id, patch) => {
    const current = statusRegistry.get(id);
    if (!current) return false;
    statusRegistry.set(id, Object.assign({}, current, patch || {}));
    paintStatusItem(id);
    return true;
  };

  const removeStatusItem = (id) => {
    if (!statusRegistry.has(id)) return false;
    statusRegistry.delete(id);
    paintStatusItem(id);
    return true;
  };

  const statusItemIds = () => Array.from(statusRegistry.keys());

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
    addStatusItem,
    updateStatusItem,
    removeStatusItem,
    statusItemIds,
    setAppearance,
    setFontSize,
    setWorkDetails,
    setCodingView,
    setLanguage,
    // The permission vocabulary travels with the state it belongs to: the
    // composer's mode menu is built from it, so there is one list of modes.
    PERMISSIONS,
    setPermission,
    setSidebar,
    setDoc,
    toggleSidebar,
    closeDrawer,
    toggleDoc,
    setDocDock,
    toggleDocWide,
    promptBrowser,
    setLayoutScope,
    openDocument,
    closeDocument,
    documents,
    registerPanel,
    unregisterPanel,
    selectPanel,
    get panels() {
      return panelIds();
    },
    get activePanel() {
      return activePanelId();
    },
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
