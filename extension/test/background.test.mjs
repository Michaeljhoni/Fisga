// The background script's rules (src/background.js), run in a VM with a fake
// extension API.   node --test extension/test/
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("../src/background.js", import.meta.url), "utf8");
const plain = (x) => JSON.parse(JSON.stringify(x ?? null));
const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

function event() {
  const listeners = [];
  return {
    listeners,
    addListener: (fn) => listeners.push(fn),
    fire: (...args) => Promise.all(listeners.map((fn) => fn(...args))),
  };
}

/** A storage area: get(string | string[] | defaults object), set(object). */
function area(data) {
  return {
    data,
    async get(keys) {
      const pick = (key) => (key in data ? structuredClone(data[key]) : undefined);
      if (typeof keys === "string") return keys in data ? { [keys]: pick(keys) } : {};
      if (Array.isArray(keys)) return Object.fromEntries(keys.filter((k) => k in data).map((k) => [k, pick(k)]));
      if (keys && typeof keys === "object") return Object.fromEntries(Object.entries(keys).map(([k, d]) => [k, k in data ? pick(k) : d]));
      return structuredClone(data);
    },
    async set(values) {
      Object.assign(data, structuredClone(values));
    },
  };
}

/**
 * Loads background.js with a fake `chrome`.
 *   session: storage.session's contents (what a sleeping worker left)
 *   native(msg): the app's answer (throw: the app is not installed)
 *   tabs: { id: tab } for tabs.get
 *   downloads: { id: item } for the downloads API (Chrome's behaviour)
 */
function load({ session = {}, local = {}, native, tabs = {}, downloads = {}, stores, frames = {} } = {}) {
  const sent = [];
  const calls = [];
  const items = new Map(Object.entries(downloads).map(([id, item]) => [Number(id), { id: Number(id), paused: false, ...item }]));
  const answer =
    native ??
    ((msg) =>
      msg.type === "settings"
        ? { type: "settings", fileTypes: ["zip", "mp4", "safetensors"], forceKey: "alt" }
        : { type: "added", id: sent.length, pending: false });
  const chrome = {
    runtime: {
      id: "fisga-test",
      onStartup: event(),
      onMessage: event(),
      onInstalled: event(),
      getURL: (path) => "chrome-extension://fisga-test/" + path,
      async sendNativeMessage(host, msg) {
        sent.push(structuredClone(msg));
        return answer(msg);
      },
    },
    tabs: {
      onActivated: event(),
      onUpdated: event(),
      onRemoved: event(),
      get: async (id) => tabs[id] ?? { id, url: "https://example.com/" },
      sendMessage: async () => null,
      create: async () => {},
    },
    storage: { local: area(local), session: area(session), onChanged: event() },
    webRequest: { onHeadersReceived: event() },
    downloads: {
      onCreated: event(),
      onChanged: event(),
      onErased: event(),
      async search({ id }) {
        return items.has(id) ? [structuredClone(items.get(id))] : [];
      },
      async pause(id) {
        calls.push(["pause", id]);
        const item = items.get(id);
        if (!item || item.state !== "in_progress") throw new Error("Download must be in progress");
        item.paused = true;
      },
      async resume(id) {
        calls.push(["resume", id]);
        items.get(id).paused = false;
      },
      // Like Chrome: also resolves for a download that already finished.
      async cancel(id) {
        calls.push(["cancel", id]);
        const item = items.get(id);
        if (item?.state === "in_progress") item.state = "interrupted";
      },
      async erase({ id }) {
        calls.push(["erase", id]);
        items.delete(id);
        return [id];
      },
      async download(options) {
        calls.push(["download", options.url]);
        return 999;
      },
    },
    contextMenus: { create() {}, removeAll: async () => {}, onClicked: event() },
    action: { setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {} },
    cookies: {
      async getAll(details) {
        calls.push(["cookies", details]);
        return [];
      },
      getAllCookieStores: async () => stores ?? [{ id: "0", tabIds: [] }],
    },
    i18n: { getMessage: () => "" },
    scripting: {
      async executeScript({ target }) {
        return (target.frameIds ?? [0]).map((frameId) => ({ frameId, result: frames[frameId] ?? null }));
      },
    },
  };
  const context = vm.createContext({
    chrome,
    console: { log: console.log, error: console.error, warn() {} },
    URL,
    setTimeout,
    clearTimeout,
    AbortController,
    navigator: { userAgent: "test" },
    fetch: async () => {
      throw new Error("offline");
    },
  });
  vm.runInContext(SOURCE, context, { filename: "background.js" });
  const run = (code) => vm.runInContext(code, context);
  return { chrome, sent, calls, items, session: chrome.storage.session.data, run };
}

/** A response the webRequest listener sees, then a moment for it to be recorded. */
async function sniff(bg, { tabId = 1, frameId = 0, url, type = "", length, documentUrl }) {
  const responseHeaders = [{ name: "Content-Type", value: type }];
  if (length) responseHeaders.push({ name: "Content-Length", value: String(length) });
  for (const fn of bg.chrome.webRequest.onHeadersReceived.listeners) {
    fn({ tabId, frameId, url, statusCode: 200, responseHeaders, documentUrl });
  }
  await tick();
}
const HLS = "application/vnd.apple.mpegurl";
const MB = 1024 * 1024;

// ------------------------------------------------------------ tab events
test("a navigation that wakes the worker keeps the other tabs' media", async () => {
  const media = {
    1: [{ url: "https://cdn.example/a/master.m3u8", kind: "stream", contentType: HLS, size: null, frameId: 0, ts: 1 }],
    2: [{ url: "https://cdn.example/b/master.m3u8", kind: "stream", contentType: HLS, size: null, frameId: 0, ts: 2 }],
  };
  const bg = load({ session: { media } });
  await bg.chrome.tabs.onUpdated.fire(1, { url: "https://site.example/next" });
  await bg.chrome.tabs.onRemoved.fire(3);
  await tick(500); // the save is debounced
  assert.deepEqual(Object.keys(bg.session.media), ["2"]);
  assert.equal(bg.session.media[2][0].url, "https://cdn.example/b/master.m3u8");
});

test("a change of the fragment alone keeps the tab's media", async () => {
  const bg = load();
  await bg.chrome.tabs.onUpdated.fire(1, { url: "https://site.example/video?id=1" });
  await sniff(bg, { url: "https://cdn.example/v/master.m3u8", type: HLS });
  await bg.chrome.tabs.onUpdated.fire(1, { url: "https://site.example/video?id=1#t=30" });
  assert.equal(bg.run("mediaByTab.get(1)?.length"), 1);
  await bg.chrome.tabs.onUpdated.fire(1, { url: "https://site.example/video?id=2" });
  assert.equal(bg.run("mediaByTab.get(1)?.length ?? 0"), 0);
});

// ------------------------------------------------------- the overlay button
/** What the button over a blob: player sends (resolveTarget), as content.js asks. */
function pick(bg, msg, sender = { tab: { id: 1, url: "https://site.example/" }, frameId: 0 }) {
  const full = { src: "blob:https://site.example/1", pageUrl: "https://site.example/", ...msg };
  return bg.run(`resolveTarget(${JSON.stringify(full)}, ${JSON.stringify(sender)})`).then(plain);
}

test("the button sends the stream its own player loaded", async () => {
  const bg = load();
  const a = "https://cdn.example/a/master.m3u8";
  const a720 = "https://cdn.example/a/720p.m3u8";
  const b = "https://cdn.example/b/master.m3u8";
  for (const url of [a, a720, b]) await sniff(bg, { url, type: HLS });
  bg.run(`for (const c of mediaByTab.get(1)) c.ts = ${JSON.stringify({ [a]: 1000, [a720]: 1100, [b]: 2000 })}[c.url]`);
  assert.equal((await pick(bg, { loadedAt: 1500 })).url, b, "the second player");
  assert.equal((await pick(bg, { loadedAt: 900 })).url, a, "the first player: its master playlist");
  assert.equal((await pick(bg, {})).url, a, "no load time: the page's first manifest");
  assert.equal((await pick(bg, { loadedAt: 5000 })).url, b, "nothing loaded since: the latest");
});

test("single videos and lists, site by site", () => {
  const bg = load();
  const single = (url) => bg.run(`isSingleVideoPage(${JSON.stringify(url)})`);
  for (const url of [
    "https://www.youtube.com/watch?v=jNQXAC9IVRw",
    "https://youtu.be/jNQXAC9IVRw",
    "https://vimeo.com/123456789",
    "https://vimeo.com/channels/staffpicks/123456789",
    "https://player.vimeo.com/video/123456789",
    "https://www.dailymotion.com/video/x8abcd",
    "https://www.instagram.com/reel/C1a2b3c4/",
    "https://www.instagram.com/p/C1a2b3c4/",
    "https://www.tiktok.com/@someone/video/7234567890123456789",
    "https://vm.tiktok.com/ZMabc123/",
    "https://www.facebook.com/watch/?v=123456",
    "https://www.facebook.com/somepage/videos/123456/",
    "https://www.twitch.tv/videos/123456",
    "https://www.twitch.tv/somechannel",
    "https://x.com/someone/status/1234567890",
    "https://twitter.com/someone/status/1234567890/video/1",
    "https://www.reddit.com/r/videos/comments/abc123/a_title/",
    "https://www.bilibili.com/video/BV1xx411c7mD",
    "https://rumble.com/v4abc-some-video.html",
    "https://soundcloud.com/artist/a-track",
    "https://artist.bandcamp.com/track/a-track",
    "https://kick.com/somechannel",
    "https://example.com/any/page",
  ]) {
    assert.equal(single(url), true, url);
  }
  for (const url of [
    "https://www.youtube.com/",
    "https://www.youtube.com/results?search_query=cats",
    "https://www.youtube.com/@channel",
    "https://vimeo.com/channels/staffpicks",
    "https://vimeo.com/someone",
    "https://vimeo.com/showcase/1234567",
    "https://www.dailymotion.com/someone",
    "https://www.instagram.com/",
    "https://www.instagram.com/someone/",
    "https://www.tiktok.com/@someone",
    "https://www.tiktok.com/foryou",
    "https://www.facebook.com/",
    "https://www.facebook.com/watch/",
    "https://www.twitch.tv/",
    "https://www.twitch.tv/directory",
    "https://www.twitch.tv/somechannel/videos",
    "https://x.com/home",
    "https://x.com/someone",
    "https://www.reddit.com/",
    "https://www.reddit.com/r/videos/",
    "https://www.bilibili.com/",
    "https://rumble.com/c/somechannel",
    "https://soundcloud.com/artist",
    "https://soundcloud.com/artist/sets/an-album",
    "https://artist.bandcamp.com/album/an-album",
    "https://kick.com/",
  ]) {
    assert.equal(single(url), false, url);
  }
});

test("in a feed, the button sends the video the preview links to", async () => {
  const bg = load();
  const feed = "https://www.tiktok.com/@someone";
  const video = "https://www.tiktok.com/@someone/video/7234567890123456789";
  const sender = { tab: { id: 1, url: feed }, frameId: 0 };
  assert.equal((await pick(bg, { pageUrl: feed, linkUrl: video }, sender)).url, video);
  await assert.rejects(pick(bg, { pageUrl: feed, linkUrl: "https://www.tiktok.com/@other" }, sender), "a profile is no video");
  const reddit = "https://www.reddit.com/";
  const post = "https://www.reddit.com/r/videos/comments/abc123/a_title/";
  assert.equal((await pick(bg, { pageUrl: reddit, linkUrl: post }, { tab: { id: 1, url: reddit }, frameId: 0 })).url, post);
  const own = "https://vimeo.com/123456789";
  const other = "https://vimeo.com/987654321";
  assert.equal((await pick(bg, { pageUrl: own, linkUrl: other }, { tab: { id: 1, url: own }, frameId: 0 })).url, own, "a video's own page wins");
});

// ------------------------------------------------------------- takeover
const ZIP = {
  url: "https://files.example/get?id=1",
  finalUrl: "https://files.example/a.zip",
  filename: "C:\\Downloads\\a.zip",
  mime: "application/zip",
  state: "in_progress",
  referrer: "https://files.example/",
};
const actions = (bg) => bg.calls.map(([name]) => name).filter((name) => name !== "cookies");
const downloadsSent = (bg) => bg.sent.filter((m) => m.type === "download");

test("the app gets the download: the browser's is held, then cancelled and erased", async () => {
  const bg = load({ downloads: { 7: ZIP } });
  await bg.chrome.downloads.onCreated.fire({ id: 7, ...ZIP });
  assert.deepEqual(downloadsSent(bg).map((m) => [m.url, m.fileName]), [[ZIP.finalUrl, "a.zip"]]);
  assert.deepEqual(actions(bg), ["pause", "cancel", "erase"]);
});

test("a download that finished before the takeover stays with the browser", async () => {
  const bg = load({ downloads: { 7: { ...ZIP, state: "complete" } } });
  await bg.chrome.downloads.onCreated.fire({ id: 7, ...ZIP }); // seen in progress when created
  assert.deepEqual(downloadsSent(bg), []);
  assert.ok(bg.items.has(7), "not erased");
});

test("when the app refuses, the browser's download goes on", async () => {
  const native = (msg) => (msg.type === "settings" ? { type: "settings", fileTypes: ["zip"] } : { type: "error", message: "no" });
  const bg = load({ downloads: { 7: ZIP }, native });
  await bg.chrome.downloads.onCreated.fire({ id: 7, ...ZIP });
  assert.deepEqual(actions(bg), ["pause", "resume"]);
  assert.equal(bg.items.get(7).paused, false);
});

test("without the app the download is not touched; once installed, the next one is taken", async () => {
  let installed = false;
  const native = (msg) => {
    if (!installed) throw new Error("Specified native messaging host not found.");
    return msg.type === "settings" ? { type: "settings", fileTypes: ["zip"] } : { type: "added", id: 1 };
  };
  const bg = load({ downloads: { 7: ZIP, 8: ZIP }, native });
  await bg.chrome.downloads.onCreated.fire({ id: 7, ...ZIP });
  assert.deepEqual(actions(bg), []);
  installed = true;
  await bg.chrome.downloads.onCreated.fire({ id: 8, ...ZIP });
  assert.deepEqual(actions(bg), ["pause", "cancel", "erase"]);
  assert.equal(downloadsSent(bg).length, 1);
});

test("the name's extension decides, the type only when the name has none", () => {
  const bg = load();
  const prefs = { fileTypes: ["zip", "mp4"] };
  const intercept = (filename, mime) => bg.run("shouldIntercept")({ filename, mime }, prefs);
  assert.equal(intercept("/home/u/report.pdf", "application/octet-stream"), false);
  assert.equal(intercept("/home/u/clip.webm", "video/webm"), false);
  assert.equal(intercept("/home/u/a.zip", "text/html"), true);
  assert.equal(intercept("/home/u/video", "video/mp4"), true);
  assert.equal(intercept("/home/u/notes", "text/plain"), false);
});

test("a download Chrome names later is judged by that name", async () => {
  const unnamed = { ...ZIP, finalUrl: "https://files.example/storage/abc", filename: "", mime: "application/octet-stream" };
  const bg = load({ downloads: { 7: unnamed, 8: unnamed } });
  for (const [id, name] of [[7, "C:\\Downloads\\report.pdf"], [8, "C:\\Downloads\\model.zip"]]) {
    await bg.chrome.downloads.onCreated.fire({ id, ...unnamed });
    bg.items.get(id).filename = name;
    await bg.chrome.downloads.onChanged.fire({ id, filename: { current: name } });
    await tick(20);
  }
  assert.deepEqual(downloadsSent(bg).map((m) => m.fileName), ["model.zip"]);
});

// --------------------------------------------------- frames and cookies
test("what a frame loaded goes with the frame's address (popup)", async () => {
  const tab = { id: 1, url: "https://news.example/article" };
  const bg = load({ tabs: { 1: tab }, frames: { 5: "https://player.example/embed/9" } });
  // Firefox gives the frame's address (documentUrl); Chrome's frame is asked.
  await sniff(bg, { frameId: 3, url: "https://cdn.example/a/master.m3u8", type: HLS, documentUrl: "https://player.example/embed/1" });
  await sniff(bg, { frameId: 5, url: "https://cdn.example/b/master.m3u8", type: HLS });
  for (const url of ["https://cdn.example/a/master.m3u8", "https://cdn.example/b/master.m3u8", "https://news.example/article"]) {
    await bg.run("handle")({ type: "popup-download", tabId: 1, item: { url, kind: "stream" } }, {});
  }
  assert.deepEqual(
    downloadsSent(bg).map((m) => [m.referrer, m.pageUrl]),
    [
      ["https://player.example/embed/1", tab.url],
      ["https://player.example/embed/9", tab.url],
      [tab.url, tab.url],
    ],
  );
});

test("the menus in a frame go with the frame's address", async () => {
  const tab = { id: 1, url: "https://news.example/article" };
  const bg = load({ tabs: { 1: tab } });
  const frame = { pageUrl: tab.url, frameUrl: "https://player.example/embed/1", frameId: 3 };
  await bg.chrome.contextMenus.onClicked.fire({ menuItemId: "fisga-media", srcUrl: "https://cdn.example/clip.mp4", ...frame }, tab);
  await bg.chrome.contextMenus.onClicked.fire({ menuItemId: "fisga-link", linkUrl: "https://files.example/a.zip", ...frame }, tab);
  assert.deepEqual(downloadsSent(bg).map((m) => m.referrer), [frame.frameUrl, frame.frameUrl]);
});

test("cookies come from the tab's store; private downloads stay with the browser", async () => {
  const container = { id: 1, url: "https://site.example/", cookieStoreId: "firefox-container-2" };
  const incognito = { id: 2, url: "https://site.example/", incognito: true };
  const bg = load({
    stores: [{ id: "0", tabIds: [3] }, { id: "1", tabIds: [2] }],
    downloads: { 7: { ...ZIP, incognito: true }, 8: { ...ZIP, cookieStoreId: "firefox-container-3" } },
  });
  for (const [file, tab] of [["b.zip", container], ["c.zip", incognito], ["d.zip", { id: 3, url: "https://site.example/" }]]) {
    await bg.run("forceLink")({ url: "https://files.example/" + file }, { tab });
  }
  await bg.chrome.downloads.onCreated.fire({ id: 7, ...ZIP, incognito: true });
  await bg.chrome.downloads.onCreated.fire({ id: 8, ...ZIP, cookieStoreId: "firefox-container-3" });
  const stores = bg.calls.filter(([name]) => name === "cookies").map(([, details]) => details.storeId ?? "default");
  assert.deepEqual(stores, ["firefox-container-2", "1", "default", "firefox-container-3"]);
  assert.equal(bg.items.get(7).paused, false, "the private download is not touched");
});

test("the link menu sends a video's page to yt-dlp, not its HTML", async () => {
  const bg = load();
  const tab = { id: 1, url: "https://blog.example/" };
  for (const linkUrl of ["https://vimeo.com/123456789", "https://www.tiktok.com/@someone", "https://files.example/a.zip"]) {
    await bg.chrome.contextMenus.onClicked.fire({ menuItemId: "fisga-link", linkUrl, pageUrl: tab.url }, tab);
  }
  assert.deepEqual(
    bg.sent.filter((m) => m.type === "download").map((m) => [m.url, m.kind]),
    [
      ["https://vimeo.com/123456789", "page"],
      ["https://www.tiktok.com/@someone", "direct"],
      ["https://files.example/a.zip", "direct"],
    ],
  );
});
