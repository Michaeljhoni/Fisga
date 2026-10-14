// Floating "Download" button on top of videos and audio players.
//
// The button lives in a closed shadow root attached to <html> (or to the
// fullscreen element), so page styles can't touch it and it never sits inside
// the player's own DOM. The DOM is built with createElement only: sites like
// YouTube enforce Trusted Types and reject innerHTML.
(() => {
  "use strict";
  if (window.__ldmOverlay) return;
  window.__ldmOverlay = true;

  const api = globalThis.browser ?? globalThis.chrome;
  const t = (key, fallback) => {
    try {
      return api.i18n.getMessage(key) || fallback;
    } catch {
      return fallback;
    }
  };

  const MIN_VIDEO = { w: 160, h: 90 };
  const MIN_AUDIO_W = 120;
  const HIDE_DELAY_MS = 2500;
  const SHOW_ON_PLAY_MS = 4000;
  const SVG_NS = "http://www.w3.org/2000/svg";

  let enabled = true;
  const tracked = new Set();
  let active = null; // media element the button belongs to
  let ui = null;
  let visible = false;
  let overUi = false;
  let menuOpen = false;
  let busy = false;
  let hideTimer = 0;
  let pointer = null;
  let framePending = false;

  // ---------------------------------------------------------------- settings
  api.storage?.local
    ?.get({ overlay: true })
    .then((s) => setEnabled(s.overlay !== false))
    .catch(() => {});
  api.storage?.onChanged?.addListener((changes, area) => {
    if (area === "local" && changes.overlay) setEnabled(changes.overlay.newValue !== false);
  });
  function setEnabled(value) {
    enabled = value;
    if (!enabled) hide(true);
  }

  // ------------------------------------------------------- media discovery
  function track(el) {
    if (el instanceof HTMLMediaElement) tracked.add(el);
  }
  function scan(root) {
    if (!root?.querySelectorAll) return;
    if (root instanceof HTMLMediaElement) track(root);
    root.querySelectorAll("video, audio").forEach(track);
  }

  const observer = new MutationObserver((mutations) => {
    for (const m of mutations) for (const node of m.addedNodes) if (node.nodeType === 1) scan(node);
  });
  function startObserving() {
    scan(document);
    observer.observe(document.documentElement, { childList: true, subtree: true });
  }
  if (document.documentElement) startObserving();
  else document.addEventListener("readystatechange", startObserving, { once: true });

  // Media inside open shadow roots or created without being in the DOM tree we
  // scanned still fires play/loadedmetadata; composedPath() gives the real target.
  for (const type of ["play", "playing", "loadedmetadata"]) {
    document.addEventListener(
      type,
      (e) => {
        const el = e.composedPath?.()[0] ?? e.target;
        if (!(el instanceof HTMLMediaElement)) return;
        track(el);
        if (type !== "loadedmetadata" && enabled && eligible(el)) {
          attach(el);
          scheduleHide(SHOW_ON_PLAY_MS);
        }
      },
      true,
    );
  }

  function eligible(el) {
    if (!el.isConnected) return false;
    if (el.srcObject instanceof MediaStream) return false; // webcam / call, nothing to download
    const r = el.getBoundingClientRect();
    if (el instanceof HTMLVideoElement) {
      if (r.width < MIN_VIDEO.w || r.height < MIN_VIDEO.h) return false;
    } else if (r.width < MIN_AUDIO_W || r.height < 16) {
      return false;
    }
    if (r.bottom <= 0 || r.right <= 0 || r.top >= innerHeight || r.left >= innerWidth) return false;
    const style = getComputedStyle(el);
    if (style.visibility === "hidden" || style.display === "none") return false;
    return Boolean(el.currentSrc || el.src || el.querySelector("source") || el.readyState > 0);
  }

  function mediaAt(x, y) {
    let best = null;
    let bestArea = Infinity;
    for (const el of tracked) {
      if (!el.isConnected) {
        tracked.delete(el);
        continue;
      }
      const r = el.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
      if (!eligible(el)) continue;
      const area = r.width * r.height;
      if (area < bestArea) {
        best = el;
        bestArea = area;
      }
    }
    return best;
  }

  // Players put overlays on top of the <video>, so hit-testing by rectangle
  // works where elementFromPoint would return the overlay.
  document.addEventListener(
    "pointermove",
    (e) => {
      pointer = { x: e.clientX, y: e.clientY };
      if (framePending || !enabled) return;
      framePending = true;
      requestAnimationFrame(onPointerFrame);
    },
    { capture: true, passive: true },
  );
  function onPointerFrame() {
    framePending = false;
    if (!pointer || !enabled) return;
    const el = mediaAt(pointer.x, pointer.y);
    if (el) {
      attach(el);
      cancelHide();
    } else if (visible && !overUi && !menuOpen) {
      scheduleHide(HIDE_DELAY_MS);
    }
  }

  // ------------------------------------------------------------------- UI
  const CSS = `
    :host { all: initial; }
    .box { position: relative; font: 600 13px/1.2 system-ui, -apple-system, "Segoe UI", Roboto, Ubuntu, sans-serif;
      color: #f8fafc; opacity: 0; transform: translateY(-3px); transition: opacity .14s ease, transform .14s ease; }
    .box.show { opacity: 1; transform: none; }
    .bar { display: flex; align-items: stretch; border-radius: 10px; overflow: hidden;
      background: rgba(15, 23, 42, .9); box-shadow: 0 6px 20px rgba(0, 0, 0, .35), 0 0 0 1px rgba(255, 255, 255, .08); }
    button { all: unset; box-sizing: border-box; cursor: pointer; display: flex; align-items: center; gap: 7px;
      padding: 7px 11px; color: inherit; white-space: nowrap; }
    button:hover { background: rgba(255, 255, 255, .12); }
    button:focus-visible { outline: 2px solid #60a5fa; outline-offset: -2px; }
    .more { padding: 7px 8px; border-left: 1px solid rgba(255, 255, 255, .14); }
    svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 2.2;
      stroke-linecap: round; stroke-linejoin: round; flex: none; }
    .main svg { color: #60a5fa; }
    .more svg { width: 13px; height: 13px; }
    .box.ok .main svg { color: #34d399; }
    .box.err .main svg { color: #f87171; }
    .box.busy .main { cursor: progress; }
    .menu, .toast { position: absolute; right: 0; top: calc(100% + 6px); box-sizing: border-box;
      background: rgba(15, 23, 42, .97); border-radius: 10px; box-shadow: 0 10px 28px rgba(0, 0, 0, .45);
      max-width: min(440px, 90vw); }
    .menu { min-width: 280px; max-height: 320px; overflow: auto; padding: 5px; }
    .item { display: block; width: 100%; padding: 8px 10px; border-radius: 7px; overflow: hidden; text-overflow: ellipsis; }
    .item small { display: block; margin-top: 3px; font-weight: 400; opacity: .65; overflow: hidden; text-overflow: ellipsis; }
    .empty { padding: 10px; font-weight: 400; opacity: .8; white-space: normal; }
    .toast { width: max-content; max-width: min(320px, 80vw); padding: 9px 11px; font-weight: 500;
      white-space: normal; background: rgba(127, 29, 29, .97); }
    [hidden] { display: none !important; }
  `;

  function svg(paths) {
    const el = document.createElementNS(SVG_NS, "svg");
    el.setAttribute("viewBox", "0 0 24 24");
    el.setAttribute("aria-hidden", "true");
    for (const d of paths) {
      const p = document.createElementNS(SVG_NS, "path");
      p.setAttribute("d", d);
      el.append(p);
    }
    return el;
  }
  function node(tag, className, text) {
    const el = document.createElement(tag);
    if (className) el.className = className;
    if (text != null) el.textContent = text;
    return el;
  }

  function buildUi() {
    const host = document.createElement("ldm-download-button");
    host.style.cssText =
      "all:initial !important; position:fixed !important; left:0 !important; top:0 !important;" +
      "z-index:2147483647 !important; display:none; pointer-events:auto !important;";
    const root = host.attachShadow({ mode: "closed" });

    let styled = false;
    try {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(CSS);
      root.adoptedStyleSheets = [sheet];
      styled = true;
    } catch {}
    if (!styled) root.append(node("style", null, CSS));

    const box = node("div", "box");
    const bar = node("div", "bar");
    const main = node("button", "main");
    main.type = "button";
    main.title = t("btnDownloadTitle", "Baixar com o LDM");
    const label = node("span", "label", t("btnDownload", "Baixar"));
    main.append(svg(["M12 4v11", "M7 10l5 5 5-5", "M5 20h14"]), label);
    const more = node("button", "more");
    more.type = "button";
    more.title = t("btnMore", "Escolher o que baixar");
    more.setAttribute("aria-haspopup", "menu");
    more.append(svg(["M6 9l6 6 6-6"]));
    bar.append(main, more);
    const menu = node("div", "menu");
    menu.setAttribute("role", "menu");
    menu.hidden = true;
    const toast = node("div", "toast");
    toast.hidden = true;
    box.append(bar, menu, toast);
    root.append(box);

    // Keep clicks away from the page (matters when we live inside a fullscreen player).
    for (const type of ["pointerdown", "pointerup", "mousedown", "mouseup", "click", "dblclick", "contextmenu"]) {
      box.addEventListener(type, (e) => e.stopPropagation());
    }
    main.addEventListener("click", (e) => {
      e.preventDefault();
      closeMenu();
      download(null);
    });
    more.addEventListener("click", (e) => {
      e.preventDefault();
      menuOpen ? closeMenu() : openMenu();
    });
    host.addEventListener("pointerenter", () => {
      overUi = true;
      cancelHide();
    });
    host.addEventListener("pointerleave", () => {
      overUi = false;
      if (!menuOpen) scheduleHide(HIDE_DELAY_MS);
    });
    root.addEventListener("keydown", (e) => {
      if (e.key === "Escape") closeMenu();
    });

    return { host, box, main, label, menu, toast };
  }

  function container() {
    const fs = document.fullscreenElement;
    // A <video> in fullscreen can't contain our element; other fullscreen
    // containers (most players) can.
    return fs && !(fs instanceof HTMLMediaElement) ? fs : document.documentElement;
  }
  function mount() {
    ui ??= buildUi();
    const parent = container();
    if (ui.host.parentNode !== parent) parent.append(ui.host);
  }
  document.addEventListener("fullscreenchange", () => visible && mount(), true);

  function attach(el) {
    if (!enabled) return;
    if (active !== el) {
      active = el;
      closeMenu();
    }
    mount();
    if (!visible) {
      visible = true;
      ui.host.style.display = "block";
      requestAnimationFrame(() => ui.box.classList.add("show"));
      requestAnimationFrame(follow);
    }
    position();
  }

  function position() {
    if (!active || !ui) return;
    if (!active.isConnected) return hide(true);
    const r = active.getBoundingClientRect();
    const w = ui.box.offsetWidth || 120;
    const h = ui.box.offsetHeight || 32;
    const margin = 8;
    let left = Math.min(r.right - w - margin, innerWidth - w - 4);
    left = Math.max(Math.max(r.left, 0) + 4, left);
    let top;
    if (active instanceof HTMLAudioElement) {
      top = r.top - h - 6 >= 4 ? r.top - h - 6 : r.bottom + 6;
    } else {
      top = Math.max(r.top, 0) + margin;
    }
    top = Math.min(Math.max(4, top), innerHeight - h - 4);
    ui.host.style.setProperty("transform", `translate(${Math.round(left)}px, ${Math.round(top)}px)`, "important");
  }

  // Follow scrolling, resizing and layout changes while visible.
  function follow() {
    if (!visible) return;
    position();
    requestAnimationFrame(follow);
  }

  function scheduleHide(delay) {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (!overUi && !menuOpen && !busy) hide(false);
    }, delay);
  }
  function cancelHide() {
    clearTimeout(hideTimer);
  }
  function hide(immediate) {
    clearTimeout(hideTimer);
    if (!ui || !visible) return;
    visible = false;
    closeMenu();
    ui.box.classList.remove("show");
    const finish = () => {
      if (!visible) ui.host.style.display = "none";
    };
    immediate ? finish() : setTimeout(finish, 160);
  }

  // ------------------------------------------------------------- actions
  function setState(state, text) {
    ui.box.classList.remove("ok", "err", "busy");
    if (state !== "idle") ui.box.classList.add(state);
    ui.label.textContent = text ?? t("btnDownload", "Baixar");
  }
  function showToast(message) {
    ui.toast.textContent = message;
    ui.toast.hidden = false;
    setTimeout(() => (ui.toast.hidden = true), 6000);
  }

  function mediaInfo(el) {
    return {
      src: el.currentSrc || el.src || "",
      mediaType: el instanceof HTMLVideoElement ? "video" : "audio",
      pageUrl: location.href,
      title: document.title,
      duration: Number.isFinite(el.duration) ? el.duration : null,
    };
  }

  async function download(choice) {
    if (!active || busy) return;
    busy = true;
    ui.toast.hidden = true;
    setState("busy", t("sending", "Enviando…"));
    try {
      const res = await api.runtime.sendMessage({ type: "download-media", choice, ...mediaInfo(active) });
      if (res?.ok) {
        setState("ok", res.pending ? t("sentConfirm", "Confirme no LDM") : t("sent", "Enviado"));
      } else {
        setState("err", t("failed", "Erro"));
        showToast(res?.error || "Erro desconhecido");
      }
    } catch (err) {
      setState("err", t("failed", "Erro"));
      showToast(String(err?.message || err));
    } finally {
      busy = false;
      setTimeout(() => setState("idle"), 2500);
      scheduleHide(HIDE_DELAY_MS + 2500);
    }
  }

  async function openMenu() {
    if (!active) return;
    menuOpen = true;
    cancelHide();
    ui.menu.replaceChildren(node("div", "empty", "…"));
    ui.menu.hidden = false;
    let items = [];
    try {
      const res = await api.runtime.sendMessage({ type: "media-candidates", ...mediaInfo(active) });
      items = res?.items ?? [];
    } catch {}
    if (!menuOpen) return;
    if (!items.length) {
      ui.menu.replaceChildren(node("div", "empty", t("noCandidates", "Nenhuma mídia detectada ainda.")));
      return;
    }
    ui.menu.replaceChildren(
      ...items.map((item) => {
        const button = node("button", "item", item.label);
        button.type = "button";
        button.setAttribute("role", "menuitem");
        button.title = item.url;
        if (item.detail) button.append(node("small", null, item.detail));
        button.addEventListener("click", (e) => {
          e.preventDefault();
          closeMenu();
          download(item.url);
        });
        return button;
      }),
    );
  }
  function closeMenu() {
    menuOpen = false;
    if (ui) ui.menu.hidden = true;
  }
})();
