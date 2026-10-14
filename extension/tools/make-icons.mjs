// Generates the toolbar/extension icons (blue rounded square, white download
// arrow) as PNGs without any dependency. Run: node extension/tools/make-icons.mjs
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const out = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "icons");
mkdirSync(out, { recursive: true });

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}
function png(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// Shapes in a 0..1 coordinate space.
function insideRoundedSquare(x, y) {
  const r = 0.22;
  const cx = Math.min(Math.max(x, r), 1 - r);
  const cy = Math.min(Math.max(y, r), 1 - r);
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
}
function insideArrow(x, y) {
  const shaft = x >= 0.43 && x <= 0.57 && y >= 0.2 && y <= 0.55;
  // Triangle pointing down: apex (0.5, 0.72), base y = 0.48 from 0.27 to 0.73.
  const head = y >= 0.46 && y <= 0.72 && Math.abs(x - 0.5) <= (0.72 - y) * (0.23 / 0.26);
  const tray = x >= 0.22 && x <= 0.78 && y >= 0.77 && y <= 0.85;
  return shaft || head || tray;
}

for (const size of [16, 32, 48, 128]) {
  const rgba = Buffer.alloc(size * size * 4);
  const ss = 4; // supersampling for anti-aliasing
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      let bg = 0;
      let fg = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const x = (px + (sx + 0.5) / ss) / size;
          const y = (py + (sy + 0.5) / ss) / size;
          if (insideRoundedSquare(x, y)) {
            if (insideArrow(x, y)) fg++;
            else bg++;
          }
        }
      }
      const n = ss * ss;
      const alpha = (bg + fg) / n;
      const mix = bg + fg ? fg / (bg + fg) : 0;
      // Background gradient from #3b82f6 (top) to #1d4ed8 (bottom).
      const k = py / size;
      const base = [59 + (29 - 59) * k, 130 + (78 - 130) * k, 246 + (216 - 246) * k];
      const i = (py * size + px) * 4;
      rgba[i] = Math.round(base[0] + (255 - base[0]) * mix);
      rgba[i + 1] = Math.round(base[1] + (255 - base[1]) * mix);
      rgba[i + 2] = Math.round(base[2] + (255 - base[2]) * mix);
      rgba[i + 3] = Math.round(alpha * 255);
    }
  }
  writeFileSync(join(out, `icon-${size}.png`), png(size, rgba));
}
console.log(`icons written to ${out}`);
