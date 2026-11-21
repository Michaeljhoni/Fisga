// The gallery: every photo, video and audio of a tab (media.html?tab=ID),
// in tabs, to pick and send to Fisga in one go. The background finds them
// (galleryScan), checks the addresses (probeUrl) and sends the selection to
// the queue, into a folder named after the site (galleryDownload).
(() => {
  "use strict";
  const api = globalThis.browser ?? globalThis.chrome;
  const $ = (id) => document.getElementById(id);
  const t = (key, ...subs) => {
    try {
      return api.i18n.getMessage(key, subs.map(String)) || key;
    } catch {
      return key;
    }
  };

  const tabId = Number(new URLSearchParams(location.search).get("tab"));
  // Photos smaller than this (both sides) are icons, avatars and buttons.
  const SMALL = 150;
  // Address checks at the same time.
  const PROBES = 6;
  const TYPES = ["video", "image", "audio"];

  let page = null;
  let items = [];
  let current = "video";
  const selected = new Set();
  const cards = new Map();
  let anchor = null;
  let sending = false;

  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n) || el.textContent;
  for (const el of document.querySelectorAll("[data-i18n-placeholder]")) el.placeholder = t(el.dataset.i18nPlaceholder);
  document.title = t("galleryTitle");

  // ----------------------------------------------------------- describing
  function fileName(url) {
    try {
      const u = new URL(url);
      return decodeURIComponent(u.pathname.split("/").filter(Boolean).pop() || u.hostname);
    } catch {
      return url;
    }
  }
  function host(url) {
    try {
      return new URL(url).hostname.replace(/^www\./, "");
    } catch {
      return "";
    }
  }
  function formatSize(bytes) {
    if (!bytes) return "";
    const units = ["B", "KB", "MB", "GB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit++;
    }
    return `${value.toFixed(unit ? 1 : 0)} ${units[unit]}`;
  }
  function formatDuration(seconds) {
    if (!Number.isFinite(seconds) || seconds <= 0) return "";
    const s = Math.round(seconds);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const pad = (n) => String(n).padStart(2, "0");
    return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
  }
  /** The address to download: the one checked, else the one the page gave. */
  const addressOf = (item) => item.best || item.url;

  function format(item) {
    if (item.kind === "page") return item.generic ? t("galleryGeneric") : t("galleryViaPage");
    if (item.kind === "stream") {
      const kind = /\.mpd(\?|$)/i.test(item.url) || /dash/i.test(item.detail || "") ? "DASH" : "HLS";
      return item.resolution ? `${kind} ${item.resolution}p` : kind;
    }
    const ext = /\.(\w{2,5})$/.exec(fileName(addressOf(item)))?.[1];
    return ext ? ext.toUpperCase() : item.type === "image" ? t("galleryPhoto") : "";
  }
  function isSmall(item) {
    if (item.type !== "image") return false;
    if (item.checked && item.width && item.height) return item.width < SMALL && item.height < SMALL;
    if (item.original) return false;
    const w = item.width || item.shownWidth || 0;
    const h = item.height || item.shownHeight || 0;
    return w > 0 && h > 0 && w < SMALL && h < SMALL;
  }
  function visible(item) {
    if (item.dead) return false;
    if (isSmall(item) && !$("small").checked) return false;
    const text = $("filter").value.trim().toLowerCase();
    return !text || `${item.name || ""} ${item.url}`.toLowerCase().includes(text);
  }
  function typeIcon(type) {
    return type === "video" ? "🎬" : type === "audio" ? "🎵" : "🖼";
  }

  // --------------------------------------------------------------- cards
  function node(tag, className, text) {
    const el = document.createElement(tag);
    if (className) el.className = className;
    if (text != null) el.textContent = text;
    return el;
  }

  function fillCard(el, item) {
    const name = item.name || fileName(addressOf(item)) || host(item.url);
    const dims = item.width && item.height ? `${item.width}×${item.height}` : "";
    // What the network showed (streams) already says its type, size and host.
    const detail =
      item.source === "network" && item.detail
        ? item.detail
        : [dims, formatSize(item.size), host(addressOf(item))].filter(Boolean).join(" · ");
    el.querySelector(".name").textContent = name;
    el.querySelector(".detail").textContent = detail;
    el.title = addressOf(item);
    const badges = el.querySelector(".badges");
    badges.replaceChildren();
    const kind = format(item);
    if (kind) badges.append(node("span", "badge", kind));
    const duration = formatDuration(item.duration);
    if (duration) badges.append(node("span", "badge", duration));
    if (item.drm) badges.append(node("span", "badge drm", t("galleryDrm")));
    if (item.original) badges.append(node("span", "badge original", t("galleryOriginal")));
    if (item.sent) badges.append(node("span", "badge original", t("galleryQueued")));
    el.setAttribute("aria-selected", String(selected.has(item.url)));
  }

  function makeCard(item) {
    const el = node("div", "card");
    el.tabIndex = 0;
    el.setAttribute("role", "option");
    const thumb = node("div", "thumb");
    const icon = () => thumb.replaceChildren(node("span", "icon", typeIcon(item.type)), node("span", "check"), badges);
    const badges = node("div", "badges");
    if (item.thumb) {
      const img = document.createElement("img");
      img.loading = "lazy";
      img.decoding = "async";
      img.referrerPolicy = "no-referrer";
      img.alt = "";
      img.src = item.thumb;
      img.addEventListener("error", icon, { once: true });
      img.addEventListener(
        "load",
        () => {
          // A photo the page showed without saying its size: the thumbnail is it.
          if (item.type === "image" && !item.width && img.src === item.url) {
            item.width = img.naturalWidth;
            item.height = img.naturalHeight;
            fillCard(el, item);
          }
        },
        { once: true },
      );
      thumb.append(img, node("span", "check"), badges);
    } else {
      icon();
    }
    const meta = node("div", "meta");
    meta.append(node("div", "name"), node("div", "detail"));
    el.append(thumb, meta);
    el.addEventListener("click", (e) => toggle(item, e.shiftKey));
    el.addEventListener("dblclick", () => api.tabs.create({ url: addressOf(item), active: true }));
    el.addEventListener("keydown", (e) => {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        toggle(item, e.shiftKey);
      }
    });
    fillCard(el, item);
    observer.observe(el);
    return el;
  }

  function shown() {
    return items.filter((item) => item.type === current && visible(item));
  }

  function render() {
    observer.disconnect();
    for (const type of TYPES) {
      const tab = document.querySelector(`[data-tab="${type}"]`);
      const count = items.filter((item) => item.type === type && visible(item)).length;
      tab.hidden = !items.some((item) => item.type === type && !item.dead);
      tab.setAttribute("aria-selected", String(type === current));
      tab.querySelector(".count").textContent = `(${count})`;
    }
    $("small-wrap").hidden = current !== "image" || !items.some((item) => item.type === "image" && isSmall(item));
    $("select-all").hidden = $("select-none").hidden = !items.some((item) => !item.dead);
    const list = shown();
    const grid = $("grid");
    const fragment = document.createDocumentFragment();
    for (const item of list) {
      let el = cards.get(item.url);
      if (!el) {
        el = makeCard(item);
        cards.set(item.url, el);
      } else {
        observer.observe(el);
      }
      fragment.append(el);
    }
    grid.replaceChildren(fragment);
    const status = $("status");
    if (!items.length) {
      status.textContent = t("galleryNothing");
      status.hidden = false;
    } else if (!list.length) {
      status.textContent = t("galleryNothingHere");
      status.hidden = false;
    } else {
      status.hidden = true;
    }
    updateBar();
  }

  function updateBar() {
    const chosen = items.filter((item) => selected.has(item.url) && !item.dead);
    const counts = TYPES.map((type) => [type, chosen.filter((item) => item.type === type).length]).filter(([, n]) => n);
    const words = { video: "galleryCountVideos", image: "galleryCountPhotos", audio: "galleryCountAudio" };
    $("selected").textContent = chosen.length
      ? counts.map(([type, n]) => t(words[type], n)).join(", ")
      : t("galleryNoneSelected");
    $("dest").textContent = page ? t("galleryDest", page.folder) : "";
    const button = $("download");
    button.textContent = t("galleryDownload", chosen.length);
    button.disabled = !chosen.length || sending;
  }

  // ----------------------------------------------------------- selecting
  function toggle(item, range) {
    if (item.drm) return; // protected: it cannot be downloaded
    const list = shown().filter((other) => !other.drm);
    if (range && anchor && list.includes(anchor)) {
      const [a, b] = [list.indexOf(anchor), list.indexOf(item)].sort((x, y) => x - y);
      const on = selected.has(anchor.url);
      for (const other of list.slice(a, b + 1)) on ? selected.add(other.url) : selected.delete(other.url);
    } else {
      selected.has(item.url) ? selected.delete(item.url) : selected.add(item.url);
      anchor = item;
    }
    for (const other of list) cards.get(other.url)?.setAttribute("aria-selected", String(selected.has(other.url)));
    $("result").textContent = "";
    updateBar();
  }
  $("select-all").addEventListener("click", () => {
    for (const item of shown()) if (!item.drm) selected.add(item.url);
    render();
  });
  $("select-none").addEventListener("click", () => {
    for (const item of shown()) selected.delete(item.url);
    render();
  });
  for (const type of TYPES) {
    document.querySelector(`[data-tab="${type}"]`).addEventListener("click", () => {
      current = type;
      anchor = null;
      render();
    });
  }
  $("filter").addEventListener("input", render);
  $("small").addEventListener("change", render);
  $("refresh").addEventListener("click", scan);

  // ------------------------------------------------- originals and sizes
  // Each card on screen gets its address checked: the original of a
  // thumbnail (fullsize.js) when there is one, and the size of the file.
  const pending = [];
  let running = 0;
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const item = items.find((i) => cards.get(i.url) === entry.target);
        if (item) queueProbe(item);
      }
    },
    { rootMargin: "300px" },
  );
  function ask(url, light) {
    return api.runtime.sendMessage({ type: "gallery-probe", url, pageUrl: page?.url, light }).catch(() => ({ ok: false }));
  }
  /** Is what the server sent the kind of file the card says? */
  function accepts(item, type) {
    if (!type) return item.type !== "image" && !item.verify; // some servers do not say
    if (item.type === "image") return /^image\//.test(type);
    if (/^application\/(octet-stream|ogg|x-mpegurl|vnd\.apple\.mpegurl|dash\+xml)$|^binary\/octet-stream$/.test(type)) return true;
    return item.type === "audio" ? /^audio\//.test(type) : /^(video|audio)\//.test(type);
  }
  /**
   * The first candidate (or its original, by fullsize.js) that answers with
   * the right kind of file. A card whose candidates are all pages or missing
   * files is dropped; one whose server could not be asked stays as it is.
   */
  async function probe(item, light) {
    if (item.kind === "stream") return probeStream(item);
    if (item.kind !== "direct") return;
    const list = item.candidates?.length ? item.candidates : [item.url];
    let refused = 0;
    let asked = 0;
    for (const candidate of list) {
      for (const address of [...(globalThis.FisgaFullsize?.candidates(candidate) ?? []), candidate]) {
        const res = await ask(address, light);
        asked++;
        if (res.ok && accepts(item, res.type)) {
          item.best = address;
          item.size = res.size ?? item.size;
          if (address !== item.thumb && address !== list[list.length - 1]) item.original = address;
          // The file's own size, read from its first bytes.
          if (res.width && res.height) {
            item.width = res.width;
            item.height = res.height;
            item.checked = true;
          } else if (item.original) {
            item.width = 0;
            item.height = 0;
          }
          return;
        }
        if (res.ok || res.status === 404 || res.status === 410) refused++;
      }
    }
    // Where the checks cannot use the tab's cookies (a container, a private
    // window), a refusal may only mean "log in": the card stays.
    if (asked && refused === asked && !page?.ownStore) {
      item.dead = true;
      selected.delete(item.url);
    }
  }
  /** The best resolution of a stream, its protection, and the quality lists a master already holds. */
  async function probeStream(item) {
    const res = await api.runtime.sendMessage({ type: "gallery-manifest", url: item.url, pageUrl: page?.url }).catch(() => null);
    if (!res?.ok) return;
    // Its own field: a refresh renews the sizes the page reports.
    item.resolution = res.height || 0;
    item.drm = res.drm;
    if (item.drm) selected.delete(item.url);
    for (const variant of res.variants ?? []) {
      const copy = items.find((other) => other.url === variant && other !== item);
      if (copy) {
        copy.dead = true;
        selected.delete(copy.url);
      }
    }
    if (res.variants?.length) scheduleRender();
  }

  // Many checks end at once on big pages: one render per frame, not one each.
  let renderQueued = false;
  function scheduleRender() {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      render();
    });
  }

  function queueProbe(item, light = false) {
    if (item.probe) return item.probe;
    item.probe = new Promise((resolve) => pending.push({ item, light, resolve }));
    pump();
    return item.probe;
  }
  function pump() {
    while (running < PROBES && pending.length) {
      const { item, light, resolve } = pending.shift();
      running++;
      probe(item, light)
        .catch(() => {})
        .finally(() => {
          running--;
          const el = cards.get(item.url);
          if (el) fillCard(el, item);
          if (item.dead) scheduleRender();
          resolve();
          pump();
        });
    }
  }

  // ------------------------------------------------------------- sending
  $("download").addEventListener("click", async () => {
    const chosen = items.filter((item) => selected.has(item.url) && !item.dead);
    if (!chosen.length || sending) return;
    sending = true;
    updateBar();
    const result = $("result");
    result.className = "result";
    // The originals (and the real files behind links) of what was picked
    // before it showed on screen, and the protection of the streams.
    const checks = chosen.filter((item) => item.kind === "stream" || (item.kind === "direct" && (item.type === "image" || item.verify)));
    let done = 0;
    result.textContent = t("galleryChecking", 0, checks.length);
    await Promise.all(
      checks.map((item) =>
        queueProbe(item, true).then(() => {
          done++;
          result.textContent = t("galleryChecking", done, checks.length);
        }),
      ),
    );
    // Each file once: two thumbnails may lead to the same original.
    const unique = new Map();
    for (const item of chosen) if (!item.dead && !item.drm && !unique.has(addressOf(item))) unique.set(addressOf(item), item);
    const live = [...unique.values()];
    const left = chosen.length - live.length;
    for (const item of chosen) if (item.dead || item.drm) selected.delete(item.url);
    let res;
    try {
      res = await api.runtime.sendMessage({
        type: "gallery-download",
        tabId,
        pageUrl: page?.url,
        items: live.map((item) => ({
          url: addressOf(item),
          kind: item.kind,
          type: item.type,
          name: item.name,
          frameUrl: item.frameUrl,
        })),
      });
    } catch (err) {
      res = { ok: false, error: String(err?.message || err) };
    }
    // What the app took is queued, even when a later part failed.
    const taken = new Set(res?.sent ?? []);
    for (const item of chosen) {
      if (taken.has(addressOf(item))) {
        item.sent = true;
        selected.delete(item.url);
      }
    }
    if (res?.ok) {
      result.textContent = t("gallerySent", res.ids.length, res.folder) + (left ? " " + t("gallerySkipped", left) : "");
      result.className = "result ok";
    } else {
      result.textContent = res?.error || t("galleryFailed");
      result.className = "result err";
    }
    sending = false;
    render();
  });

  // ------------------------------------------------------------- scanning
  async function scan() {
    $("status").textContent = t("galleryScanning");
    $("status").hidden = false;
    $("result").textContent = "";
    if (!Number.isInteger(tabId)) {
      $("status").textContent = t("galleryNoTab");
      return;
    }
    let res;
    try {
      res = await api.runtime.sendMessage({ type: "gallery-scan", tabId });
    } catch (err) {
      res = { ok: false, error: String(err?.message || err) };
    }
    if (!res?.ok) {
      $("status").textContent = t("galleryScanFailed", res?.error || "?");
      return;
    }
    page = res.page;
    $("page").textContent = [page.title, host(page.url)].filter(Boolean).join(" — ");
    if (res.error && !res.items.length) {
      $("status").textContent = t("galleryScanFailed", res.error);
      items = [];
      $("grid").replaceChildren();
      updateBar();
      return;
    }
    if (res.blocked) {
      $("status").textContent = t("blockedSite");
      items = [];
      $("grid").replaceChildren();
      updateBar();
      return;
    }
    // A refresh keeps the same objects for what it found again: a check
    // still running writes into the object that will be sent, and the cards
    // and the selection stay theirs. Only what the page says is renewed.
    const before = new Map(items.map((item) => [item.url, item]));
    items = res.items.map((fresh) => {
      const old = before.get(fresh.url);
      if (!old) return { ...fresh };
      for (const key of ["thumb", "name", "frameUrl", "frameId", "duration", "detail", "source", "candidates", "verify", "generic"]) {
        if (fresh[key] !== undefined) old[key] = fresh[key];
      }
      if (!old.checked && !old.original) {
        for (const key of ["width", "height", "shownWidth", "shownHeight"]) old[key] = fresh[key];
      }
      if (!old.size && fresh.size) old.size = fresh.size;
      return old;
    });
    for (const key of [...selected]) if (!items.some((item) => item.url === key)) selected.delete(key);
    for (const key of [...cards.keys()]) if (!items.some((item) => item.url === key)) cards.delete(key);
    for (const item of items) {
      const el = cards.get(item.url);
      if (el) fillCard(el, item);
    }
    if (!items.some((item) => item.type === current)) current = TYPES.find((type) => items.some((item) => item.type === type)) ?? "video";
    render();
    // Streams first: their checks hide the quality lists a master holds.
    for (const item of items) if (item.kind === "stream") queueProbe(item);
  }

  scan();
})();
