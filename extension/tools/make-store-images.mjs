// Renders the store listing's images into extension/store/ with headless
// Chrome: the Chrome Web Store's small promo tile (440x280) and screenshots
// (1280x800) made from docs/screenshots, reused on addons.mozilla.org.
// Run: node extension/tools/make-store-images.mjs   (CHROME=/path/to/chrome overrides the browser)
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const out = join(root, "extension", "store");
const dataUrl = (file, type) => `data:${type};base64,${readFileSync(file).toString("base64")}`;
const icon = dataUrl(join(root, "crates", "fisga-gui", "data", "io.fisga.Fisga.svg"), "image/svg+xml");
const shot = (name) => dataUrl(join(root, "docs", "screenshots", name), "image/png");
const CHROME =
  process.env.CHROME ??
  (process.platform === "win32" ? "C:/Program Files/Google/Chrome/Application/chrome.exe" : "google-chrome");

const style = `
  * { margin: 0; box-sizing: border-box; }
  body { width: 100vw; height: 100vh; overflow: hidden; background: #161c27; color: #e6e9ef;
         font-family: "Segoe UI", "Cantarell", "Noto Sans", sans-serif; }
  .accent { color: #f59e0b; }`;

const images = {
  "promo-440x280.png": [440, 280, `
    <style>${style}
      body { display: flex; align-items: center; gap: 22px; padding: 0 34px; }
      img { width: 120px; height: 120px; }
      h1 { font-size: 52px; font-weight: 700; }
      p { font-size: 19px; line-height: 1.3; margin-top: 6px; opacity: .85; }
    </style>
    <img src="${icon}"><div><h1>Fisga</h1><p>Downloads <span class="accent">acelerados</span><br>direto do navegador</p></div>`],
  "screenshot-1-1280x800.png": [1280, 800, `
    <style>${style}
      body { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 26px; }
      h1 { font-size: 34px; font-weight: 600; }
      img { width: 1060px; border-radius: 12px; box-shadow: 0 18px 50px rgba(0, 0, 0, .55); }
    </style>
    <h1>Seus downloads em <span class="accent">várias conexões</span>, com pausa e retomada</h1>
    <img src="${shot("janela-principal.png")}">`],
  "screenshot-2-1280x800.png": [1280, 800, `
    <style>${style}
      body { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 34px; }
      h1 { font-size: 34px; font-weight: 600; text-align: center; line-height: 1.35; }
      img { width: 870px; border-radius: 12px; box-shadow: 0 18px 50px rgba(0, 0, 0, .55); }
    </style>
    <h1>Clique em <span class="accent">Baixar</span> no vídeo ou no link<br>e escolha onde salvar</h1>
    <img src="${shot("novo-download.png")}">`],
};

mkdirSync(out, { recursive: true });
const work = mkdtempSync(join(tmpdir(), "fisga-store-"));
try {
  for (const [name, [width, height, body]] of Object.entries(images)) {
    const page = join(work, `${name}.html`);
    writeFileSync(page, `<!doctype html><meta charset="utf-8">${body}`);
    execFileSync(CHROME, [
      "--headless=new",
      "--disable-gpu",
      "--hide-scrollbars",
      `--window-size=${width},${height}`,
      `--screenshot=${join(out, name)}`,
      pathToFileURL(page).href,
    ], { stdio: "ignore" });
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
console.log(`store images written to ${out}`);
