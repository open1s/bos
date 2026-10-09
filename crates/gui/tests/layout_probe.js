// Injected into a copy of index.html by layout_selftest.sh. It reports geometry,
// not opinions: the narrow-window question is whether anything leaves the
// viewport horizontally, and by how much.
(function () {
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
      const rows = sb.querySelectorAll(".session-row, .nav-item, .ws-head, .sidebar-head").length;
      out.sidebarRows = rows;
      // `scrollWidth` is flush even when the text is squeezed, because the flex
      // children shrink with the box. The honest test for "unreadable" is a box
      // too small that still has rows in it.
      out.sidebarSqueezed = sb.clientWidth < 200 && rows > 0;
      if (!open) out.offscreen = out.offscreen.filter((k) => !k.startsWith("sidebar"));
    }
    out.drawer = measureDrawer();
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
    return out;
  }
  function report(data) {
    let host = document.getElementById("result");
    if (!host) {
      host = document.createElement("div");
      host.id = "result";
      document.body.appendChild(host);
    }
    host.textContent = JSON.stringify(data);
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
