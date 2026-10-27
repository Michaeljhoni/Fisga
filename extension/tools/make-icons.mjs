// Renders the extension's toolbar icons (PNG, 16 to 128 px) from the app's
// icon, crates/fisga-gui/data/io.fisga.Fisga.svg, with headless Chrome.
// Run: node extension/tools/make-icons.mjs   (CHROME=/path/to/chrome overrides the browser)
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const svg = readFileSync(join(root, "crates", "fisga-gui", "data", "io.fisga.Fisga.svg"), "utf8");
const out = join(root, "extension", "src", "icons");
const CHROME =
  process.env.CHROME ??
  (process.platform === "win32" ? "C:/Program Files/Google/Chrome/Application/chrome.exe" : "google-chrome");

const work = mkdtempSync(join(tmpdir(), "fisga-icons-"));
try {
  for (const size of [16, 32, 48, 128]) {
    const page = join(work, `icon-${size}.html`);
    const data = `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;
    writeFileSync(page, `<!doctype html><body style="margin:0"><img src="${data}" width="${size}" height="${size}" style="display:block">`);
    execFileSync(CHROME, [
      "--headless=new",
      "--disable-gpu",
      "--hide-scrollbars",
      "--default-background-color=00000000",
      `--window-size=${size},${size}`,
      `--screenshot=${join(out, `icon-${size}.png`)}`,
      pathToFileURL(page).href,
    ], { stdio: "ignore" });
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
console.log(`icons written to ${out}`);
