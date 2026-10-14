// Background script (Chrome service worker / Firefox event page).
//
// - Sniffs media responses per tab, so a click on the overlay button can send
//   the real file or stream even when the <video> only shows a blob: URL.
// - Resolves what to download and hands it to the LDM app via native messaging.
// - Optionally takes over regular browser downloads, like IDM does.

const api = globalThis.browser ?? globalThis.chrome;
const HOST = "io.ldm.native";

const DEFAULT_SETTINGS = {
  intercept: true,
  overlay: true,
  extensions:
    "3gp 7z aac apk appimage avi bin bz2 deb dmg epub exe flac flatpak flv gz img iso jar m4a m4v mkv " +
    "mov mp3 mp4 mpeg mpg msi ogg ogv opus pkg rar rpm run tar tgz wav webm wma wmv xz zip zst",
};

// Sites where the media is best extracted from the page by yt-dlp (signed or
// fragmented URLs, separate audio/video tracks).
const PAGE_EXTRACTOR_HOSTS = [
  "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "dailymotion.com", "twitch.tv",
  "instagram.com", "facebook.com", "tiktok.com", "x.com", "twitter.com", "reddit.com",
  "soundcloud.com", "bilibili.com", "bandcamp.com", "rumble.com", "kick.com",
];
// Hosts whose media responses are fragments, never whole files.
const IGNORED_MEDIA_HOSTS = ["googlevideo.com", "fbcdn.net", "cdninstagram.com"];

const MANIFEST_TYPES = /mpegurl|dash\+xml/i;
const MEDIA_TYPES = /^(video|audio)\//i;
const SEGMENT_TYPES = /^(video\/(mp2t|iso\.segment)|audio\/(mp2t|iso\.segment))$/i;
const MANIFEST_EXT = /\.(m3u8|mpd)$/i;
const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|flv|avi|mp3|m4a|aac|ogg|oga|opus|wav|flac)$/i;
const SEGMENT_EXT = /\.(ts|m4s|m4f|cmfv|cmfa)$/i;
const MIN_MEDIA_BYTES = 256 * 1024;
const MAX_PER_TAB = 60;

function hostMatches(hostname, list) {
  return list.some((h) => hostname === h || hostname.endsWith("." + h));
}
function parseUrl(url) {
  try {
    return new URL(url);
  } catch {
    return null;
  }
}
function isExtractorPage(url) {
  const u = parseUrl(url);
  return Boolean(u && /^https?:$/.test(u.protocol) && hostMatches(u.hostname, PAGE_EXTRACTOR_HOSTS));
}
function message(key, fallback) {
  try {
    return api.i18n.getMessage(key) || fallback;
  } catch {
    return fallback;
  }
}

async function settings() {
  try {
    return { ...DEFAULT_SETTINGS, ...(await api.storage.local.get(DEFAULT_SETTINGS)) };
  } catch {
    return DEFAULT_SETTINGS;
  }
}

// ------------------------------------------------------------ media sniffing
// tabId -> [{ url, kind, contentType, size, frameId, ts }]
const mediaByTab = new Map();
let restored = null;

function restore() {
  restored ??= (async () => {
    try {
      const { media } = await api.storage.session.get("media");
      for (const [tab, list] of Object.entries(media ?? {})) {
        const current = mediaByTab.get(Number(tab)) ?? [];
        mediaByTab.set(Number(tab), [...list, ...current]);
      }
    } catch {}
  })();
  return restored;
}

let persistTimer = 0;
function persist() {
  clearTimeout(persistTimer);
  persistTimer = setTimeout(() => {
    api.storage.session?.set({ media: Object.fromEntries(mediaByTab) }).catch(() => {});
  }, 400);
}

function updateBadge(tabId) {
  const count = mediaByTab.get(tabId)?.length ?? 0;
  api.action.setBadgeText({ tabId, text: count ? String(count) : "" }).catch(() => {});
  api.action.setBadgeBackgroundColor?.({ tabId, color: "#2563eb" }).catch(() => {});
}

function headerValue(headers, name) {
  return headers?.find((h) => h.name.toLowerCase() === name)?.value ?? "";
}

function classify(details) {
  if (details.tabId < 0) return null;
  const okStatus = (details.statusCode >= 200 && details.statusCode < 300) || details.statusCode === 0;
  if (!okStatus) return null;
  const u = parseUrl(details.url);
  if (!u || !/^https?:$/.test(u.protocol) || hostMatches(u.hostname, IGNORED_MEDIA_HOSTS)) return null;

  const contentType = headerValue(details.responseHeaders, "content-type").split(";")[0].trim().toLowerCase();
  const path = u.pathname.toLowerCase();
  if (MANIFEST_TYPES.test(contentType) || MANIFEST_EXT.test(path)) {
    return { url: details.url, kind: "stream", contentType, size: null };
  }
  if (SEGMENT_TYPES.test(contentType) || SEGMENT_EXT.test(path)) return null;
  if (!MEDIA_TYPES.test(contentType) && !MEDIA_EXT.test(path)) return null;
  // Byte-range fragments requested by players through query parameters.
  if (/[?&](range|bytestart|byterange)=/i.test(u.search)) return null;

  const contentRange = headerValue(details.responseHeaders, "content-range");
  const total = Number(contentRange.split("/")[1]);
  const size = Number.isFinite(total) && total > 0 ? total : Number(headerValue(details.responseHeaders, "content-length")) || null;
  if (size !== null && size < MIN_MEDIA_BYTES) return null;
  return { url: details.url, kind: "direct", contentType, size };
}

async function record(tabId, frameId, candidate) {
  await restore();
  const list = mediaByTab.get(tabId) ?? [];
  const existing = list.find((c) => c.url === candidate.url);
  if (existing) {
    existing.size = candidate.size ?? existing.size;
  } else {
    list.push({ ...candidate, frameId, ts: Date.now() });
    if (list.length > MAX_PER_TAB) list.shift();
  }
  mediaByTab.set(tabId, list);
  persist();
  updateBadge(tabId);
}

api.webRequest.onHeadersReceived.addListener(
  (details) => {
    const candidate = classify(details);
    if (candidate) record(details.tabId, details.frameId, candidate);
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "object", "other"] },
  ["responseHeaders"],
);

api.tabs.onUpdated.addListener((tabId, change) => {
  if (change.url) {
    mediaByTab.delete(tabId);
    persist();
    updateBadge(tabId);
  }
});
api.tabs.onRemoved.addListener((tabId) => {
  mediaByTab.delete(tabId);
  persist();
});

// -------------------------------------------------------------- candidates
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

function describe(candidate) {
  const u = parseUrl(candidate.url);
  const name = decodeURIComponent(u?.pathname.split("/").filter(Boolean).pop() ?? candidate.url);
  const type =
    candidate.kind === "stream"
      ? /mpd/i.test(candidate.url) || /dash/i.test(candidate.contentType) ? "DASH" : "HLS"
      : (candidate.contentType.split("/")[1] || name.split(".").pop() || "").toUpperCase();
  return {
    url: candidate.url,
    kind: candidate.kind,
    label: name,
    detail: [type, formatSize(candidate.size), u?.hostname].filter(Boolean).join(" · "),
  };
}

async function candidatesFor(tabId, frameUrl, tabUrl) {
  await restore();
  const items = [];
  const pageUrl = isExtractorPage(frameUrl) ? frameUrl : tabUrl;
  if (pageUrl && /^https?:/.test(pageUrl)) {
    items.push({
      url: pageUrl,
      kind: "page",
      label: message("autoDetect", "Detectar automaticamente (melhor qualidade)"),
      detail: parseUrl(pageUrl)?.hostname ?? "",
    });
  }
  const sniffed = [...(mediaByTab.get(tabId) ?? [])].sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === "stream" ? -1 : 1;
    return a.kind === "stream" ? a.ts - b.ts : (b.size ?? 0) - (a.size ?? 0);
  });
  items.push(...sniffed.map(describe));
  return items;
}

/** Picks what the overlay button should download when the user did not choose. */
async function resolveTarget(msg, sender) {
  const tabId = sender.tab?.id;
  const frameUrl = sender.url || msg.pageUrl;
  if (isExtractorPage(frameUrl)) return { url: frameUrl, kind: "page" };
  if (/^https?:/i.test(msg.src)) {
    return { url: msg.src, kind: MANIFEST_EXT.test(parseUrl(msg.src)?.pathname ?? "") ? "stream" : "direct" };
  }
  await restore();
  const list = mediaByTab.get(tabId) ?? [];
  const sameFrame = list.filter((c) => c.frameId === sender.frameId);
  const pool = sameFrame.length ? sameFrame : list;
  // The first manifest of a page is usually the master playlist (all qualities).
  const stream = pool.filter((c) => c.kind === "stream").sort((a, b) => a.ts - b.ts)[0];
  if (stream) return stream;
  const direct = pool.filter((c) => c.kind === "direct").sort((a, b) => (b.size ?? 0) - (a.size ?? 0))[0];
  if (direct) return direct;
  // Nothing sniffed (e.g. the video has not started): let yt-dlp try the page.
  return { url: frameUrl, kind: "page" };
}

// ---------------------------------------------------------- talking to LDM
async function cookiesFor(url) {
  try {
    const cookies = await api.cookies.getAll({ url });
    return cookies.map(({ name, value, domain, path, secure, hostOnly, expirationDate }) => ({
      name, value, domain, path, secure, hostOnly, expirationDate,
    }));
  } catch {
    return [];
  }
}

async function sendToApp(payload) {
  let reply;
  try {
    reply = await api.runtime.sendNativeMessage(HOST, { type: "download", ...payload });
  } catch (err) {
    const text = String(err?.message || err);
    if (/not found|no such native application|access to the specified native messaging host is forbidden/i.test(text)) {
      throw new Error(message("hostMissing", "App LDM não encontrado. Instale-o e rode: ldm install-hosts"));
    }
    throw new Error(text);
  }
  if (reply?.type === "added") return reply.id;
  throw new Error(reply?.message || "resposta inválida do app");
}

async function sendMedia(target, { referrer, pageUrl, title }) {
  return sendToApp({
    url: target.url,
    kind: target.kind,
    referrer,
    pageUrl,
    title,
    userAgent: navigator.userAgent,
    cookies: await cookiesFor(target.url),
  });
}

async function downloadMedia(msg, sender) {
  const tabId = sender.tab?.id;
  let target;
  if (msg.choice) {
    await restore();
    target =
      (mediaByTab.get(tabId) ?? []).find((c) => c.url === msg.choice) ??
      (isExtractorPage(msg.choice) || msg.choice === (sender.url || msg.pageUrl) || msg.choice === sender.tab?.url
        ? { url: msg.choice, kind: "page" }
        : { url: msg.choice, kind: MANIFEST_EXT.test(parseUrl(msg.choice)?.pathname ?? "") ? "stream" : "direct" });
  } else {
    target = await resolveTarget(msg, sender);
  }
  const id = await sendMedia(target, {
    referrer: sender.url || msg.pageUrl,
    pageUrl: sender.tab?.url || msg.pageUrl,
    title: msg.title,
  });
  return { ok: true, id };
}

async function ping() {
  try {
    const reply = await api.runtime.sendNativeMessage(HOST, { type: "ping" });
    return { ok: reply?.type === "pong", version: reply?.version, daemon: reply?.daemon };
  } catch (err) {
    return { ok: false, error: String(err?.message || err) };
  }
}

async function handle(msg, sender) {
  switch (msg?.type) {
    case "media-candidates":
      return { ok: true, items: await candidatesFor(sender.tab?.id, sender.url || msg.pageUrl, sender.tab?.url) };
    case "download-media":
      return downloadMedia(msg, sender);
    case "popup-candidates": {
      const tab = await api.tabs.get(msg.tabId);
      return { ok: true, items: await candidatesFor(tab.id, tab.url, tab.url) };
    }
    case "popup-download": {
      const tab = await api.tabs.get(msg.tabId);
      const id = await sendMedia(msg.item, { referrer: tab.url, pageUrl: tab.url, title: tab.title });
      return { ok: true, id };
    }
    case "ping":
      return ping();
    default:
      return { ok: false, error: "mensagem desconhecida" };
  }
}

api.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  handle(msg, sender).then(sendResponse, (err) => sendResponse({ ok: false, error: String(err?.message || err) }));
  return true; // async response
});

// --------------------------------------------------- browser download takeover
const passThrough = new Set();

function extensionOf(name) {
  const base = (name || "").split(/[\\/]/).pop().split(/[?#]/)[0];
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

function shouldIntercept(item, prefs) {
  const allowed = new Set(prefs.extensions.split(/\s+/).filter(Boolean));
  const url = item.finalUrl || item.url;
  const ext = extensionOf(item.filename) || extensionOf(parseUrl(url)?.pathname);
  if (ext && allowed.has(ext)) return true;
  const mime = (item.mime || "").toLowerCase();
  return /^(video|audio)\//.test(mime) || /^application\/(zip|x-7z|x-rar|vnd\.rar|x-tar|gzip|x-xz|zstd|x-iso9660|x-msdownload|x-debian|x-rpm|x-apple-diskimage|octet-stream)/.test(mime);
}

api.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url)) return;
  if (passThrough.delete(url) || item.byExtensionId === api.runtime.id) return;
  const prefs = await settings();
  if (!prefs.intercept || item.state !== "in_progress" || !shouldIntercept(item, prefs)) return;

  try {
    await api.downloads.cancel(item.id);
  } catch {
    return; // already finished or not cancellable: leave it to the browser
  }
  api.downloads.erase({ id: item.id }).catch(() => {});
  try {
    await sendToApp({
      url,
      kind: "direct",
      referrer: item.referrer || undefined,
      fileName: extensionOf(item.filename) ? item.filename.split(/[\\/]/).pop() : undefined,
      userAgent: navigator.userAgent,
      cookies: await cookiesFor(url),
    });
  } catch (err) {
    console.warn("LDM: devolvendo o download ao navegador:", err);
    passThrough.add(url);
    api.downloads.download({ url }).catch(() => passThrough.delete(url));
  }
});

// ------------------------------------------------------------ context menus
api.runtime.onInstalled.addListener(async () => {
  await Promise.resolve(api.contextMenus.removeAll()).catch(() => {});
  api.contextMenus.create({ id: "ldm-link", title: message("menuLink", "Baixar link com o LDM"), contexts: ["link"] });
  api.contextMenus.create({ id: "ldm-media", title: message("menuMedia", "Baixar esta mídia com o LDM"), contexts: ["video", "audio"] });
});

api.contextMenus.onClicked.addListener(async (info, tab) => {
  try {
    if (info.menuItemId === "ldm-link" && info.linkUrl) {
      await sendToApp({
        url: info.linkUrl,
        kind: "direct",
        referrer: info.pageUrl,
        pageUrl: tab?.url,
        userAgent: navigator.userAgent,
        cookies: await cookiesFor(info.linkUrl),
      });
    } else if (info.menuItemId === "ldm-media") {
      const sender = { tab, url: info.frameUrl || info.pageUrl, frameId: info.frameId ?? 0 };
      await downloadMedia({ src: info.srcUrl, pageUrl: info.pageUrl, title: tab?.title }, sender);
    }
  } catch (err) {
    console.warn("LDM:", err);
  }
});
