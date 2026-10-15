// Real-click test of the overlay button on YouTube, in the installed Chrome
// (Windows or Linux). Chrome is driven over the DevTools protocol (pipe
// transport, which allows Extensions.loadUnpacked) with trusted mouse events.
// Native messaging is stubbed inside the extension's service worker, so the
// report shows exactly what the extension would send to the app; nothing is
// downloaded.
//
// Usage: node extension/test/real-clicks.mjs <extension-dir> <label> [scenario...]
//   scenarios: search, menu-search, watch, spa (search -> click a video -> button), home
//   CHROME=/path/to/chrome overrides the browser; results go to extension/test/out/<label>/.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

const CHROME =
  process.env.CHROME ??
  (process.platform === "win32" ? "C:/Program Files/Google/Chrome/Application/chrome.exe" : "google-chrome");
const [extDir, label = "test", ...scenarios] = process.argv.slice(2);
const here = resolve(import.meta.dirname);
const out = join(here, "out", label);
mkdirSync(out, { recursive: true });
const profile = join(here, "profile-" + label);
rmSync(profile, { recursive: true, force: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log(`[${label}]`, ...a);

class CDP {
  constructor(write, read) {
    this.id = 0;
    this.pending = new Map();
    this.listeners = [];
    this.write = write;
    let buf = Buffer.alloc(0);
    read.on("data", (chunk) => {
      buf = Buffer.concat([buf, chunk]);
      let i;
      while ((i = buf.indexOf(0)) >= 0) {
        const msg = JSON.parse(buf.subarray(0, i).toString("utf8"));
        buf = buf.subarray(i + 1);
        this.dispatch(msg);
      }
    });
  }
  dispatch(m) {
    if (m.id && this.pending.has(m.id)) {
      const { resolve, reject, method } = this.pending.get(m.id);
      this.pending.delete(m.id);
      m.error ? reject(new Error(`${method}: ${m.error.message}`)) : resolve(m.result);
    } else for (const l of this.listeners) l(m);
  }
  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const msg = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    this.write.write(JSON.stringify(msg) + "\0");
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject, method }));
  }
}

const proc = spawn(
  CHROME,
  [
    `--user-data-dir=${profile}`,
    "--remote-debugging-pipe",
    "--enable-unsafe-extension-debugging",
    "--no-first-run",
    "--no-default-browser-check",
    "--autoplay-policy=no-user-gesture-required",
    "--window-size=1400,950",
    "--lang=pt-BR",
    "about:blank",
  ],
  { stdio: ["ignore", "ignore", "ignore", "pipe", "pipe"] },
);
const cdp = new CDP(proc.stdio[3], proc.stdio[4]);

async function evaluate(session, expression) {
  const r = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, session);
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
}

const STUB = `(() => {
  globalThis.__ldmSent ??= [];
  const fake = async (host, msg) => {
    globalThis.__ldmSent.push(msg);
    if (msg.type === "ping") return { type: "pong", version: "stub", daemon: true };
    return { type: "added", id: globalThis.__ldmSent.length, pending: msg.confirm !== false };
  };
  try { chrome.runtime.sendNativeMessage = fake; } catch {}
  if (chrome.runtime.sendNativeMessage !== fake) Object.defineProperty(chrome.runtime, "sendNativeMessage", { value: fake, configurable: true });
  return chrome.runtime.sendNativeMessage === fake;
})()`;

let extId;
async function worker() {
  for (let i = 0; i < 50; i++) {
    const { targetInfos } = await cdp.send("Target.getTargets");
    const sw = targetInfos.find((t) => t.type === "service_worker" && t.url.startsWith(`chrome-extension://${extId}/`));
    if (sw) {
      const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: sw.targetId, flatten: true });
      if (!(await evaluate(sessionId, STUB))) throw new Error("stub not installed");
      return sessionId;
    }
    await sleep(200);
  }
  throw new Error("extension service worker not found");
}

async function sent(sw) {
  return evaluate(sw, "JSON.stringify(globalThis.__ldmSent.filter(m => m.type !== 'ping').map(m => ({url: m.url, kind: m.kind, title: m.title, confirm: m.confirm})))").then(JSON.parse);
}

async function shot(page, name) {
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" }, page);
  writeFileSync(join(out, name + ".png"), Buffer.from(data, "base64"));
}

async function mouse(page, type, x, y, extra = {}) {
  await cdp.send("Input.dispatchMouseEvent", { type, x, y, button: "none", ...extra }, page);
}
async function moveTo(page, x, y, from = { x: x - 200, y: y + 150 }) {
  const steps = 12;
  for (let i = 1; i <= steps; i++) {
    await mouse(page, "mouseMoved", from.x + ((x - from.x) * i) / steps, from.y + ((y - from.y) * i) / steps);
    await sleep(30);
  }
}
async function click(page, x, y) {
  await mouse(page, "mouseMoved", x, y);
  await mouse(page, "mousePressed", x, y, { button: "left", clickCount: 1 });
  await mouse(page, "mouseReleased", x, y, { button: "left", clickCount: 1 });
}

/** Box of a node inside the overlay's closed shadow root (DOM domain pierces it). */
async function overlayBox(page, className) {
  const { root } = await cdp.send("DOM.getDocument", { depth: -1, pierce: true }, page);
  let hostNode = null;
  const walk = (n) => {
    if (hostNode) return;
    if (n.nodeName === "LDM-DOWNLOAD-BUTTON") hostNode = n;
    for (const c of [...(n.children ?? []), ...(n.shadowRoots ?? []), ...(n.contentDocument ? [n.contentDocument] : [])]) walk(c);
  };
  walk(root);
  if (!hostNode) return null;
  let target = null;
  const find = (n) => {
    if (target) return;
    const attrs = n.attributes ?? [];
    const cls = attrs[attrs.indexOf("class") + 1];
    if (attrs.includes("class") && cls?.split(" ").includes(className)) target = n;
    for (const c of [...(n.children ?? []), ...(n.shadowRoots ?? [])]) find(c);
  };
  find(hostNode);
  if (!target) return null;
  try {
    const { model } = await cdp.send("DOM.getBoxModel", { nodeId: target.nodeId }, page);
    const q = model.border;
    return { x: (q[0] + q[2] + q[4] + q[6]) / 4, y: (q[1] + q[3] + q[5] + q[7]) / 4, w: model.width, h: model.height };
  } catch {
    return null; // display:none
  }
}

async function overlayLabel(page) {
  const { root } = await cdp.send("DOM.getDocument", { depth: -1, pierce: true }, page);
  let text = null;
  const walk = (n) => {
    const attrs = n.attributes ?? [];
    if (attrs[attrs.indexOf("class") + 1] === "label") text = n.children?.[0]?.nodeValue ?? null;
    for (const c of [...(n.children ?? []), ...(n.shadowRoots ?? [])]) walk(c);
  };
  walk(root);
  return text;
}

let pageSession = null;
async function openPage(url) {
  if (!pageSession) {
    const { targetInfos } = await cdp.send("Target.getTargets");
    const tab = targetInfos.find((t) => t.type === "page");
    const targetId = tab ? tab.targetId : (await cdp.send("Target.createTarget", { url: "about:blank" })).targetId;
    await cdp.send("Target.activateTarget", { targetId });
    ({ sessionId: pageSession } = await cdp.send("Target.attachToTarget", { targetId, flatten: true }));
    await cdp.send("Page.enable", {}, pageSession);
    await cdp.send("Runtime.enable", {}, pageSession);
    await cdp.send("Emulation.setFocusEmulationEnabled", { enabled: true }, pageSession);
  }
  const sessionId = pageSession;
  await cdp.send("Page.bringToFront", {}, sessionId);
  await cdp.send("Page.navigate", { url }, sessionId);
  for (let i = 0; i < 60; i++) {
    await sleep(500);
    if ((await evaluate(sessionId, "document.readyState").catch(() => "")) === "complete") break;
  }
  await sleep(4000);
  const info = await evaluate(sessionId, "({ url: location.href, title: document.title, text: document.body.innerText.slice(0, 200).replace(/\\s+/g, ' ') })");
  log(`page: ${JSON.stringify(info)}`);
  return sessionId;
}

const THUMBS = `(() => [...document.querySelectorAll('a[href*="/watch?v="]')]
  .filter(a => { const r = a.getBoundingClientRect(); return r.width > 200 && r.height > 100 && r.top > 80 && r.bottom < innerHeight - 10 && a.querySelector('img, yt-image, .yt-core-image'); })
  .slice(0, 6)
  .map(a => { const r = a.getBoundingClientRect(); return { href: a.href, x: r.left + r.width / 2, y: r.top + r.height / 2 }; }))()`;

const VIDEOS = `(() => [...document.querySelectorAll('video')].map(v => { const r = v.getBoundingClientRect(); return { w: Math.round(r.width), h: Math.round(r.height), paused: v.paused, src: (v.currentSrc || '').slice(0, 30) }; }).filter(v => v.w > 0))()`;

async function feedScenario(sw, name, url, useMenu = false) {
  log(`--- ${name}: ${url}`);
  const page = await openPage(url);
  const thumbs = await evaluate(page, THUMBS);
  log(`thumbnails found: ${thumbs.length}`);
  if (!thumbs.length) {
    await shot(page, `${name}-no-thumbs`);
    return { name, error: "no thumbnails" };
  }
  const results = [];
  for (const thumb of thumbs.slice(0, 2)) {
    await moveTo(page, thumb.x, thumb.y);
    let box = null;
    for (let i = 0; i < 20 && !box; i++) {
      await sleep(400);
      await mouse(page, "mouseMoved", thumb.x + (i % 2), thumb.y);
      box = await overlayBox(page, "main");
    }
    log(`hovered ${thumb.href} videos=${JSON.stringify(await evaluate(page, VIDEOS))} button=${box ? "shown" : "absent"}`);
    if (!box) {
      await shot(page, `${name}-no-button`);
      results.push({ expected: thumb.href, error: "button did not appear" });
      continue;
    }
    await shot(page, `${name}-hover-${results.length}`);
    const before = (await sent(sw)).length;
    if (useMenu) {
      const more = await overlayBox(page, "more");
      await click(page, more.x, more.y);
      await sleep(1500);
      await shot(page, `${name}-menu-${results.length}`);
      const first = await overlayBox(page, "item");
      if (first) await click(page, first.x, first.y);
    } else {
      await click(page, box.x, box.y);
    }
    await sleep(1500);
    const label = await overlayLabel(page);
    await shot(page, `${name}-clicked-${results.length}`);
    const now = await sent(sw);
    const msg = now.slice(before)[0] ?? null;
    results.push({ expected: thumb.href, sent: msg, buttonLabel: label });
    log(`sent: ${JSON.stringify(msg)} | button: ${label}`);
    await moveTo(page, 30, 500, { x: thumb.x, y: thumb.y });
    await sleep(1500);
  }
  return { name, results };
}

async function watchScenario(sw) {
  const url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
  log(`--- watch: ${url}`);
  const page = await openPage(url);
  const player = await evaluate(page, `(() => { const v = [...document.querySelectorAll('video')].find(v => v.getBoundingClientRect().width > 300); if (!v) return null; const r = v.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  if (!player) return { name: "watch", error: "no player" };
  await moveTo(page, player.x, player.y);
  let box = null;
  for (let i = 0; i < 15 && !box; i++) {
    await sleep(300);
    await mouse(page, "mouseMoved", player.x + (i % 2), player.y);
    box = await overlayBox(page, "main");
  }
  if (!box) {
    await shot(page, "watch-no-button");
    return { name: "watch", error: "button did not appear" };
  }
  await shot(page, "watch-hover");
  const before = (await sent(sw)).length;
  await click(page, box.x, box.y);
  await sleep(1500);
  await shot(page, "watch-clicked");
  const msg = (await sent(sw)).slice(before)[0] ?? null;
  log(`sent: ${JSON.stringify(msg)}`);
  return { name: "watch", results: [{ expected: url, sent: msg, buttonLabel: await overlayLabel(page) }] };
}

// Like the user: start on a list page, click a video (YouTube navigates with
// pushState, no reload), then use the button on the main player.
async function spaScenario(sw) {
  const start = "https://www.youtube.com/results?search_query=palworld+breeding";
  log(`--- spa: ${start}`);
  const page = await openPage(start);
  const thumbs = await evaluate(page, THUMBS);
  if (!thumbs.length) return { name: "spa", error: "no thumbnails" };
  const thumb = thumbs[0];
  await moveTo(page, thumb.x, thumb.y);
  await sleep(500);
  await click(page, thumb.x, thumb.y);
  for (let i = 0; i < 30; i++) {
    await sleep(500);
    if ((await evaluate(page, "location.pathname")) === "/watch") break;
  }
  await sleep(4000);
  const url = await evaluate(page, "location.href");
  const reloaded = await evaluate(page, "performance.getEntriesByType('navigation')[0].name");
  log(`now at ${url} (document loaded as ${reloaded})`);
  const player = await evaluate(page, `(() => { const v = [...document.querySelectorAll('video')].find(v => v.getBoundingClientRect().width > 300); if (!v) return null; const r = v.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  if (!player) return { name: "spa", error: "no player" };
  await moveTo(page, player.x, player.y);
  let box = null;
  for (let i = 0; i < 15 && !box; i++) {
    await sleep(300);
    await mouse(page, "mouseMoved", player.x + (i % 2), player.y);
    box = await overlayBox(page, "main");
  }
  if (!box) return { name: "spa", error: "button did not appear" };
  const before = (await sent(sw)).length;
  await click(page, box.x, box.y);
  await sleep(1500);
  await shot(page, "spa-clicked");
  const msg = (await sent(sw)).slice(before)[0] ?? null;
  log(`sent: ${JSON.stringify(msg)}`);
  return { name: "spa", results: [{ expected: url, sent: msg }] };
}

try {
  ({ id: extId } = await cdp.send("Extensions.loadUnpacked", { path: resolve(extDir) }));
  log(`extension loaded: ${extId}`);
  const sw = await worker();
  const report = [];
  for (const s of scenarios.length ? scenarios : ["home", "watch"]) {
    // The worker can be restarted between scenarios: reinstall the stub.
    await evaluate(sw, STUB).catch(() => {});
    if (s === "home") report.push(await feedScenario(sw, "home", "https://www.youtube.com/?gl=BR&tab=w1"));
    if (s === "menu-home") report.push(await feedScenario(sw, "menu-home", "https://www.youtube.com/?gl=BR&tab=w1", true));
    if (s === "menu-search") report.push(await feedScenario(sw, "menu-search", "https://www.youtube.com/results?search_query=palworld+breeding", true));
    if (s === "search") report.push(await feedScenario(sw, "search", "https://www.youtube.com/results?search_query=palworld+breeding"));
    if (s === "watch") report.push(await watchScenario(sw));
    if (s === "spa") report.push(await spaScenario(sw));
  }
  writeFileSync(join(out, "report.json"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
} catch (e) {
  console.error(`[${label}] FAILED:`, e.message);
  process.exitCode = 1;
} finally {
  await cdp.send("Browser.close").catch(() => {});
  setTimeout(() => proc.kill(), 2000).unref();
}
