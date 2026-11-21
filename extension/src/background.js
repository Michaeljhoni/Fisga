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
  throw new Error(reply?.message || message("badReply", "Invalid answer from the Fisga app."));
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
    return { ok: reply?.type === "pong", version: reply?.version, daemon: reply?.daemon, galleryDl: Boolean(reply?.galleryDl) };
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

// Posts, albums and profiles of social and art sites, whose photos a page
// does not hold whole (a carousel shows one at a time, a feed thumbnails,
// a slideshow is a video): the app lists their files with gallery-dl, when
// it has it, and each one goes to the queue. Words of their paths that are
// the sites' own pages, not users (or, for Pinterest's boards, a profile's tabs).
const NOT_USERS = {
  instagram: "about accounts ads api challenge creators developer direct download emails explore graphql legal lite locations nametag oauth p privacy reel reels session settings stories terms topics tv web your_activity",
  x: "about account bookmarks communities compose download explore hashtag home i intent jobs lists login logout messages notifications premium privacy rules search settings share signup tos verified-choose",
  deviantart: "about account checkout core-membership daily-deviations developers deviations forum groups join messages notifications popular search settings shop status tag team topic users watch",
  pinterest: "_ about business categories explore ideas login logout news_hub pin search settings shopping today",
  // A user's pages that are neither a board nor the profile's pins.
  pinterestBoard: "_community _profile _shop _tools followers following more_ideas",
};
const notUser = (site, word) => NOT_USERS[site].split(" ").includes(word.toLowerCase());
// The sites' own hosts (help.instagram.com, about.x.com... are not profiles).
const GALLERY_HOSTS = {
  instagram: /^(?:www\.)?instagram\.com$/,
  tiktok: /^(?:www\.|m\.)?tiktok\.com$/,
  x: /^(?:www\.|mobile\.)?(?:x|twitter)\.com$/,
  deviantart: /^(?:www\.)?deviantart\.com$/,
  flickr: /^(?:www\.|m\.)?flickr\.com$/,
  reddit: /^(?:www\.|old\.|new\.|np\.|m\.)?reddit\.com$/,
  // pinterest.com and its countries' (br.pinterest.com, pinterest.co.uk, pinterest.com.mx...).
  pinterest: /^(?:www\.|[a-z]{2}\.)?pinterest\.(?:com|[a-z]{2})(?:\.[a-z]{2})?$/,
};

/**
 * What gallery-dl can take at `url`: { url, whole, name }, `whole` being
 * "post" (every photo of one post), "album" (a board, an album) or "profile"
 * (everything a user posted), and `url` the address gallery-dl knows it by.
 * Null for any other page.
 */
function galleryTarget(url) {
  const u = parseUrl(url ?? "");
  if (!u || !/^https?:$/.test(u.protocol)) return null;
  const host = u.hostname.toLowerCase();
  const p = u.pathname.split("/").filter(Boolean);
  const name = /^[\w.-]+$/;
  const at = (n) => p[n] ?? "";
  if (GALLERY_HOSTS.instagram.test(host)) {
    const post = at(0) === "p" ? at(1) : at(1) === "p" ? at(2) : "";
    if (/^[\w-]+$/.test(post)) return { url: `https://www.instagram.com/p/${post}/`, whole: "post", name: "" };
    const user = at(0);
    if (/^[\w.]+$/.test(user) && !notUser("instagram", user) && (p.length === 1 || (p.length === 2 && ["tagged", "reels"].includes(at(1))))) {
      return { url: `https://www.instagram.com/${user}/`, whole: "profile", name: `@${user}` };
    }
    return null;
  }
  if (GALLERY_HOSTS.tiktok.test(host)) {
    const user = /^@([\w.-]+)$/.exec(at(0))?.[1];
    if (!user) return null;
    if (at(1) === "photo" && /^\d+$/.test(at(2))) return { url: `https://www.tiktok.com/@${user}/photo/${at(2)}`, whole: "post", name: "" };
    return p.length === 1 ? { url: `https://www.tiktok.com/@${user}`, whole: "profile", name: `@${user}` } : null;
  }
  if (GALLERY_HOSTS.x.test(host)) {
    const user = at(0);
    if (!/^\w{1,15}$/.test(user) || notUser("x", user)) return null;
    if (at(1) === "status" && /^\d+$/.test(at(2))) return { url: `https://x.com/${user}/status/${at(2)}`, whole: "post", name: "" };
    return p.length === 1 || (p.length === 2 && at(1) === "media") ? { url: `https://x.com/${user}/media`, whole: "profile", name: `@${user}` } : null;
  }
  if (GALLERY_HOSTS.deviantart.test(host)) {
    const user = at(0);
    if (!/^[\w-]+$/.test(user) || notUser("deviantart", user)) return null;
    if (at(1) === "art" && name.test(at(2))) return { url: `https://www.deviantart.com/${user}/art/${at(2)}`, whole: "post", name: "" };
    if (at(1) === "gallery" && /^\d+$/.test(at(2))) {
      return { url: `https://www.deviantart.com/${user}/gallery/${at(2)}${name.test(at(3)) ? `/${at(3)}` : ""}`, whole: "album", name: user };
    }
    return p.length === 1 || (at(1) === "gallery" && p.length <= 3) ? { url: `https://www.deviantart.com/${user}/gallery`, whole: "profile", name: user } : null;
  }
  if (GALLERY_HOSTS.flickr.test(host)) {
    const user = at(1);
    if (at(0) !== "photos" || !/^[\w@-]+$/.test(user) || user === "tags") return null;
    if (/^\d+$/.test(at(2))) return { url: `https://www.flickr.com/photos/${user}/${at(2)}/`, whole: "post", name: "" };
    if (["albums", "sets"].includes(at(2)) && /^\d+$/.test(at(3))) return { url: `https://www.flickr.com/photos/${user}/albums/${at(3)}`, whole: "album", name: user };
    return p.length === 2 || (p.length === 3 && /^page\d+$/.test(at(2))) ? { url: `https://www.flickr.com/photos/${user}/`, whole: "profile", name: user } : null;
  }
  if (GALLERY_HOSTS.reddit.test(host)) {
    if (at(0) === "r" && /^\w+$/.test(at(1)) && at(2) === "comments" && /^\w+$/.test(at(3))) {
      return { url: `https://www.reddit.com/r/${at(1)}/comments/${at(3)}/`, whole: "post", name: "" };
    }
    if (["gallery", "comments"].includes(at(0)) && /^\w+$/.test(at(1))) return { url: `https://www.reddit.com/${at(0)}/${at(1)}/`, whole: "post", name: "" };
    if (["u", "user"].includes(at(0)) && /^[\w-]+$/.test(at(1)) && (p.length === 2 || (p.length === 3 && at(2) === "submitted"))) {
      return { url: `https://www.reddit.com/user/${at(1)}/submitted/`, whole: "profile", name: `u/${at(1)}` };
    }
    return null;
  }
  if (GALLERY_HOSTS.pinterest.test(host)) {
    // A pin's id has digits: not /pin/create/button/ (a "Pin it" link) nor /pin/find/.
    if (at(0) === "pin") return /^(?=[\w-]*\d)[\w-]+$/.test(at(1)) ? { url: `https://${host}/pin/${at(1)}/`, whole: "post", name: "" } : null;
    const user = at(0);
    if (!name.test(user) || notUser("pinterest", user)) return null;
    // The profile's own tabs are the profile.
    const tab = ["_saved", "_created", "pins"].includes(at(1));
    if (p.length === 2 && name.test(at(1)) && !tab && !notUser("pinterestBoard", at(1))) {
      return { url: `https://${host}/${user}/${at(1)}/`, whole: "album", name: `${user}/${at(1)}` };
    }
    return p.length === 1 || (p.length === 2 && tab) ? { url: `https://${host}/${user}/`, whole: "profile", name: user } : null;
  }
  return null;
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
  if (!frames.length && !error) error = message("noFrames", "No frame of the page answered.");

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
        } else if (hasVideo(frame) || isExtractorPage(frameUrl)) {
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
  // Posts gallery-dl can take: a photo that links to one (a feed's, a
  // grid's) or, linking nowhere, sits in the post the page is. The page
  // itself, a post, an album or a profile, is one more item when the app has
  // gallery-dl: its key is its own (the page may be listed already, as a
  // post's video for yt-dlp), its address the `target`.
  const here = galleryTarget(tab.url);
  for (const item of items) {
    // A post's video, for yt-dlp: the whole post, when sent too, holds it.
    const post = item.kind === "page" ? galleryTarget(item.url) : item.type !== "image" ? null : item.postLink ? galleryTarget(item.postLink) : here;
    if (post?.whole === "post") item.post = post.url;
    delete item.postLink;
  }
  const app = here || items.some((item) => item.post) ? await ping() : {};
  const galleryDl = Boolean(app.ok && app.galleryDl);
  if (here && galleryDl) {
    const cover = here.whole === "post" ? items.find((item) => item.type === "image" && item.source === "meta")?.url : "";
    const name = here.name || tab.title || here.url;
    items.unshift({ type: "image", kind: "gallery", url: `${here.url}#gallery`, target: here.url, whole: here.whole, name, thumb: cover || "", source: "gallery-dl" });
  }
  // A Firefox container or a private window: the address checks cannot use
  // its cookies, so a refused check proves nothing there.
  const ownStore = Boolean(tab.incognito || (tab.cookieStoreId && !/^(firefox-default|0)$/.test(tab.cookieStoreId)));
  const galleryHint = Boolean((here || items.some((item) => item.post)) && !galleryDl);
  return { ok: true, error, page: { url: tab.url, title: tab.title, folder: siteFolder(tab.url), ownStore, galleryDl, galleryHint }, items };
}

/** Width and height from the first bytes of a JPEG, PNG, GIF, WebP, AVIF/HEIF or BMP. */
function imageSize(bytes) {
  const b = bytes;
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const text = (at, n) => String.fromCharCode(...b.subarray(at, at + n));
  if (b.length >= 24 && b[0] === 0x89 && text(1, 3) === "PNG") return { width: dv.getUint32(16), height: dv.getUint32(20) };
  if (b.length >= 10 && text(0, 3) === "GIF") return { width: dv.getUint16(6, true), height: dv.getUint16(8, true) };
  if (b.length >= 30 && text(0, 4) === "RIFF" && text(8, 4) === "WEBP") {
    const kind = text(12, 4);
    if (kind === "VP8 ") return { width: dv.getUint16(26, true) & 0x3fff, height: dv.getUint16(28, true) & 0x3fff };
    if (kind === "VP8L") {
      const bits = dv.getUint32(21, true);
      return { width: (bits & 0x3fff) + 1, height: ((bits >> 14) & 0x3fff) + 1 };
    }
    if (kind === "VP8X") return { width: 1 + (b[24] | (b[25] << 8) | (b[26] << 16)), height: 1 + (b[27] | (b[28] << 8) | (b[29] << 16)) };
  }
  if (b.length >= 4 && b[0] === 0xff && b[1] === 0xd8) {
    // JPEG: skip the segments (EXIF, ICC...) up to the frame header.
    let i = 2;
    while (i + 9 < b.length) {
      if (b[i] !== 0xff) return null;
      const marker = b[i + 1];
      if (marker === 0xff) {
        i++;
        continue;
      }
      if (marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc) {
        return { width: dv.getUint16(i + 7), height: dv.getUint16(i + 5) };
      }
      i += 2 + dv.getUint16(i + 2);
    }
    return null;
  }
  if (b.length >= 26 && text(0, 2) === "BM") return { width: dv.getInt32(18, true), height: Math.abs(dv.getInt32(22, true)) };
  // AVIF/HEIF: the image's 'ispe' box.
  for (let i = 4; i + 16 <= b.length; i++) {
    if (b[i] === 0x69 && text(i, 4) === "ispe") return { width: dv.getUint32(i + 8), height: dv.getUint32(i + 12) };
  }
  return null;
}

/** The site an address belongs to: its last two labels (three under a short second level such as com.br, co.uk). */
function siteOf(hostname) {
  const labels = (hostname || "").toLowerCase().split(".").filter(Boolean);
  if (labels.length <= 2 || /^\d+$/.test(labels[labels.length - 1])) return labels.join(".");
  const second = labels[labels.length - 2];
  const short = second.length <= 3 && labels[labels.length - 1].length === 2;
  return labels.slice(short ? -3 : -2).join(".");
}
/** Loopback, private networks and names without a dot (the local network). */
function isPrivateHost(hostname) {
  const host = (hostname || "").toLowerCase().replace(/^\[|\]$/g, "");
  return (
    !host.includes(".") ||
    /\.(local|localhost|internal|lan|home\.arpa)$/.test(host) ||
    /^(127|10)\.|^192\.168\.|^169\.254\.|^172\.(1[6-9]|2\d|3[01])\.|^0\./.test(host) ||
    /^(::1?|f[cd][0-9a-f]{2}:.*|fe80:.*)$/.test(host)
  );
}
/**
 * How the gallery may ask for an address the page named: with the user's
 * cookies only on the page's own site, and never inside the local network
 * when the page is on the internet (a page must not make the extension
 * call the router or localhost for it).
 */
function probePolicy(url, pageUrl) {
  const u = parseUrl(url);
  const page = parseUrl(pageUrl || "");
  if (!u || !/^https?:$/.test(u.protocol) || isBlocked(url)) return null;
  if (isPrivateHost(u.hostname) && !(page && isPrivateHost(page.hostname))) return null;
  return page && siteOf(u.hostname) === siteOf(page.hostname) ? "include" : "omit";
}

/**
 * Is this address there, and what is it? The first 64 KiB (their header
 * gives a photo's size), never HEAD: some CDNs refuse it (X answers 503,
 * Google 404) where GET works. `light`: only whether it is there and what
 * it is (no size read), for the checks right before sending.
 */
async function probeUrl(url, pageUrl, light = false) {
  const credentials = probePolicy(url, pageUrl);
  if (!credentials) return { ok: false };
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 12000);
  try {
    const wanted = light ? "bytes=0-0" : "bytes=0-65535";
    const res = await fetch(url, { headers: { Range: wanted }, credentials, signal: controller.signal, redirect: "follow" });
    const type = (res.headers.get("content-type") || "").split(";")[0].trim().toLowerCase();
    const range = res.headers.get("content-range");
    const total = range ? Number(range.split("/")[1]) : Number(res.headers.get("content-length"));
    let dimensions = null;
    if (!light && res.ok && /^image\//.test(type) && res.body) {
      const reader = res.body.getReader();
      const chunks = [];
      let length = 0;
      while (length < 65536) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        length += value.length;
      }
      reader.cancel().catch(() => {});
      const bytes = new Uint8Array(length);
      let at = 0;
      for (const chunk of chunks) {
        bytes.set(chunk, at);
        at += chunk.length;
      }
      try {
        dimensions = imageSize(bytes);
      } catch {}
    } else {
      res.body?.cancel().catch(() => {});
    }
    return {
      ok: res.ok,
      status: res.status,
      type,
      size: Number.isFinite(total) && total > 0 ? total : null,
      url: res.url || url,
      width: dimensions?.width || 0,
      height: dimensions?.height || 0,
    };
  } catch (err) {
    return { ok: false, error: String(err?.message || err) };
  } finally {
    clearTimeout(timer);
  }
}

/**
 * An HLS or DASH manifest: the best resolution it offers, whether it is
 * protected (DRM: cannot be downloaded), and the quality playlists a master
 * lists (shown once, as the master).
 */
/** At most `limit` bytes of an answer, as text: a playlist that never ends must not fill the memory. */
async function readText(res, limit) {
  if (!res.body) return "";
  const reader = res.body.getReader();
  const chunks = [];
  let length = 0;
  while (length < limit) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    length += value.length;
  }
  reader.cancel().catch(() => {});
  const bytes = new Uint8Array(Math.min(length, limit));
  let at = 0;
  for (const chunk of chunks) {
    const part = chunk.subarray(0, bytes.length - at);
    bytes.set(part, at);
    at += part.length;
    if (at >= bytes.length) break;
  }
  return new TextDecoder().decode(bytes);
}

const HLS_DRM = /#EXT-X(?:-SESSION)?-KEY:[^\n]*(METHOD=SAMPLE-AES|KEYFORMAT="(com\.apple\.streamingkeydelivery|urn:uuid))/i;

async function probeManifest(url, pageUrl) {
  const credentials = probePolicy(url, pageUrl);
  if (!credentials) return { ok: false };
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 12000);
  try {
    const res = await fetch(url, { credentials, signal: controller.signal });
    const text = await readText(res, 512 * 1024);
    if (/^#EXTM3U/m.test(text)) {
      const lines = text.split(/\r?\n/);
      let height = 0;
      const variants = [];
      lines.forEach((line, i) => {
        if (!/^#EXT-X-STREAM-INF/.test(line)) return;
        height = Math.max(height, Number(/RESOLUTION=\d+x(\d+)/.exec(line)?.[1] ?? 0));
        const next = lines.slice(i + 1).find((l) => l.trim() && !l.startsWith("#"));
        if (next) variants.push(new URL(next.trim(), res.url || url).href);
      });
      let drm = HLS_DRM.test(text);
      // The keys usually sit in the quality playlists: look at the first one.
      if (!drm && variants.length && probePolicy(variants[0], pageUrl)) {
        const first = await fetch(variants[0], { credentials: probePolicy(variants[0], pageUrl), signal: controller.signal }).catch(() => null);
        if (first?.ok) drm = HLS_DRM.test(await readText(first, 128 * 1024));
      }
      return { ok: true, master: variants.length > 0, height, variants, drm };
    }
    if (/<MPD[\s>]/.test(text)) {
      let height = 0;
      for (const m of text.matchAll(/<Representation\b[^>]*\bheight="(\d+)"/g)) height = Math.max(height, Number(m[1]));
      // Widevine and PlayReady.
      const drm = /urn:uuid:(edef8ba9-79d6-4ace-a3c8-27dcd51d21ed|9a04f079-9840-4286-ab92-e65be0885f95)/i.test(text);
      return { ok: true, master: true, height, variants: [], drm };
    }
    return { ok: res.ok, master: false, height: 0, variants: [], drm: false };
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
  // The page the gallery listed, even if the tab moved on since.
  const pageUrl = msg.pageUrl || tab?.url;
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
    const kind = ["page", "stream", "gallery"].includes(item.kind) ? item.kind : "direct";
    // gallery-dl gets only the posts and profiles it is known to take, as it knows them.
    if (kind === "gallery" && galleryTarget(item.url)?.url !== item.url) continue;
    downloads.push({
      url: item.url,
      kind,
      referrer: item.frameUrl || pageUrl,
      pageUrl,
      title: kind === "direct" ? undefined : item.name || undefined,
      userAgent: navigator.userAgent,
      cookies: await cookiesOf(item.url),
      category: item.type === "image" ? "image" : item.type === "audio" ? "music" : "video",
      // A post goes to its own site's folder, wherever it was linked from.
      subfolder: kind === "gallery" ? siteFolder(item.url) : folder,
      start: true,
    });
  }
  if (!downloads.length) throw new Error(message("galleryNothingSelected", "Nenhuma mídia selecionada."));
  const ids = [];
  // What the app took: after a failed part, the gallery marks only these as queued.
  const sent = [];
  for (let i = 0; i < downloads.length; i += GALLERY_CHUNK) {
    const chunk = downloads.slice(i, i + GALLERY_CHUNK);
    let reply;
    try {
      reply = await api.runtime.sendNativeMessage(HOST, { type: "media-batch", downloads: chunk });
    } catch (err) {
      const text = String(err?.message || err);
      const error = /not found|no such native application|forbidden/i.test(text)
        ? message("hostMissing", "App Fisga não encontrado. Instale-o e rode: fisga install-hosts")
        : text;
      return { ok: false, error, ids, sent, folder };
    }
    if (reply?.type !== "added-batch") {
      return { ok: false, error: reply?.message || message("badReply", "Invalid answer from the Fisga app."), ids, sent, folder };
    }
    ids.push(...reply.ids);
    sent.push(...chunk.map((download) => download.url));
  }
  return { ok: true, ids, sent, folder };
}

/** Pages of the extension itself (popup, gallery), not content scripts. */
function fromExtensionPage(sender) {
  return Boolean(sender?.url?.startsWith(api.runtime.getURL("")));
}

async function handle(msg, sender) {
  if (/^gallery-/.test(msg?.type ?? "") && !fromExtensionPage(sender)) return { ok: false, error: message("unknownMessage", "Unknown message.") };
  switch (msg?.type) {
    case "gallery-open": {
      const tab = await api.tabs.get(msg.tabId);
      await openGallery(tab);
      return { ok: true };
    }
    case "gallery-scan":
      return galleryScan(msg.tabId);
    case "gallery-probe":
      return probeUrl(msg.url, msg.pageUrl, Boolean(msg.light));
    case "gallery-manifest":
      return probeManifest(msg.url, msg.pageUrl);
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
      return { ok: false, error: message("unknownMessage", "Unknown message.") };
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
  if (!/^https?:/i.test(url ?? "")) throw new Error(message("notHttpLink", "This link has no http(s) address."));
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
  if (!links.length) throw new Error(message("noLinks", "No links on the page."));
  const answer = await api.runtime.sendNativeMessage(HOST, { type: "links", pageUrl: info.frameUrl || info.pageUrl || tab.url, links });
  if (answer?.type !== "shown") throw new Error(answer?.message || message("badReply", "Invalid answer from the Fisga app."));
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
