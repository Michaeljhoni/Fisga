// Background script (Chrome service worker / Firefox event page).
//
// - Sniffs media responses per tab, so a click on the overlay button can send
//   the real file or stream even when the <video> only shows a blob: URL.
// - Resolves what to download and hands it to the Fisga app via native messaging.
// - Optionally takes over regular browser downloads, like IDM does: the file
//   types and the force key come from the app's "Navegador" preferences.

const api = globalThis.browser ?? globalThis.chrome;
const HOST = "io.fisga.native";

const DEFAULT_SETTINGS = {
  intercept: true,
  overlay: true,
  // Open the "Novo download" window in the app before starting (like IDM).
  confirm: true,
  // The app's "Navegador" preferences (copied by syncWithApp); these serve
  // until the app answers. Keep them like DEFAULT_FILE_TYPES in fisga-ipc.
  fileTypes: (
    "3gp 7z aac apk appimage avi bin bz2 ckpt deb dmg epub exe flac flatpak flv gguf gz img iso jar m4a m4v mkv " +
    "mov mp3 mp4 mpeg mpg msi ogg ogv onnx opus pkg pt pth rar rpm run safetensors tar tgz wav webm wma wmv xz zip zst"
  ).split(" "),
  forceKey: "alt",
  // Sites whose downloads stay with the browser (the app's "Sites excluídos").
  excludedSites: [],
};

// Sites where the media is best extracted from the page by yt-dlp (signed or
// fragmented URLs, separate audio/video tracks).
const PAGE_EXTRACTOR_HOSTS = [
  "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "dailymotion.com", "twitch.tv",
  "instagram.com", "facebook.com", "tiktok.com", "x.com", "twitter.com", "reddit.com",
  "soundcloud.com", "bilibili.com", "bandcamp.com", "rumble.com", "kick.com",
];
// Sites this build never downloads from. Empty here; the Chrome Web Store
// package lists YouTube in it (build.sh), as that store's rules require.
const BLOCKED_HOSTS = [];
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
/** On a site this build does not download from (or a subdomain of one). */
function isBlocked(url) {
  const u = parseUrl(url ?? "");
  return Boolean(u && hostMatches(u.hostname.toLowerCase(), BLOCKED_HOSTS));
}
function isExtractorPage(url) {
  const u = parseUrl(url);
  return Boolean(u && /^https?:$/.test(u.protocol) && hostMatches(u.hostname, PAGE_EXTRACTOR_HOSTS));
}

// Pages of one video (or post, clip, track) on the PAGE_EXTRACTOR_HOSTS sites,
// like content.js's VIDEO_PAGES. Their other pages (home, feeds, profiles,
// channels, searches) list many: sending them to yt-dlp would download every
// video in the list.
const YOUTUBE_HOSTS = ["youtube.com", "youtube-nocookie.com", "youtu.be"];
function isYoutubeVideo(u) {
  if (u.hostname === "youtu.be" || u.hostname.endsWith(".youtu.be")) return u.pathname.length > 1;
  if (u.pathname === "/watch") return u.searchParams.has("v");
  return /^\/(shorts|live|embed|v)\/[\w-]{6,}/.test(u.pathname);
}
const SINGLE_PAGES = [
  { hosts: YOUTUBE_HOSTS, test: isYoutubeVideo },
  {
    hosts: ["vimeo.com"],
    test: (u) => /^\/(?:\d+(?:\/\w+)?\/?$|channels\/[\w-]+\/\d+|(?:[\w-]+\/)*videos?\/\d+|[\w-]+\/review\/\d+|ondemand\/[\w-]+\/\d+|event\/\d+)/.test(u.pathname),
  },
  { hosts: ["dailymotion.com"], test: (u) => /^\/(?:embed\/)?video\/\w+/.test(u.pathname) || u.searchParams.has("video") },
  { hosts: ["instagram.com"], test: (u) => /^\/(?:(?:[\w.]+\/)?(?:p|reels?|tv)\/[\w-]+|stories\/[\w.]+\/\d+)/.test(u.pathname) },
  {
    hosts: ["tiktok.com"],
    test: (u) => /^v[mt]\./.test(u.hostname) || /^\/(?:@[\w.-]+\/(?:video|photo)\/\d+|@[\w.-]+\/live|embed(?:\/v2)?\/\d+|v\/\d+|t\/\w+|player\/v1\/\d+)/.test(u.pathname),
  },
  {
    hosts: ["facebook.com"],
    test: (u) =>
      /^\/(?:[\w.-]+\/videos\/(?:[\w.-]+\/)?\d+|reel\/\d+|share\/[rv]\/\w+|groups\/[\w.-]+\/(?:permalink|posts)\/\d+|[\w.-]+\/posts\/[\w-]+)/.test(u.pathname) ||
      (/^\/(?:watch(?:\/live)?\/?|video\.php)$/.test(u.pathname) && u.searchParams.has("v")) ||
      u.searchParams.has("story_fbid"),
  },
  {
    hosts: ["twitch.tv"],
    test: (u) =>
      u.hostname.startsWith("clips.") ? u.pathname.length > 1 || u.searchParams.has("clip")
      : u.hostname.startsWith("player.") ? ["video", "channel", "clip"].some((p) => u.searchParams.has(p))
      : /^\/(?:videos\/\d+|\w+\/(?:clip|v)\/[\w-]+|(?!(?:directory|search|settings|subscriptions|inventory|wallet|drops|downloads|jobs|p|turbo|prime|store|friends|messages|following|videos)\/?$)\w+\/?$)/.test(u.pathname),
  },
  { hosts: ["x.com", "twitter.com"], test: (u) => /^\/(?:(?:\w+|i(?:\/web)?)\/status\/\d+|i\/(?:broadcasts|spaces)\/\w+)/.test(u.pathname) },
  { hosts: ["reddit.com"], test: (u) => /^\/(?:(?:r|u|user)\/[\w-]+\/)?(?:comments|s)\/\w+/.test(u.pathname) },
  {
    hosts: ["bilibili.com"],
    test: (u) =>
      /^\/(?:video\/(?:BV\w+|av\d+)|bangumi\/play\/ep\d+)/i.test(u.pathname) ||
      (u.hostname.startsWith("player.") && (u.searchParams.has("bvid") || u.searchParams.has("aid"))) ||
      (u.hostname.startsWith("live.") && /^\/\d+/.test(u.pathname)),
  },
  { hosts: ["rumble.com"], test: (u) => /^\/(?:v[\w-]+\.html|embed\/[\w.-]+)/.test(u.pathname) },
  {
    hosts: ["soundcloud.com"],
    test: (u) =>
      u.hostname.startsWith("w.")
        ? /\/tracks\//.test(u.searchParams.get("url") ?? "")
        : /^\/(?!(?:discover|stream|feed|you|search|charts|tags|settings)\/)[\w-]+\/(?!(?:sets|likes|tracks|albums|reposts|popular-tracks|followers|following|comments|spotlight|toptracks)\/?$)[\w-]+\/?$/.test(u.pathname),
  },
  { hosts: ["bandcamp.com"], test: (u) => /^\/track\/|\/track=\d+/.test(u.pathname) },
  {
    hosts: ["kick.com"],
    test: (u) =>
      u.searchParams.has("clip") ||
      /^\/(?:video\/[\w-]+|[\w-]+\/(?:videos|clips)\/[\w-]+|(?!(?:video|videos|categories|category|search|browse|following|auth)\/?$)[\w-]+\/?$)/.test(u.pathname),
  },
];
/** False for pages known to list many videos, where the button must not send the page itself. */
function isSingleVideoPage(url) {
  const u = parseUrl(url);
  if (!u) return false;
  const rule = SINGLE_PAGES.find((r) => hostMatches(u.hostname, r.hosts));
  return rule ? rule.test(u) : true;
}
/** A link to one video of a site yt-dlp downloads from. */
function isVideoLink(url) {
  return isExtractorPage(url) && isSingleVideoPage(url);
}
/** How the app fetches a link: a video's page (yt-dlp), a playlist, or the file itself. */
function linkKind(url) {
  if (isVideoLink(url)) return "page";
  return MANIFEST_EXT.test(parseUrl(url)?.pathname ?? "") ? "stream" : "direct";
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

// Asks the app for its "Navegador" preferences at most this often; the
// content scripts read the force key from storage.
const SYNC_EVERY_MS = 5000;
let lastSync = 0;
let syncing = null;
// Whether the last try reached the app (false: not installed, or its host fails).
let appReachable = true;

/** Copies the app's file types and force key; without the app, keeps the last ones. `force` skips the wait. */
function syncWithApp(force = false) {
  if (syncing || (!force && Date.now() - lastSync < SYNC_EVERY_MS)) return syncing ?? Promise.resolve();
  lastSync = Date.now();
  syncing = (async () => {
    let reply;
    try {
      reply = await api.runtime.sendNativeMessage(HOST, { type: "settings" });
      appReachable = true;
    } catch {
      appReachable = false;
      return;
    }
    if (reply?.type === "settings" && Array.isArray(reply.fileTypes)) {
      await api.storage.local
        .set({
          fileTypes: reply.fileTypes,
          forceKey: reply.forceKey ?? "none",
          excludedSites: Array.isArray(reply.excludedSites) ? reply.excludedSites : [],
        })
        .catch(() => {});
    }
  })().finally(() => (syncing = null));
  return syncing;
}

api.runtime.onStartup?.addListener(() => syncWithApp());
api.tabs.onActivated.addListener(() => syncWithApp());

// ------------------------------------------------------------ media sniffing
// tabId -> [{ url, kind, contentType, size, frameId, ts }]
const mediaByTab = new Map();
// tabId -> the address its media belongs to (a new one clears the list).
const pageByTab = new Map();
let restored = null;

function restore() {
  restored ??= (async () => {
    try {
      const { media, pages } = await api.storage.session.get(["media", "pages"]);
      for (const [tab, list] of Object.entries(media ?? {})) {
        const current = mediaByTab.get(Number(tab)) ?? [];
        mediaByTab.set(Number(tab), [...list, ...current]);
      }
      for (const [tab, url] of Object.entries(pages ?? {})) {
        if (!pageByTab.has(Number(tab))) pageByTab.set(Number(tab), url);
      }
    } catch {}
  })();
  return restored;
}

let persistTimer = 0;
function persist() {
  clearTimeout(persistTimer);
  persistTimer = setTimeout(() => {
    api.storage.session
      ?.set({ media: Object.fromEntries(mediaByTab), pages: Object.fromEntries(pageByTab) })
      .catch(() => {});
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

/** Under the folder of a manifest: a segment of that stream (fMP4), not a file of its own. */
function underManifest(url, manifestUrl) {
  const u = parseUrl(url);
  const m = parseUrl(manifestUrl);
  if (!u || !m || u.origin !== m.origin) return false;
  const folder = m.pathname.slice(0, m.pathname.lastIndexOf("/") + 1);
  return folder.length > 1 && u.pathname.startsWith(folder);
}

async function record(tabId, frameId, candidate) {
  await restore();
  const list = mediaByTab.get(tabId) ?? [];
  const existing = list.find((c) => c.url === candidate.url);
  if (existing) {
    existing.size = candidate.size ?? existing.size;
  } else {
    if (candidate.kind === "direct" && list.some((c) => c.kind === "stream" && underManifest(candidate.url, c.url))) return;
    list.push({ ...candidate, frameId, ts: Date.now() });
    // Full: the oldest file goes, then the oldest stream (the first manifest
    // is usually the master playlist).
    if (list.length > MAX_PER_TAB) {
      const file = list.findIndex((c) => c.kind === "direct");
      list.splice(file >= 0 ? file : 0, 1);
    }
  }
  mediaByTab.set(tabId, list);
  persist();
  updateBadge(tabId);
}

api.webRequest.onHeadersReceived.addListener(
  (details) => {
    if (isBlocked(details.initiator ?? details.documentUrl)) return;
    const candidate = classify(details);
    // documentUrl (Firefox): the frame's address, the referrer this file goes with.
    if (candidate) record(details.tabId, details.frameId, { ...candidate, frameUrl: details.documentUrl });
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "object", "other"] },
  ["responseHeaders"],
);

const withoutHash = (url) => String(url).split("#")[0];

// The stored lists come first: a navigation may wake the worker with empty
// maps, and saving those would wipe every other tab's media.
api.tabs.onUpdated.addListener(async (tabId, change) => {
  if (!change.url) return;
  await restore();
  // Only the fragment changed (#t=30, #comments): same page, same player.
  const before = pageByTab.get(tabId);
  if (before && withoutHash(before) === withoutHash(change.url)) return;
  pageByTab.set(tabId, change.url);
  mediaByTab.delete(tabId);
  persist();
  updateBadge(tabId);
});
api.tabs.onRemoved.addListener(async (tabId) => {
  await restore();
  mediaByTab.delete(tabId);
  pageByTab.delete(tabId);
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
  let name = u?.pathname.split("/").filter(Boolean).pop() ?? candidate.url;
  try {
    name = decodeURIComponent(name);
  } catch {} // a malformed escape (%E0%A4%A): the name as it is
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

async function candidatesFor(tabId, frameUrl, tabUrl, linkUrl) {
  if (isBlocked(tabUrl) || isBlocked(frameUrl)) return [];
  await restore();
  const items = [];
  let pageUrl = isExtractorPage(frameUrl) ? frameUrl : tabUrl;
  if (isVideoLink(linkUrl) && !isSingleVideoPage(pageUrl)) pageUrl = linkUrl;
  if (pageUrl && /^https?:/.test(pageUrl) && isSingleVideoPage(pageUrl)) {
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
  return items.filter((item) => !isBlocked(item.url));
}

/**
 * Current URL of the frame that sent `msg`. The content script reports
 * `location.href`; `sender.url` is where the frame was loaded and goes stale on
 * single-page sites (YouTube navigates from the home page to a video with
 * history.pushState), which made a watch page look like the feed.
 */
function frameUrlOf(msg, sender) {
  return msg.pageUrl || sender.url;
}

/** Picks what the overlay button should download when the user did not choose. */
async function resolveTarget(msg, sender) {
  const tabId = sender.tab?.id;
  const frameUrl = frameUrlOf(msg, sender);
  // A preview playing in a feed: download the video it links to.
  if (isVideoLink(msg.linkUrl) && !isSingleVideoPage(frameUrl)) {
    return { url: msg.linkUrl, kind: "page", title: msg.linkTitle };
  }
  if (isExtractorPage(frameUrl)) {
    if (!isSingleVideoPage(frameUrl)) {
      throw new Error(message("notAVideo", "Esta página é uma lista de vídeos. Abra o vídeo (ou passe o mouse sobre a prévia dele) e tente de novo."));
    }
    return { url: frameUrl, kind: "page" };
  }
  if (/^https?:/i.test(msg.src)) {
    return { url: msg.src, kind: MANIFEST_EXT.test(parseUrl(msg.src)?.pathname ?? "") ? "stream" : "direct" };
  }
  await restore();
  const list = mediaByTab.get(tabId) ?? [];
  const sameFrame = list.filter((c) => c.frameId === sender.frameId);
  const pool = sameFrame.length ? sameFrame : list;
  // What the player loaded since it last started loading (content.js's
  // loadedAt); without that time, everything the frame loaded.
  const since = Number(msg.loadedAt) || 0;
  const fresh = pool.filter((c) => c.ts >= since);
  const streams = (l) => l.filter((c) => c.kind === "stream").sort((a, b) => a.ts - b.ts);
  const largest = (l) => l.filter((c) => c.kind === "direct").sort((a, b) => (b.size ?? 0) - (a.size ?? 0))[0];
  // The first manifest is usually the master playlist (all qualities); when
  // nothing came since, the latest one is the closest guess.
  const found = streams(fresh)[0] ?? largest(fresh) ?? streams(pool).at(-1) ?? largest(pool);
  if (found) return found;
  // Nothing sniffed (e.g. the video has not started): let yt-dlp try the page.
  return { url: frameUrl, kind: "page" };
}

// ---------------------------------------------------------- talking to Fisga
/** The cookies the browser sends to `url`, from that store (a container, a private window); none: the default one. */
async function cookiesFor(url, storeId) {
  try {
    const cookies = await api.cookies.getAll(storeId ? { url, storeId } : { url });
    return cookies.map(({ name, value, domain, path, secure, hostOnly, expirationDate }) => ({
      name, value, domain, path, secure, hostOnly, expirationDate,
    }));
  } catch {
    return [];
  }
}

/** The cookie store of a tab: its container (Firefox) or the incognito one; undefined: the default one. */
async function cookieStoreOf(tab) {
  if (tab?.cookieStoreId) return tab.cookieStoreId;
  if (tab?.id == null || !tab.incognito) return undefined;
  const stores = await api.cookies.getAllCookieStores().catch(() => []);
  return stores.find((store) => store.tabIds?.includes(tab.id))?.id;
}

/** The address of the frame a sniffed file or stream came from: Firefox recorded it, a Chrome frame is asked. */
async function frameUrlOfCandidate(tab, candidate) {
  if (candidate.frameUrl) return candidate.frameUrl;
  if (!candidate.frameId) return tab?.url ?? null;
  if (tab?.id == null) return null;
  const [frame] = await api.scripting
    .executeScript({ target: { tabId: tab.id, frameIds: [candidate.frameId] }, func: () => location.href })
    .catch(() => []);
  return frame?.result ?? null;
}

/** Resolves to { id, pending }: pending = the app is asking the user to confirm. */
async function sendToApp(payload) {
  if (isBlocked(payload.url)) {
    throw new Error(message("blockedSite", "Esta versão da extensão, da Chrome Web Store, não baixa do YouTube."));
  }
  const prefs = await settings();
  let reply;
  try {
    reply = await api.runtime.sendNativeMessage(HOST, { type: "download", confirm: prefs.confirm !== false, ...payload });
  } catch (err) {
    const text = String(err?.message || err);
    if (/not found|no such native application|access to the specified native messaging host is forbidden/i.test(text)) {
      throw new Error(message("hostMissing", "App Fisga não encontrado. Instale-o e rode: fisga install-hosts"));
    }
    throw new Error(text);
  }
  if (reply?.type === "added") return { id: reply.id, pending: Boolean(reply.pending) };
  throw new Error(reply?.message || "resposta inválida do app");
}

async function sendMedia(target, { referrer, pageUrl, title, tab }) {
  return sendToApp({
    url: target.url,
    kind: target.kind,
    referrer,
    pageUrl,
    title,
    userAgent: navigator.userAgent,
    cookies: await cookiesFor(target.url, await cookieStoreOf(tab)),
  });
}

async function downloadMedia(msg, sender) {
  const tabId = sender.tab?.id;
  let target;
  if (msg.choice) {
    await restore();
    target =
      (mediaByTab.get(tabId) ?? []).find((c) => c.url === msg.choice) ??
      (isExtractorPage(msg.choice) || msg.choice === frameUrlOf(msg, sender) || msg.choice === sender.tab?.url
        ? { url: msg.choice, kind: "page" }
        : { url: msg.choice, kind: MANIFEST_EXT.test(parseUrl(msg.choice)?.pathname ?? "") ? "stream" : "direct" });
  } else {
    target = await resolveTarget(msg, sender);
  }
  // What another frame loaded goes with that frame's address.
  const otherFrame = target.frameId != null && target.frameId !== sender.frameId;
  const { id, pending } = await sendMedia(target, {
    referrer: (otherFrame && (await frameUrlOfCandidate(sender.tab, target))) || frameUrlOf(msg, sender),
    pageUrl: sender.tab?.url || msg.pageUrl,
    title: target.title || (target.url === msg.linkUrl && msg.linkTitle) || msg.title,
    tab: sender.tab,
  });
  return { ok: true, id, pending };
}

async function ping() {
  try {
    const reply = await api.runtime.sendNativeMessage(HOST, { type: "ping" });
    return { ok: reply?.type === "pong", version: reply?.version, daemon: reply?.daemon };
  } catch (err) {
    return { ok: false, error: String(err?.message || err) };
  }
}

// ------------------------------------------------------------ media gallery
// media.html lists every photo, video and audio of a tab, and sends the ones
// the user picks to Fisga in one go: into the queue, each in its category's
// folder, inside a folder named after the site.

/** The site's folder name: its host without "www." ("pt.wikipedia.org", "instagram.com"). */
function siteFolder(url) {
  const host = parseUrl(url)?.hostname ?? "";
  return host.replace(/^www\d*\./, "") || "site";
}

/** Opens the gallery of `tab` next to it. */
async function openGallery(tab) {
  if (!tab?.id) return;
  await api.tabs.create({
    url: api.runtime.getURL(`media.html?tab=${tab.id}`),
    index: (tab.index ?? 0) + 1,
    openerTabId: tab.id,
    windowId: tab.windowId,
  });
}

/**
 * What every frame of the tab has (content.js's collectMedia), plus the files
 * and streams the tab loaded (the sniffing above). A player with no address
 * of its own (blob:) becomes the stream it loaded, or the page for yt-dlp.
 */
async function galleryScan(tabId) {
  const tab = await api.tabs.get(tabId);
  if (isBlocked(tab.url)) return { ok: true, blocked: true, page: { url: tab.url, title: tab.title, folder: siteFolder(tab.url) }, items: [] };
  let frames = [];
  let error = null;
  try {
    frames = await api.scripting.executeScript({
      target: { tabId, allFrames: true },
      func: () => globalThis.__fisgaCollectMedia?.() ?? null,
    });
  } catch (err) {
    error = String(err?.message || err);
  }
  // Without the function in that world (Firefox, or a frame where the page
  // script did not run yet): ask the frame's content script.
  for (const frame of frames) {
    if (frame.result == null && frame.frameId != null) {
      frame.result = await api.tabs.sendMessage(tabId, { type: "collect-media" }, { frameId: frame.frameId }).catch(() => null);
    }
  }
  if (!frames.length && !error) error = "nenhum quadro respondeu";

  await restore();
  const sniffed = mediaByTab.get(tabId) ?? [];
  const items = [];
  const seen = new Set();
  const push = (item) => {
    if (!item?.url || seen.has(item.url) || isBlocked(item.url)) return;
    seen.add(item.url);
    items.push(item);
  };
  // Frames that already have a video with an address: a player without one
  // there is the same video (or one we cannot get); it is not offered twice.
  const hasVideo = (frame) =>
    sniffed.some((c) => c.frameId === frame.frameId) ||
    (frame.result?.items ?? []).some((item) => item.type !== "image" && item.kind !== "blob");
  for (const frame of frames) {
    const result = frame.result;
    if (!result?.items) continue;
    const frameUrl = result.url || tab.url;
    for (const item of result.items) {
      if (item.kind === "blob") {
        if (isExtractorPage(frameUrl) && isSingleVideoPage(frameUrl)) {
          push({ ...item, url: frameUrl, kind: "page", frameUrl, frameId: frame.frameId });
        } else if (hasVideo(frame)) {
          continue;
        } else if (/^https?:/.test(frameUrl)) {
          // yt-dlp's generic extractor may still find it in the page.
          push({ ...item, url: frameUrl, kind: "page", generic: true, frameUrl, frameId: frame.frameId });
        }
        continue;
      }
      push({ ...item, frameUrl, frameId: frame.frameId });
    }
  }
  // The address of each frame, the referrer of what it loaded.
  const frameUrls = new Map(frames.map((frame) => [frame.frameId, frame.result?.url]));
  for (const candidate of sniffed) {
    const described = describe(candidate);
    const audio = /^audio\//.test(candidate.contentType) || /\.(mp3|m4a|aac|ogg|oga|opus|wav|flac)$/i.test(parseUrl(candidate.url)?.pathname ?? "");
    push({
      type: audio ? "audio" : "video",
      kind: candidate.kind,
      url: candidate.url,
      thumb: "",
      name: described.label,
      detail: described.detail,
      size: candidate.size ?? null,
      source: "network",
      frameUrl: candidate.frameUrl || frameUrls.get(candidate.frameId),
      frameId: candidate.frameId,
    });
  }
  return { ok: true, error, page: { url: tab.url, title: tab.title, folder: siteFolder(tab.url) }, items };
}

/** Is this address there, and what is it? HEAD, or one byte when HEAD is refused. */
async function probeUrl(url) {
  if (!/^https?:/i.test(url ?? "") || isBlocked(url)) return { ok: false };
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 10000);
  try {
    let res = await fetch(url, { method: "HEAD", credentials: "include", signal: controller.signal, redirect: "follow" });
    if (!res.ok) {
      res = await fetch(url, { headers: { Range: "bytes=0-0" }, credentials: "include", signal: controller.signal, redirect: "follow" });
      res.body?.cancel().catch(() => {});
    }
    const type = (res.headers.get("content-type") || "").split(";")[0].trim().toLowerCase();
    const range = res.headers.get("content-range");
    const total = range ? Number(range.split("/")[1]) : Number(res.headers.get("content-length"));
    return { ok: res.ok, status: res.status, type, size: Number.isFinite(total) && total > 0 ? total : null, url: res.url || url };
  } catch (err) {
    return { ok: false, error: String(err?.message || err) };
  } finally {
    clearTimeout(timer);
  }
}

// Each batch is one message; the native host takes up to 8 MiB.
const GALLERY_CHUNK = 150;

/** The gallery's selection, to the queue: each in its category's folder, inside the site's. */
async function galleryDownload(msg) {
  const tab = await api.tabs.get(msg.tabId).catch(() => null);
  const pageUrl = tab?.url || msg.pageUrl;
  const folder = siteFolder(pageUrl);
  const storeId = await cookieStoreOf(tab);
  const cookies = new Map();
  const cookiesOf = async (url) => {
    const origin = parseUrl(url)?.origin ?? "";
    if (!cookies.has(origin)) cookies.set(origin, await cookiesFor(url, storeId));
    return cookies.get(origin);
  };
  const downloads = [];
  for (const item of msg.items ?? []) {
    if (!/^(https?|ftp):/i.test(item.url ?? "") || isBlocked(item.url)) continue;
    const kind = item.kind === "page" || item.kind === "stream" ? item.kind : "direct";
    downloads.push({
      url: item.url,
      kind,
      referrer: item.frameUrl || pageUrl,
      pageUrl,
      title: kind === "direct" ? undefined : item.name || undefined,
      userAgent: navigator.userAgent,
      cookies: await cookiesOf(item.url),
      category: item.type === "image" ? "image" : item.type === "audio" ? "music" : "video",
      subfolder: folder,
      start: true,
    });
  }
  if (!downloads.length) throw new Error(message("galleryNothingSelected", "Nenhuma mídia selecionada."));
  const ids = [];
  for (let i = 0; i < downloads.length; i += GALLERY_CHUNK) {
    let reply;
    try {
      reply = await api.runtime.sendNativeMessage(HOST, { type: "media-batch", downloads: downloads.slice(i, i + GALLERY_CHUNK) });
    } catch (err) {
      const text = String(err?.message || err);
      if (/not found|no such native application|forbidden/i.test(text)) {
        throw new Error(message("hostMissing", "App Fisga não encontrado. Instale-o e rode: fisga install-hosts"));
      }
      throw new Error(text);
    }
    if (reply?.type !== "added-batch") throw new Error(reply?.message || "resposta inválida do app");
    ids.push(...reply.ids);
  }
  return { ok: true, ids, folder };
}

/** Pages of the extension itself (popup, gallery), not content scripts. */
function fromExtensionPage(sender) {
  return Boolean(sender?.url?.startsWith(api.runtime.getURL("")));
}

async function handle(msg, sender) {
  if (/^gallery-/.test(msg?.type ?? "") && !fromExtensionPage(sender)) return { ok: false, error: "mensagem desconhecida" };
  switch (msg?.type) {
    case "gallery-open": {
      const tab = await api.tabs.get(msg.tabId);
      await openGallery(tab);
      return { ok: true };
    }
    case "gallery-scan":
      return galleryScan(msg.tabId);
    case "gallery-probe":
      return probeUrl(msg.url);
    case "gallery-download":
      return galleryDownload(msg);
    case "media-candidates":
      return { ok: true, items: await candidatesFor(sender.tab?.id, frameUrlOf(msg, sender), sender.tab?.url, msg.linkUrl) };
    case "download-media":
      return downloadMedia(msg, sender);
    case "popup-candidates": {
      const tab = await api.tabs.get(msg.tabId);
      return { ok: true, items: await candidatesFor(tab.id, tab.url, tab.url), blocked: isBlocked(tab.url) };
    }
    case "popup-download": {
      const tab = await api.tabs.get(msg.tabId);
      await restore();
      // What a frame loaded goes with that frame's address.
      const sniffed = (mediaByTab.get(tab.id) ?? []).find((c) => c.url === msg.item?.url);
      const referrer = (sniffed && (await frameUrlOfCandidate(tab, sniffed))) || tab.url;
      const { id, pending } = await sendMedia(msg.item, { referrer, pageUrl: tab.url, title: tab.title, tab });
      return { ok: true, id, pending };
    }
    case "force-link":
      return forceLink(msg, sender);
    case "ping":
      syncWithApp();
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
function extensionOf(name) {
  const base = (name || "").split(/[\\/]/).pop().split(/[?#]/)[0];
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

/** On one of the sites the user keeps out of Fisga (or a subdomain of one). */
function isExcluded(url, prefs) {
  const u = parseUrl(url || "");
  return Boolean(u && hostMatches(u.hostname.toLowerCase(), prefs.excludedSites ?? []));
}

/** The extension of the file's name decides (the user's file types); its type only when the name has none. */
function shouldIntercept(item, prefs) {
  const allowed = new Set((prefs.fileTypes ?? []).map((type) => String(type).toLowerCase()));
  const ext = extensionOf(item.filename);
  if (ext) return allowed.has(ext);
  const mime = (item.mime || "").toLowerCase();
  return /^(video|audio)\//.test(mime) || /^application\/(zip|x-7z|x-rar|vnd\.rar|x-tar|gzip|x-xz|zstd|x-iso9660|x-msdownload|x-debian|x-rpm|x-apple-diskimage|octet-stream)/.test(mime);
}

// Downloads already judged (taken over or left to the browser), by id.
const judged = new Set();

async function takeOver(item) {
  if (judged.has(item.id) || !/^https?:/i.test(item.finalUrl || item.url)) return;
  judged.add(item.id);
  // Private windows' downloads (and their cookies) stay with the browser.
  if (item.byExtensionId === api.runtime.id || item.incognito) return;
  // Without the app (not installed) the browser keeps its downloads; after a
  // failed try the app is asked again now.
  await syncWithApp(!appReachable);
  if (!appReachable) return;
  const prefs = await settings();
  if (!prefs.intercept || item.state !== "in_progress") return;
  // Chrome names the file (Content-Disposition) after creating the item: the
  // name may have come while waiting, else it gets its own look in onChanged.
  if (!item.filename) {
    const [named] = await api.downloads.search({ id: item.id }).catch(() => []);
    if (!named?.filename) {
      judged.delete(item.id);
      return;
    }
    item = named;
  }
  const url = item.finalUrl || item.url;
  // The site of the file, or of the page it came from.
  if (isExcluded(url, prefs) || isExcluded(item.referrer, prefs) || !shouldIntercept(item, prefs)) return;

  // The browser holds the download while the app is asked, and goes on with
  // it if the app refuses. One that already finished stays with the browser:
  // pause() refuses it (Chrome's cancel() would not), and one still finishing
  // does not pause.
  try {
    await api.downloads.pause(item.id);
  } catch {
    return;
  }
  const [held] = await api.downloads.search({ id: item.id }).catch(() => []);
  if (!held || held.state === "complete" || (held.state === "in_progress" && !held.paused)) return;
  try {
    await sendToApp({
      url,
      kind: "direct",
      referrer: item.referrer || undefined,
      fileName: extensionOf(item.filename) ? item.filename.split(/[\\/]/).pop() : undefined,
      userAgent: navigator.userAgent,
      // The download's cookie store (a Firefox container).
      cookies: await cookiesFor(url, item.cookieStoreId),
    });
  } catch (err) {
    console.warn("Fisga: the download stays with the browser:", err);
    api.downloads.resume(item.id).catch(() => {});
    return;
  }
  await api.downloads.cancel(item.id).catch(() => {});
  // Finished before the cancel after all: the browser's copy stays listed.
  const [after] = await api.downloads.search({ id: item.id }).catch(() => []);
  if (after?.state !== "complete") api.downloads.erase({ id: item.id }).catch(() => {});
}

api.downloads.onCreated.addListener((item) => takeOver(item));
api.downloads.onChanged.addListener(async (delta) => {
  if (!delta.filename?.current) return;
  const [item] = await api.downloads.search({ id: delta.id }).catch(() => []);
  if (item) takeOver(item);
});
api.downloads.onErased.addListener((id) => judged.delete(id));

// -------------------------------------------------------------- force key
/** A link clicked with the force key held: to Fisga, whatever it is. */
async function forceLink(msg, sender) {
  const url = msg.url;
  if (!/^https?:/i.test(url ?? "")) throw new Error("link sem endereço http(s)");
  const kind = linkKind(url);
  const { id, pending } = await sendToApp({
    url,
    kind,
    referrer: frameUrlOf(msg, sender),
    pageUrl: sender.tab?.url || msg.pageUrl,
    title: kind === "page" ? msg.linkTitle : undefined,
    userAgent: navigator.userAgent,
    cookies: await cookiesFor(url, await cookieStoreOf(sender.tab)),
  });
  return { ok: true, id, pending };
}

// ------------------------------------------------------------ context menus
api.runtime.onInstalled.addListener(async () => {
  syncWithApp();
  await Promise.resolve(api.contextMenus.removeAll()).catch(() => {});
  api.contextMenus.create({ id: "fisga-link", title: message("menuLink", "Baixar link com o Fisga"), contexts: ["link"] });
  api.contextMenus.create({ id: "fisga-media", title: message("menuMedia", "Baixar esta mídia com o Fisga"), contexts: ["video", "audio"] });
  api.contextMenus.create({ id: "fisga-all-links", title: message("menuAllLinks", "Baixar todos os links com o Fisga"), contexts: ["page"] });
  api.contextMenus.create({
    id: "fisga-selected-links",
    title: message("menuSelectedLinks", "Baixar os links selecionados com o Fisga"),
    contexts: ["selection"],
  });
  api.contextMenus.create({ id: "fisga-gallery", title: message("menuGallery", "Baixar fotos e vídeos da página com o Fisga…"), contexts: ["page", "image", "video"] });
});

/** The links of the clicked frame (or the selected ones) go to the app, which lists them to pick from. */
async function sendLinks(info, tab, onlySelected) {
  const reply = await api.tabs.sendMessage(tab.id, { type: "collect-links", selection: onlySelected }, { frameId: info.frameId ?? 0 });
  const links = (reply?.links ?? []).filter((url) => !isBlocked(url));
  if (!links.length) throw new Error("nenhum link na página");
  const answer = await api.runtime.sendNativeMessage(HOST, { type: "links", pageUrl: info.frameUrl || info.pageUrl || tab.url, links });
  if (answer?.type !== "shown") throw new Error(answer?.message || "resposta inválida do app");
}

api.contextMenus.onClicked.addListener(async (info, tab) => {
  try {
    if (info.menuItemId === "fisga-link" && info.linkUrl) {
      // Like the force key: a video's page goes to yt-dlp, not saved as HTML.
      const kind = linkKind(info.linkUrl);
      await sendToApp({
        url: info.linkUrl,
        kind,
        referrer: info.frameUrl || info.pageUrl,
        pageUrl: tab?.url,
        title: kind === "page" ? info.linkText || undefined : undefined,
        userAgent: navigator.userAgent,
        cookies: await cookiesFor(info.linkUrl, await cookieStoreOf(tab)),
      });
    } else if (info.menuItemId === "fisga-gallery") {
      await openGallery(tab);
    } else if (info.menuItemId === "fisga-all-links" || info.menuItemId === "fisga-selected-links") {
      await sendLinks(info, tab, info.menuItemId === "fisga-selected-links");
    } else if (info.menuItemId === "fisga-media") {
      const sender = { tab, url: info.frameUrl || info.pageUrl, frameId: info.frameId ?? 0 };
      await downloadMedia({ src: info.srcUrl, pageUrl: info.frameUrl || info.pageUrl, title: tab?.title }, sender);
    }
  } catch (err) {
    console.warn("Fisga:", err);
  }
});
