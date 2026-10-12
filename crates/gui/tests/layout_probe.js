// Injected into a copy of index.html by layout_selftest.sh. It reports geometry,
// not opinions: the narrow-window question is whether anything leaves the
// viewport horizontally, and by how much.
(function () {
  // Elements that paint a shadow, a blur, a filter or a gradient. Depth in this
  // UI comes from a 1px border and a solid fill, so any of those is a defect and
  // the name of the offending element is what makes it fixable.
  function paintedOffenders() {
    const bad = [];
    const skip = new Set(["script", "style", "link", "meta", "head", "html"]);
    for (const el of document.querySelectorAll("*")) {
      const tag = el.tagName.toLowerCase();
      if (skip.has(tag)) continue;
      const cs = getComputedStyle(el);
      const why = [];
      if (cs.boxShadow && cs.boxShadow !== "none") why.push("shadow");
      if (cs.textShadow && cs.textShadow !== "none") why.push("text-shadow");
      if (cs.backdropFilter && cs.backdropFilter !== "none") why.push("backdrop-filter");
      if (cs.filter && cs.filter !== "none") why.push("filter");
      if (cs.backgroundImage && cs.backgroundImage.indexOf("gradient") >= 0) why.push("gradient");
      if (!why.length) continue;
      const name = el.id || (typeof el.className === "string" ? el.className : "") || tag;
      bad.push(String(name).split(" ")[0] + "(" + why.join(",") + ")");
    }
    return [...new Set(bad)].slice(0, 8);
  }

  function measure() {
    const out = { viewport: window.innerWidth, docScrollW: document.documentElement.scrollWidth,
                  docClientW: document.documentElement.clientWidth, boxes: {}, offscreen: [] };
    for (const id of ["app", "sidebar", "main", "chat-head", "messages", "composer", "statusbar"]) {
      const el = document.getElementById(id);
      if (!el) { out.boxes[id] = null; continue; }
      const r = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      out.boxes[id] = [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height),
                       cs.display === "none" ? "hidden" : "shown"];
    }
    const seen = new Set();
    for (const el of document.querySelectorAll("#app, #app *")) {
      const r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) continue;
      const parkedEl = document.getElementById("sidebar");
      const parked = parkedEl && !parkedEl.classList.contains("open") &&
        getComputedStyle(parkedEl).position === "fixed";
      if (parked && el.closest && el.closest("#sidebar")) continue;
      if (r.right <= window.innerWidth + 1 && r.left >= -1) continue;
      const name = el.id || (typeof el.className === "string" ? el.className : "") || el.tagName;
      const key = String(name).split(" ")[0] + ":" + Math.round(r.left) + ".." + Math.round(r.right);
      if (seen.has(key)) continue;
      seen.add(key);
      out.offscreen.push(key);
    }
    out.offscreen = out.offscreen.slice(0, 6);
    // A sidebar that is merely squeezed keeps every element inside the viewport,
    // so the off-screen check above is blind to it: the text overflows its own
    // box instead. That is a chat list nobody can read, and it is invisible in a
    // screenshot of the shell too.
    const sb = document.getElementById("sidebar");
    if (sb) {
      const open = sb.classList.contains("open");
      out.sidebarOpen = open;
      const rows = sb.querySelectorAll(".session-row, .ws-head, .sidebar-head").length;
      // A collapsed rail keeps its head and hides the rows; the head is not a
      // list, so only visible *content* rows can make a narrow box unreadable.
      const visibleRows = [...sb.querySelectorAll(".session-row, .ws-head")]
        .filter((el) => el.getClientRects().length > 0).length;
      out.sidebarRows = rows;
      out.sidebarVisibleRows = visibleRows;
      // `scrollWidth` is flush even when the text is squeezed, because the flex
      // children shrink with the box. The honest test for "unreadable" is a box
      // too small that still has rows in it.
      out.sidebarSqueezed = sb.clientWidth < 200 && visibleRows > 0;
      if (!open) out.offscreen = out.offscreen.filter((k) => !k.startsWith("sidebar"));
    }
    out.drawer = measureDrawer();
    // Flat is a property of what is *painted*, not of what the stylesheet says:
    // a shadow arriving from a token, a class added in JS or an inline style is
    // invisible to a text scan of style.css and perfectly visible on screen. So
    // every element the page lays out is asked what it actually paints.
    out.painted = paintedOffenders();
    const beforeX = window.scrollX;
    window.scrollTo(4000, 0);
    out.scrolledX = Math.round(window.scrollX);
    window.scrollTo(beforeX, 0);
    // Scanned across the whole document, not just `#app`: an element outside it
    // can be what widens the scrollable area, and naming it is the whole point.
    const live = [...document.querySelectorAll("*")].map((el) => [el, el.getBoundingClientRect()])
      .filter(([, r]) => r.width > 1 && r.height > 1);
    const name = ([el, r]) => (el.id || String(el.className).split(" ")[0] || el.tagName) +
      "@" + Math.round(r.left) + ".." + Math.round(r.right);
    out.widest = live.slice().sort((x, y) => y[1].right - x[1].right).slice(0, 4).map(name);
    out.leftmost = live.slice().sort((x, y) => x[1].left - y[1].left).slice(0, 3).map(name);
    out.modal = measureModal();
    out.attach = measureAttachments();
    return out;
  }
  // The attach strip is the one composer surface the empty-window passes above
  // never see: it exists only while images are staged. Staged here through the
  // app's own renderer, then measured against the input it must not cover, the
  // palette it shares an anchor with, and the remove control it must obey.
  function measureAttachments() {
    const strip = document.getElementById("attach-strip");
    const input = document.getElementById("input");
    const send = document.getElementById("send");
    const palette = document.getElementById("cmd-palette");
    const missing = !strip || !input || !send || !palette ||
      typeof state === "undefined" || typeof renderAttachments !== "function" ||
      typeof syncPalette !== "function";
    if (missing) return { present: false };
    const rect = (el) => {
      const r = el.getBoundingClientRect();
      return [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)];
    };
    const dot = "data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACwAAAAAAQABAAACAUwAOw==";
    state.pendingImages = [dot, dot + "-2"];
    renderAttachments();
    const out = { present: true, chips: strip.querySelectorAll(".attach-chip").length };
    out.strip = rect(strip);
    out.input = rect(input);
    const sR = strip.getBoundingClientRect();
    const iR = input.getBoundingClientRect();
    out.overlapsInput = sR.bottom > iR.top + 1 && sR.top < iR.bottom - 1;
    out.insideViewport =
      sR.left >= -1 && sR.right <= window.innerWidth + 1 && sR.top >= -1;
    out.sendEnabledImagesOnly = !send.disabled && input.value === "";
    // The palette opens at the same anchor as the strip: it must cover the
    // strip while open (an opaque panel that half-dodges would tear), and the
    // strip must be back the moment the palette closes.
    input.value = "/";
    syncPalette();
    const pR = palette.getBoundingClientRect();
    const sR2 = strip.getBoundingClientRect();
    out.paletteOpen = !palette.classList.contains("hidden");
    out.paletteCoversStrip = out.paletteOpen &&
      pR.top <= sR2.top + 1 && pR.bottom >= sR2.bottom - 1 &&
      pR.left <= sR2.left + 1 && pR.right >= sR2.right - 1;
    input.value = "";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    out.stripBackAfterPalette = !strip.classList.contains("hidden");
    const x = strip.querySelector(".attach-x");
    if (x) x.click();
    out.chipsAfterRemove = strip.querySelectorAll(".attach-chip").length;
    state.pendingImages = [];
    renderAttachments();
    out.hiddenWhenEmpty = strip.classList.contains("hidden") && send.disabled;
    return out;
  }
  function report(data) {
    // The hosts are layout-inert on purpose: their JSON is one unbreakable
    // token, and a probe that widens the document would then report itself as
    // the overflow (measured: #result-attach pushed scrollWidth to 1382 at a
    // 900px window and the guard failed on a defect the page never had).
    const inert = "position:fixed;left:-99999px;top:0;white-space:pre;";
    let host = document.getElementById("result");
    if (!host) {
      host = document.createElement("div");
      host.id = "result";
      host.style.cssText = inert;
      document.body.appendChild(host);
    }
    const { attach, ...rest } = data;
    host.textContent = JSON.stringify(rest);
    let attachHost = document.getElementById("result-attach");
    if (!attachHost) {
      attachHost = document.createElement("div");
      attachHost.id = "result-attach";
      attachHost.style.cssText = inert;
      document.body.appendChild(attachHost);
    }
    attachHost.textContent = JSON.stringify(attach);
  }
  // The settings modal ships hidden, so the first measurement cannot see it.
  // A narrow window breaks a modal long before it breaks the shell, and a modal
  // nobody measured is a modal nobody checked.
  function measureModal() {
    const modal = document.getElementById("settings-modal");
    if (!modal) return { open: false };
    modal.classList.remove("hidden");
    const card = modal.querySelector(".modal-card");
    const out = { open: true, card: null, clipped: [], outside: [] };
    if (card) {
      const r = card.getBoundingClientRect();
      out.card = [Math.round(r.left), Math.round(r.width), Math.round(r.height)];
      if (r.right > window.innerWidth + 1 || r.left < -1) out.outside.push("card");
    }
    const seen = new Set();
    for (const el of modal.querySelectorAll("*")) {
      const r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) continue;
      if (r.right > window.innerWidth + 1 || r.left < -1) {
        const name = el.id || (typeof el.className === "string" ? el.className : "") || el.tagName;
        const key = "outside:" + String(name).split(" ")[0];
        if (!seen.has(key)) { seen.add(key); out.outside.push(key); }
      }
      if (el.scrollWidth > el.clientWidth + 2 && r.width > 40) {
        const name = el.id || (typeof el.className === "string" ? el.className : "") || el.tagName;
        const key = String(name).split(" ")[0] + ":" + el.scrollWidth + ">" + el.clientWidth;
        if (!seen.has(key)) { seen.add(key); out.clipped.push(key); }
      }
    }
    out.clipped = out.clipped.slice(0, 6);
    out.outside = out.outside.slice(0, 6);
    modal.classList.add("hidden");
    return out;
  }
  // The drawer is a behaviour, not a style: it is only correct if the button
  // actually brings the chat list back, so the probe clicks it and reports where
  // the sidebar landed.
  function measureDrawer() {
    const toggle = document.getElementById("sidebar-toggle");
    const sb = document.getElementById("sidebar");
    if (!toggle || !sb) return { present: false };
    if (getComputedStyle(toggle).display === "none") {
      return { present: false, closedWidth: Math.round(sb.getBoundingClientRect().width) };
    }
    // Measure the settled layout, not the animation: the transition would make
    // the rect depend on when the probe happens to look. The motion itself is not
    // measured here and is not claimed to be.
    /* A drawer that opens without the list it exists for is the defect this probe
       was blind to: it measured the box, and the box was right. Ask what the fold
       was hiding, and ask whether honouring the request also armed a rail the window
       cannot justify — the flag that used to outlive the window it was made in. */
    const list = document.getElementById("session-list");
    const listShown = () => !!list && getComputedStyle(list).display !== "none";
    const armed = () =>
      typeof Shell !== "undefined" && Shell.state ? !!Shell.state.sidebarCollapsed : null;
    const transition = sb.style.transition;
    sb.style.transition = "none";
    const before = sb.getBoundingClientRect();
    const closedList = listShown();
    const armedBefore = armed();
    toggle.click();
    const opened = sb.classList.contains("open");
    const mode = document.getElementById("app").dataset.sidebar;
    const openList = listShown();
    const after = sb.getBoundingClientRect();
    const expanded = toggle.getAttribute("aria-expanded");
    /* Read the flag *between* the two clicks: a write that is a toggle returns to
       where it started by the time it is closed again, so only the moment the drawer
       is open can tell "honoured the request" from "armed a rail as well". */
    const armedOpen = armed();
    toggle.click();
    const reclosed = sb.classList.contains("open");
    const armedAfter = armed();
    sb.style.transition = transition;
    return { present: true, opened, mode, closedList, openList,
             closedLeft: Math.round(before.left), openLeft: Math.round(after.left),
             openWidth: Math.round(after.width), expanded, reclosed,
             armedBefore, armedOpen, armedAfter };
  }
  /* §3B parity tokens: the reference's measured numbers, read back as computed
   values so the guard can name the one that drifted. Dark is deliberately not
   matched yet (§3B.6: no dark capture exists). */
(function () {
  function swatch() {
    const probe = document.createElement("div");
    probe.setAttribute("data-theme", "light");
    probe.style.display = "none";
    document.body.appendChild(probe);
    const cs = getComputedStyle(probe);
    const get = (k) => cs.getPropertyValue(k).trim();
    const out = {
      accent: get("--accent"),
      sidebarBg: get("--sidebar"),
      selected: get("--active"),
      text: get("--text"),
      dim: get("--dim"),
    };
    probe.remove();
    return out;
  }
  /* The stylesheet's own computation for a real row: the empty probe page has no
     sessions, and a token that no selector applies is a number, not a metric. */
  function rowBox() {
    const row = document.createElement("div");
    row.className = "session-row";
    const title = document.createElement("span");
    title.className = "title";
    title.textContent = "probe";
    row.appendChild(title);
    const host = document.getElementById("sidebar") || document.body;
    row.style.visibility = "hidden";
    host.appendChild(row);
    const out = {
      rowH: Math.round(row.getBoundingClientRect().height),
      rowFont: Math.round(parseFloat(getComputedStyle(title).fontSize)),
    };
    row.remove();
    return out;
  }
  function measureTokens() {
    const app = document.getElementById("app");
    const sb = document.getElementById("sidebar");
    const cs = app ? getComputedStyle(app) : null;
    const rootCs = getComputedStyle(document.documentElement);
    return Object.assign(swatch(), rowBox(), {
      // The token is the parity number; #app's inline value is whatever state the
      // window is in right now (348 expanded, 56 railed), so both are reported.
      tokenSidebarW: Math.round(parseFloat(rootCs.getPropertyValue("--sidebar-w"))),
      appSidebarW: cs ? Math.round(parseFloat(cs.getPropertyValue("--sidebar-w"))) : null,
      sideRowH: cs ? cs.getPropertyValue("--side-row-h").trim() : null,
      sidebarFont: sb ? Math.round(parseFloat(getComputedStyle(sb).fontSize)) : null,
      ...badgeBox(),
    });
  }
  /* W1's badge, measured as if it were showing: `hidden` means no box, and the
     spec's numbers are about where it lands, not about whether it is out. `headH`
     rides along because the badge's top is stated as 200px below the tab divider. */
  function badgeBox() {
    const box = document.getElementById("working-badge");
    const head = document.getElementById("chat-head");
    if (!box) return {};
    const wasHidden = box.classList.contains("hidden");
    if (wasHidden) box.classList.remove("hidden");
    const r = box.getBoundingClientRect();
    const main = document.getElementById("main").getBoundingClientRect();
    if (wasHidden) box.classList.add("hidden");
    // C1b: the send circle is measured against the composer it sits in, since the
    // spec states its size and its insets rather than a row position.
    const send = document.getElementById("send");
    const composer = document.getElementById("composer");
    const sr = send ? send.getBoundingClientRect() : null;
    const cr = composer ? composer.getBoundingClientRect() : null;
    return {
      headH: head ? Math.round(head.getBoundingClientRect().height) : null,
      badgeW: Math.round(r.width),
      badgeTop: Math.round(r.top),
      badgeRightInset: Math.round(main.right - r.right),
      sendW: sr ? Math.round(sr.width) : null,
      sendH: sr ? Math.round(sr.height) : null,
      sendRightInset: sr && cr ? Math.round(cr.right - sr.right) : null,
      sendBottomInset: sr && cr ? Math.round(cr.bottom - sr.bottom) : null,
    };
  }
  function reportTokens() {
    const old = document.getElementById("result-tokens");
    if (old) old.remove();
    const pre = document.createElement("pre");
    pre.style.cssText = "position:absolute;left:-99999px;top:0;margin:0;";
    pre.id = "result-tokens";
    pre.textContent = JSON.stringify(measureTokens());
    document.body.insertBefore(pre, document.body.firstChild);
  }
  if (document.body) reportTokens();
  window.addEventListener("load", reportTokens);
})();

(function () {
  // The document toggle belongs to the head's own control row, on its right edge.
  // It was once appended to `#chat-head`, the column that holds the row and the
  // tabs, so it painted as a full-width block *under the tabs* (rev 124) — the
  // reported bug, and exactly what a number beats a screenshot for.
  function measureDoc() {
    const btn = document.getElementById("doc-toggle");
    const row = document.querySelector("#chat-head .head-row");
    if (!btn || !row) return { present: false };
    const b = btn.getBoundingClientRect();
    const r = row.getBoundingClientRect();
    const tabs = document.getElementById("chat-tabs");
    const t = tabs ? tabs.getBoundingClientRect() : null;
    return {
      present: true,
      // Below a certain width the head hides its controls outright; a display:none
      // button is 0-wide at the origin, which is a layout choice, not this bug.
      visible: b.width > 0 && b.height > 0,
      inRow:
        b.top >= r.top - 1 && b.bottom <= r.bottom + 1 &&
        b.left >= r.left - 1 && b.right <= r.right + 1,
      atRight: b.right >= r.right - 8,
      notABar: b.width < r.width - 40,
      aboveTabs: !t || b.bottom <= t.top + 1,
      w: Math.round(b.width),
      rowW: Math.round(r.width),
    };
  }
  window.__docProbe = measureDoc;

  function reportDoc() {
    // One element, written twice: at 560px and 500px the page's load event never
    // fires at all (the main report below is parse-time for the same reason), and
    // a measurement registered only on load was simply absent there.
    const old = document.getElementById("result-doc");
    if (old) old.remove();
    const pre = document.createElement("pre");
    /* A probe that reports *inside* the flow reports on a page the guard then
       measures: a long JSON line here made the document 178px wider than the
       window and reddened "the window can be panned sideways". Off-flow, like
       the probe's own report elements. */
    pre.style.cssText = "position:absolute;left:-99999px;top:0;margin:0;";
    pre.id = "result-doc";
    pre.textContent = JSON.stringify(measureDoc());
    // First child, not last: the dump for the narrowest window is the largest DOM,
    // and its tail is where a serialized page gets cut off.
    document.body.insertBefore(pre, document.body.firstChild);
  }
  if (document.body) reportDoc();
  window.addEventListener("load", reportDoc);
})();

window.__layoutMeasure = function () { report(measure()); return measure(); };
  report(measure());
  window.addEventListener("load", () => report(measure()));
})();

/* Last resort: at one width the report above never reached the DOM, and a probe that
   vanishes looks exactly like a page that never ran. This always writes one — with the
   reason, if there is one — so a missing measurement can be told from a bad one. */
(function () {
  function write(extra) {
    const old = document.getElementById("result-doc");
    if (old) old.remove();
    const pre = document.createElement("pre");
    /* A probe that reports *inside* the flow reports on a page the guard then
       measures: a long JSON line here made the document 178px wider than the
       window and reddened "the window can be panned sideways". Off-flow, like
       the probe's own report elements. */
    pre.style.cssText = "position:absolute;left:-99999px;top:0;margin:0;";
    pre.id = "result-doc";
    let body;
    try {
      body = window.__docProbe ? window.__docProbe() : { present: false, error: "probe not exposed" };
    } catch (e) {
      body = { present: false, error: String(e && e.message ? e.message : e) };
    }
    pre.textContent = JSON.stringify(Object.assign({}, body, extra || {}));
    document.body.insertBefore(pre, document.body.firstChild);
  }
  write();
  window.addEventListener("load", () => write());
})();
