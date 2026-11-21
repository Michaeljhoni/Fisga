// Floating "Download" button on top of videos and audio players.
//
// The button lives in a closed shadow root attached to <html> (or to the
// fullscreen element), so page styles can't touch it and it never sits inside
// the player's own DOM. The DOM is built with createElement only: sites like
// YouTube enforce Trusted Types and reject innerHTML.
(() => {
  "use strict";
  if (window.__fisgaOverlay) return;
  window.__fisgaOverlay = true;

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
  let dismissals = []; // { el, key }: videos whose button the user closed

  // ------------------------------------------------------------- dismissal
  // The × hides the button over one video until it changes: another source
  // or another address (YouTube opens the next video without reloading).
  // The popup's "show the button again" brings them all back.
  const dismissKey = (el) => `${el.currentSrc || el.src}|${location.href}`;
  function liveDismissals() {
    dismissals = dismissals.filter((d) => d.el.isConnected && d.key === dismissKey(d.el));
    return dismissals;
  }
  function isDismissed(el) {
    return dismissals.some((d) => d.el === el && d.key === dismissKey(el));
  }
  function dismiss() {
    if (!active) return;
    liveDismissals().push({ el: active, key: dismissKey(active) });
    overUi = false;
    hide(true);
  }

  // ---------------------------------------------------------------- settings
  api.storage?.local
    ?.get({ overlay: true })
    .then((s) => setEnabled(s.overlay !== false))
    .catch(() => {});
  api.storage?.onChanged?.addListener((changes, area) => {
    if (area === "local" && changes.overlay) setEnabled(changes.overlay.newValue !== false);
    if (area === "local" && changes.forceKey) forceKey = FORCE_KEYS[changes.forceKey.newValue] ?? null;
  });
  function setEnabled(value) {
    enabled = value;
    if (!enabled) hide(true);
  }

  // --------------------------------------------------------------- force key
  // Held while clicking a link, it sends the link to Fisga whatever it is (the
  // app's "Navegador" preferences). Alt+click would save it with the browser.
  const FORCE_KEYS = {
    alt: { alt: true, ctrl: false, shift: false },
    ctrl_alt: { alt: true, ctrl: true, shift: false },
    shift_alt: { alt: true, ctrl: false, shift: true },
    ctrl_shift: { alt: false, ctrl: true, shift: true },
  };
  let forceKey = FORCE_KEYS.alt;
  api.storage?.local
    ?.get({ forceKey: "alt" })
    .then((s) => (forceKey = FORCE_KEYS[s.forceKey] ?? null))
    .catch(() => {});
  window.addEventListener(
    "click",
    (e) => {
      const keys = forceKey;
      if (!keys || e.button !== 0 || e.metaKey) return;
      if (e.altKey !== keys.alt || e.ctrlKey !== keys.ctrl || e.shiftKey !== keys.shift) return;
      const link = e.composedPath().find((el) => el instanceof HTMLAnchorElement || el instanceof HTMLAreaElement);
      if (!link || !/^https?:/i.test(link.href)) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      api.runtime
        .sendMessage({ type: "force-link", url: link.href, pageUrl: location.href, linkTitle: link.textContent?.trim() || link.title })
        .then((res) => res?.ok || console.warn("Fisga:", res?.error))
        .catch(() => {});
    },
    true,
  );

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
    if (isDismissed(el)) return false;
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
    .more, .close { padding: 7px 8px; border-left: 1px solid rgba(255, 255, 255, .14); }
    svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 2.2;
      stroke-linecap: round; stroke-linejoin: round; flex: none; }
    .main svg { color: #60a5fa; }
    .more svg, .close svg { width: 13px; height: 13px; }
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
    const host = document.createElement("fisga-download-button");
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
    main.title = t("btnDownloadTitle", "Baixar com o Fisga");
    const label = node("span", "label", t("btnDownload", "Baixar"));
    main.append(svg(["M12 4v11", "M7 10l5 5 5-5", "M5 20h14"]), label);
    const more = node("button", "more");
    more.type = "button";
    more.title = t("btnMore", "Escolher o que baixar");
    more.setAttribute("aria-haspopup", "menu");
    more.append(svg(["M6 9l6 6 6-6"]));
    const close = node("button", "close");
    close.type = "button";
    close.title = t("btnClose", "Esconder neste vídeo");
    close.setAttribute("aria-label", close.title);
    close.append(svg(["M7 7l10 10", "M17 7L7 17"]));
    bar.append(main, more, close);
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
    close.addEventListener("click", (e) => {
      e.preventDefault();
      dismiss();
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

  // Links that point at a single video. Feed previews (YouTube home, search,
  // channel pages) play inside or next to a card that links to the video;
  // without it the button would send the feed itself.
  const VIDEO_LINK =
    'a[href*="/watch?v="], a[href*="/shorts/"], a[href*="/live/"], a[href*="youtu.be/"], ' +
    'a[href*="vimeo.com/"], a[href*="/video/"], a[href*="/videos/"], a[href*="/reel/"], a[href*="/clip/"]';
  const MAX_LINK_DEPTH = 14;

  function parentOf(node) {
    return node.parentElement ?? (node.getRootNode?.() instanceof ShadowRoot ? node.getRootNode().host : null);
  }

  /** The video page a media element belongs to, when it plays inside a feed. */
  function videoLink(el) {
    let node = el;
    for (let depth = 0; node && depth < MAX_LINK_DEPTH; depth++, node = parentOf(node)) {
      if (node === document.body || node === document.documentElement) break;
      const link = node.matches?.(VIDEO_LINK) ? node : node.querySelector?.(VIDEO_LINK);
      if (link?.href && link.href !== location.href) return { url: link.href, title: linkTitle(link) };
    }
    return null;
  }

  /** Title of the video a link points to: any link to the same video that carries one. */
  function linkTitle(link) {
    let key = link.pathname;
    try {
      key = new URL(link.href).searchParams.get("v") || link.pathname;
    } catch {}
    const clean = (s) => s?.replace(/\s+/g, " ").trim() || "";
    const links = [...document.querySelectorAll("a[href]")].filter((a) => a.href.includes(key));
    // The title link usually carries a title attribute; the thumbnail link only
    // has overlays such as the duration ("21:25") or "Tocando agora".
    for (const a of links) {
      const title = clean(a.getAttribute("title"));
      if (title.length > 3) return title;
    }
    for (const a of links) {
      const title = clean(a.textContent);
      if (title.length > 3 && !/\d+:\d{2}/.test(title)) return title;
    }
    return "";
  }

  function mediaInfo(el) {
    const link = videoLink(el);
    return {
      src: el.currentSrc || el.src || "",
      mediaType: el instanceof HTMLVideoElement ? "video" : "audio",
      pageUrl: location.href,
      linkUrl: link?.url ?? null,
      linkTitle: link?.title || null,
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
        setState("ok", res.pending ? t("sentConfirm", "Confirme no Fisga") : t("sent", "Enviado"));
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

  // "Baixar todos os links com o Fisga": the addresses this frame links to
  // (only the selected ones, when the menu came from a selection).
  function collectLinks(onlySelected) {
    const selection = getSelection();
    const ranges = onlySelected && selection ? [...Array(selection.rangeCount).keys()].map((i) => selection.getRangeAt(i)) : [];
    const urls = new Set();
    for (const el of document.querySelectorAll("a[href], area[href], video[src], audio[src], source[src]")) {
      if (onlySelected && !ranges.some((range) => range.intersectsNode(el))) continue;
      const url = el.href || el.src;
      if (/^(https?|ftp):/i.test(url)) urls.add(url);
      if (urls.size >= 5000) break;
    }
    return [...urls];
  }
  // ------------------------------------------------------- media gallery
  // Every photo, video and audio of this frame, for the gallery page
  // (media.html): the address of the real file when the page has it (the
  // largest of a srcset, the full image a thumbnail links to, the lazy-load
  // attributes), the thumbnail to show, and what is known of its size. The
  // background calls it in every frame (scripting.executeScript) and turns
  // thumbnails into originals (fullsize.js) before showing them.
  const IMAGE_EXT = /\.(jpe?g|jfif|png|gif|webp|avif|bmp|tiff?|heic|heif|jxl)$/i;
  const VIDEO_EXT = /\.(mp4|m4v|webm|mkv|mov|flv|avi|ogv|3gp|wmv)$/i;
  const AUDIO_EXT = /\.(mp3|m4a|aac|ogg|oga|opus|wav|flac|wma)$/i;
  const STREAM_EXT = /\.(m3u8|mpd)$/i;
  // Where galleries keep the full-size image (zoom, lightbox, WordPress).
  const FULL_ATTRS = [
    "data-full", "data-full-src", "data-fullsrc", "data-large", "data-large-file", "data-large_image",
    "data-orig-file", "data-original-src", "data-zoom-image", "data-zoom-src", "data-hi-res", "data-hires",
    "data-highres", "data-src-large", "data-big",
  ];
  // Where lazy-loading scripts keep the address until the image is on screen.
  const LAZY_ATTRS = [
    "data-src", "data-original", "data-lazy-src", "data-lazy", "data-image", "data-img", "data-img-src",
    "data-actualsrc", "data-echo", "data-pagespeed-lazy-src", "data-url",
  ];
  const LAZY_SRCSET_ATTRS = ["data-srcset", "data-lazy-srcset", "data-original-set"];
  const BG_ATTRS = ["data-bg", "data-background", "data-background-image", "data-bg-src"];
  const MAX_ITEMS = 3000;
  const MAX_BG_ELEMENTS = 4000;

  function absolute(url) {
    if (!url || typeof url !== "string") return "";
    try {
      return new URL(url.trim(), document.baseURI).href;
    } catch {
      return "";
    }
  }
  const isHttp = (url) => /^https?:/i.test(url);
  function pathOf(url) {
    try {
      return decodeURIComponent(new URL(url).pathname);
    } catch {
      return "";
    }
  }
  function nameOf(url) {
    return pathOf(url).split("/").filter(Boolean).pop() || "";
  }
  function typeOfUrl(url) {
    const path = pathOf(url);
    if (STREAM_EXT.test(path)) return { type: "video", kind: "stream" };
    if (VIDEO_EXT.test(path)) return { type: "video", kind: "direct" };
    if (AUDIO_EXT.test(path)) return { type: "audio", kind: "direct" };
    if (IMAGE_EXT.test(path)) return { type: "image", kind: "direct" };
    return null;
  }

  /** The candidates of a srcset, as the HTML spec splits them (URLs may hold commas). */
  function parseSrcset(value) {
    const out = [];
    const s = String(value || "");
    let i = 0;
    while (i < s.length) {
      while (i < s.length && /[\s,]/.test(s[i])) i++;
      if (i >= s.length) break;
      const start = i;
      while (i < s.length && !/\s/.test(s[i])) i++;
      let url = s.slice(start, i);
      let descriptor = "";
      if (/,+$/.test(url)) {
        url = url.replace(/,+$/, "");
      } else {
        const from = i;
        let depth = 0;
        while (i < s.length && (s[i] !== "," || depth > 0)) {
          if (s[i] === "(") depth++;
          else if (s[i] === ")") depth--;
          i++;
        }
        descriptor = s.slice(from, i).trim();
        i++;
      }
      const m = /^(\d+(?:\.\d+)?)([wx])$/i.exec(descriptor.split(/\s+/)[0] || "");
      out.push({ url, w: m && /w/i.test(m[2]) ? Number(m[1]) : 0, x: m && /x/i.test(m[2]) ? Number(m[1]) : m ? 0 : 1 });
    }
    return out;
  }
  /** The largest candidate of a srcset: by width, else by density. */
  function largestOf(srcset) {
    const list = parseSrcset(srcset).filter((c) => c.url && !/^data:/i.test(c.url));
    if (!list.length) return null;
    list.sort((a, b) => b.w - a.w || b.x - a.x);
    return { url: absolute(list[0].url), w: list[0].w, x: list[0].x };
  }

  /** Open shadow roots too: many sites build their players and grids inside them. */
  function* roots() {
    const stack = [document];
    let seen = 0;
    while (stack.length && seen < 200) {
      const root = stack.pop();
      seen++;
      yield root;
      for (const el of root.querySelectorAll("*")) if (el.shadowRoot) stack.push(el.shadowRoot);
    }
  }
  function queryAll(selector) {
    const out = [];
    for (const root of roots()) out.push(...root.querySelectorAll(selector));
    return out;
  }

  /** A small frame of a playing video, when the page lets us read it (same origin or CORS). */
  function frameOf(video) {
    try {
      if (!video.videoWidth || video.readyState < 2) return "";
      const scale = Math.min(1, 320 / video.videoWidth);
      const canvas = document.createElement("canvas");
      canvas.width = Math.round(video.videoWidth * scale);
      canvas.height = Math.round(video.videoHeight * scale);
      canvas.getContext("2d").drawImage(video, 0, 0, canvas.width, canvas.height);
      return canvas.toDataURL("image/jpeg", 0.7);
    } catch {
      return ""; // a cross-origin video taints the canvas
    }
  }

  // Single videos of the sites whose videos yt-dlp downloads from the page: in
  // a feed, each one is offered with its thumbnail. The address is the one
  // yt-dlp expects.
  const VIDEO_PAGES = [
    { re: /^https?:\/\/(?:www\.|m\.)?youtube\.com\/watch\?(?:.*&)?v=([\w-]{11})/, url: (m) => `https://www.youtube.com/watch?v=${m[1]}`, thumb: (m) => `https://i.ytimg.com/vi/${m[1]}/hqdefault.jpg` },
    { re: /^https?:\/\/(?:www\.|m\.)?youtube\.com\/(?:shorts|live|embed)\/([\w-]{11})/, url: (m) => `https://www.youtube.com/watch?v=${m[1]}`, thumb: (m) => `https://i.ytimg.com/vi/${m[1]}/hqdefault.jpg` },
    { re: /^https?:\/\/(?:www\.)?youtube-nocookie\.com\/embed\/([\w-]{11})/, url: (m) => `https://www.youtube.com/watch?v=${m[1]}`, thumb: (m) => `https://i.ytimg.com/vi/${m[1]}/hqdefault.jpg` },
    { re: /^https?:\/\/youtu\.be\/([\w-]{11})/, url: (m) => `https://www.youtube.com/watch?v=${m[1]}`, thumb: (m) => `https://i.ytimg.com/vi/${m[1]}/hqdefault.jpg` },
    { re: /^https?:\/\/(?:www\.)?vimeo\.com\/(\d{5,})(?:[/?#]|$)/, url: (m) => `https://vimeo.com/${m[1]}` },
    { re: /^https?:\/\/player\.vimeo\.com\/video\/(\d{5,})/, url: (m) => `https://vimeo.com/${m[1]}` },
    { re: /^https?:\/\/(?:www\.)?dailymotion\.com\/(?:embed\/)?video\/([a-z0-9]+)/i, url: (m) => `https://www.dailymotion.com/video/${m[1]}` },
    { re: /^https?:\/\/(?:www\.)?instagram\.com\/(?:[\w.]+\/)?(?:reels?|tv)\/([\w-]+)/, url: (m) => `https://www.instagram.com/reel/${m[1]}/` },
    { re: /^https?:\/\/(?:www\.)?tiktok\.com\/@[\w.-]+\/video\/(\d+)/, url: (m) => m[0] },
    { re: /^https?:\/\/(?:www\.|m\.)?facebook\.com\/(?:[\w.]+\/videos\/(?:[\w-]+\/)?\d+|watch\/?\?v=\d+|reel\/\d+)/, url: (m) => m[0] },
    { re: /^https?:\/\/(?:www\.)?twitch\.tv\/(?:videos\/\d+|[\w]+\/clip\/[\w-]+)/, url: (m) => m[0] },
    { re: /^https?:\/\/clips\.twitch\.tv\/[\w-]+/, url: (m) => m[0] },
    { re: /^https?:\/\/(?:www\.)?(?:x|twitter)\.com\/\w+\/status\/(\d+)\/video\/\d/, url: (m) => m[0].replace(/\/video\/\d$/, "") },
    { re: /^https?:\/\/(?:www\.)?bilibili\.com\/video\/(BV[\w]+|av\d+)/, url: (m) => `https://www.bilibili.com/video/${m[1]}` },
    { re: /^https?:\/\/(?:www\.)?rumble\.com\/v[\w-]+\.html/, url: (m) => m[0] },
  ];
  function videoPage(url) {
    for (const rule of VIDEO_PAGES) {
      const m = rule.re.exec(url);
      if (m) return { url: rule.url(m), thumb: rule.thumb?.(m) || "" };
    }
    return null;
  }

  function collectMedia() {
    const items = [];
    const seen = new Set();
    const clean = (s) => String(s || "").replace(/\s+/g, " ").trim().slice(0, 200);
    const add = (item) => {
      if (!item.url || seen.has(item.url) || items.length >= MAX_ITEMS) return;
      seen.add(item.url);
      items.push(item);
    };

    // Videos and audio players. A blob: or MediaSource player has no address
    // of its own: the feed link it plays under, the page itself (yt-dlp) or
    // the stream the background sniffed stand in for it. On a video's own
    // page the largest player is that video: the links inside it (end
    // screens, chapters) are other videos or the same one.
    const players = queryAll("video, audio");
    const here = videoPage(location.href);
    const area = (el) => {
      const r = el.getBoundingClientRect();
      return r.width * r.height;
    };
    const mainPlayer = here ? players.filter((el) => el instanceof HTMLVideoElement).sort((a, b) => area(b) - area(a))[0] : null;
    for (const el of players) {
      if (el.srcObject instanceof MediaStream) continue;
      const isVideo = el instanceof HTMLVideoElement;
      const sources = [el.currentSrc, el.src, ...[...el.querySelectorAll("source[src]")].map((s) => s.src)].map(absolute);
      const src = sources.find(isHttp) || "";
      const poster = isVideo ? absolute(el.poster) : "";
      const base = {
        type: isVideo ? "video" : "audio",
        thumb: poster || (isVideo ? frameOf(el) : ""),
        width: isVideo ? el.videoWidth || 0 : 0,
        height: isVideo ? el.videoHeight || 0 : 0,
        duration: Number.isFinite(el.duration) ? el.duration : null,
        name: clean(el.getAttribute("title") || el.getAttribute("aria-label")),
        source: "player",
      };
      if (src) {
        const known = typeOfUrl(src);
        add({ ...base, url: src, kind: known?.kind === "stream" ? "stream" : "direct" });
        continue;
      }
      // MediaWiki loads the sources on play; its file page leads to the file.
      const resource = absolute(el.getAttribute("resource"));
      if (isHttp(resource) && typeOfUrl(resource)) {
        add({ ...base, url: resource, kind: "direct", candidates: [resource], verify: true, name: base.name || nameOf(resource) });
        continue;
      }
      const link = el === mainPlayer ? null : videoLink(el);
      const page = videoPage(link?.url || "") || here;
      if (page) {
        // The page's own video is named by the page; a feed's, by its link.
        const title = page.url === here?.url ? clean(document.title.replace(/\s+[-|–]\s+[^-|–]+$/, "")) : link?.title;
        add({ ...base, url: page.url, kind: "page", thumb: base.thumb || page.thumb, name: title || base.name || clean(document.title) });
      } else {
        add({ ...base, url: `blob:${location.href}#${items.length}`, kind: "blob", name: base.name || clean(document.title) });
      }
    }

    // Videos of the page's feed (YouTube, Vimeo, TikTok, Instagram reels...)
    // and embedded players: one each, with its thumbnail.
    const titles = new Map();
    for (const a of queryAll("a[href]")) {
      const page = videoPage(a.href);
      if (!page) continue;
      const title = clean(a.getAttribute("title") || a.getAttribute("aria-label"));
      const text = clean(a.textContent);
      const best = title.length > 3 ? title : text.length > 3 && !/^\d+:\d{2}/.test(text) ? text : "";
      const entry = titles.get(page.url) ?? { page, title: "", thumb: "" };
      if (best && best.length > entry.title.length) entry.title = best;
      const img = a.querySelector("img");
      const shown = img && (largestOf(img.srcset)?.url || absolute(img.currentSrc || img.src));
      if (!entry.thumb && shown && isHttp(shown)) entry.thumb = shown;
      titles.set(page.url, entry);
    }
    for (const frame of queryAll("iframe[src], embed[src]")) {
      const page = videoPage(absolute(frame.getAttribute("src")));
      if (page && !titles.has(page.url)) titles.set(page.url, { page, title: clean(frame.title), thumb: "" });
    }
    for (const { page, title, thumb } of titles.values()) {
      add({ type: "video", kind: "page", url: page.url, thumb: page.thumb || thumb, name: title, source: "link" });
    }

    // Photos, the real file first: the image a thumbnail links to, then the
    // gallery's full-size attribute, then the widest of its srcset (or of its
    // <picture>) when wider than what shows, then the lazy-load address while
    // a placeholder shows, then what shows.
    for (const img of queryAll("img")) {
      const shown = absolute(img.currentSrc || img.src);
      const natural = { w: img.naturalWidth || 0, h: img.naturalHeight || 0 };
      const placeholder = !isHttp(shown) || natural.w <= 2;
      const linked = img.closest("a[href]");
      const link = linked && typeOfUrl(linked.href)?.type === "image" && isHttp(linked.href) ? linked.href : "";
      const full = FULL_ATTRS.map((attr) => absolute(img.getAttribute(attr))).find(isHttp) || "";
      const sets = [img.srcset || img.getAttribute("srcset"), ...LAZY_SRCSET_ATTRS.map((attr) => img.getAttribute(attr))];
      if (img.parentElement instanceof HTMLPictureElement) {
        for (const source of img.parentElement.querySelectorAll("source[srcset]")) sets.push(source.srcset);
      }
      const widest = sets.map(largestOf).filter((c) => c && isHttp(c.url)).sort((a, b) => b.w - a.w)[0];
      const lazy = LAZY_ATTRS.map((attr) => absolute(img.getAttribute(attr))).find(isHttp) || "";
      // A link may lead to a page about the photo (Wikipedia's "File:"), not
      // to the photo: the gallery checks each candidate in this order.
      const order = [];
      if (link) order.push({ url: link, from: "link" });
      if (full) order.push({ url: full, from: "full" });
      if (widest && widest.url !== shown && (widest.w > natural.w || (!widest.w && widest.x > 1) || placeholder)) {
        order.push({ ...widest, from: "srcset" });
      }
      if (lazy && placeholder) order.push({ url: lazy, from: "lazy" });
      if (!placeholder) order.push({ url: shown, w: natural.w, h: natural.h, from: "shown" });
      const candidates = [...new Set(order.map((c) => c.url))];
      if (!candidates.length || seen.has(candidates[0])) continue;
      const best = order[0];
      add({
        type: "image",
        kind: "direct",
        url: best.url,
        candidates,
        verify: best.from === "link",
        thumb: placeholder ? (order.find((c) => c.from !== "link") ?? best).url : shown,
        width: best.from === "shown" ? natural.w : best.w || 0,
        height: best.from === "shown" ? natural.h : 0,
        shownWidth: natural.w,
        shownHeight: natural.h,
        name: clean(img.alt || img.title),
        source: best.from,
      });
    }

    // Backgrounds: inline styles and lazy-load attributes, then the computed
    // style of elements big enough to be a photo (banners, covers, galleries).
    const bgUrl = (value) => /url\(\s*(['"]?)(.*?)\1\s*\)/i.exec(value || "")?.[2] || "";
    let checked = 0;
    for (const el of queryAll("body *")) {
      if (checked++ > MAX_BG_ELEMENTS) break;
      let url = "";
      for (const attr of BG_ATTRS) url ||= absolute(el.getAttribute(attr));
      if (!url && el.offsetWidth >= 120 && el.offsetHeight >= 120) url = absolute(bgUrl(getComputedStyle(el).backgroundImage));
      if (!isHttp(url)) continue;
      add({ type: "image", kind: "direct", url, thumb: url, width: 0, height: 0, name: clean(el.getAttribute("aria-label") || el.title), source: "background" });
    }

    // Links to media files ("download the original", galleries without a viewer).
    for (const a of queryAll("a[href]")) {
      const known = typeOfUrl(a.href);
      if (!known || !isHttp(a.href)) continue;
      add({
        ...known,
        url: a.href,
        candidates: [a.href],
        verify: true,
        thumb: known.type === "image" ? a.href : "",
        name: clean(a.textContent || a.title) || nameOf(a.href),
        source: "link",
      });
    }

    // What the page says it is about: og:image, og:video, JSON-LD.
    const meta = (name) => document.querySelector(`meta[property="${name}"], meta[name="${name}"]`)?.content;
    for (const name of ["og:video:secure_url", "og:video:url", "og:video", "twitter:player:stream"]) {
      const url = absolute(meta(name));
      const known = isHttp(url) && typeOfUrl(url);
      if (known && known.type === "video") add({ ...known, url, thumb: absolute(meta("og:image")), name: clean(meta("og:title") || document.title), source: "meta" });
    }
    for (const name of ["og:image:secure_url", "og:image", "twitter:image", "twitter:image:src"]) {
      const url = absolute(meta(name));
      if (isHttp(url)) add({ type: "image", kind: "direct", url, thumb: url, width: Number(meta("og:image:width")) || 0, height: Number(meta("og:image:height")) || 0, name: clean(meta("og:title") || document.title), source: "meta" });
    }
    for (const script of document.querySelectorAll('script[type="application/ld+json"]')) {
      let data;
      try {
        data = JSON.parse(script.textContent);
      } catch {
        continue;
      }
      const stack = [data];
      for (let guard = 0; stack.length && guard < 500; guard++) {
        const node = stack.pop();
        if (!node || typeof node !== "object") continue;
        if (Array.isArray(node)) {
          stack.push(...node);
          continue;
        }
        const type = String(node["@type"] || "");
        const url = absolute(typeof node.contentUrl === "string" ? node.contentUrl : "");
        const known = isHttp(url) && typeOfUrl(url);
        if (/VideoObject/.test(type) && known) add({ ...known, url, thumb: absolute([].concat(node.thumbnailUrl || [])[0]), name: clean(node.name), source: "meta" });
        if (/ImageObject/.test(type) && isHttp(url)) add({ type: "image", kind: "direct", url, thumb: url, width: Number(node.width) || 0, height: Number(node.height) || 0, name: clean(node.name || node.caption), source: "meta" });
        for (const value of Object.values(node)) if (value && typeof value === "object") stack.push(value);
      }
    }

    return { url: location.href, title: document.title, items };
  }
  // Called by the background with scripting.executeScript, in this same world.
  globalThis.__fisgaCollectMedia = collectMedia;

  api.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (msg?.type === "collect-media") {
      sendResponse(collectMedia());
    } else if (msg?.type === "collect-links") {
      sendResponse({ links: collectLinks(Boolean(msg.selection)) });
    } else if (msg?.type === "overlay-dismissed") {
      // Only frames with a closed button answer: the first answer wins.
      if (liveDismissals().length) sendResponse({ dismissed: true });
    } else if (msg?.type === "show-overlay") {
      dismissals = [];
      sendResponse({ ok: true });
    }
  });
})();
