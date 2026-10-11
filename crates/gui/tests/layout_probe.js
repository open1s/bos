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
    const transition = sb.style.transition;
    sb.style.transition = "none";
    const before = sb.getBoundingClientRect();
    toggle.click();
    const opened = sb.classList.contains("open");
    const after = sb.getBoundingClientRect();
    const expanded = toggle.getAttribute("aria-expanded");
    toggle.click();
    const reclosed = sb.classList.contains("open");
    sb.style.transition = transition;
    return { present: true, opened, closedLeft: Math.round(before.left),
             openLeft: Math.round(after.left), openWidth: Math.round(after.width),
             expanded, reclosed };
  }
  window.__layoutMeasure = function () { report(measure()); return measure(); };
  report(measure());
  window.addEventListener("load", () => report(measure()));
})();
