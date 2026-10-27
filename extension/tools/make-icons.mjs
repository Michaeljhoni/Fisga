// Renders PNGs of the app's icon, crates/fisga-gui/data/io.fisga.Fisga.svg,
// with headless Chrome: the extension's toolbar icons (16 to 128 px) and the
// app's own sizes (crates/fisga-gui/data/icons/<size>/io.fisga.Fisga.png), for
// systems without an SVG loader; the window's header embeds the 64 px one.
// Run: node extension/tools/make-icons.mjs   (CHROME=/path/to/chrome overrides the browser)
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const svg = readFileSync(join(root, "crates", "fisga-gui", "data", "io.fisga.Fisga.svg"), "utf8");
const extensionIcons = join(root, "extension", "src", "icons");
const appIcons = join(root, "crates", "fisga-gui", "data", "icons");
const targets = [
  ...[16, 32, 48, 128].map((size) => [size, join(extensionIcons, `icon-${size}.png`)]),
  ...[32, 48, 64, 128, 256].map((size) => [size, join(appIcons, String(size), "io.fisga.Fisga.png")]),
];
const CHROME =
  process.env.CHROME ??
  (process.platform === "win32" ? "C:/Program Files/Google/Chrome/Application/chrome.exe" : "google-chrome");

const work = mkdtempSync(join(tmpdir(), "fisga-icons-"));
try {
  for (const [size, png] of targets) {
    mkdirSync(dirname(png), { recursive: true });
    const page = join(work, `icon-${size}.html`);
    const data = `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;
    writeFileSync(page, `<!doctype html><body style="margin:0"><img src="${data}" width="${size}" height="${size}" style="display:block">`);
    execFileSync(CHROME, [
      "--headless=new",
      "--disable-gpu",
      "--hide-scrollbars",
      "--default-background-color=00000000",
      `--window-size=${size},${size}`,
      `--screenshot=${png}`,
      pathToFileURL(page).href,
    ], { stdio: "ignore" });
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
console.log(`icons written to ${extensionIcons} and ${appIcons}`);
