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
    console,
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
