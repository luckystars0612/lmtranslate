// Generates app-icon.png — the source image for `npm run tauri icon`.
// The mark is the gloss rail from DESIGN.md: a cool→warm bar binding a block of
// "source" lines above a block of "target" lines.
//
// Written as a tiny hand-rolled PNG encoder so the build needs no image library.

import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const S = 1024;
const px = new Uint8Array(S * S * 4);

const BG = [0x17, 0x1b, 0x24];
const SOURCE = [0x7c, 0x9c, 0xe8];
const TARGET = [0xe8, 0xa3, 0x3d];

const lerp = (a, b, t) => a.map((v, i) => Math.round(v + (b[i] - v) * Math.min(Math.max(t, 0), 1)));

function blend(x, y, [r, g, b], a) {
  if (a <= 0) return;
  const i = (y * S + x) * 4;
  const dstA = px[i + 3] / 255;
  const outA = a + dstA * (1 - a);
  if (outA <= 0) return;
  const src = [r, g, b];
  for (let c = 0; c < 3; c++) {
    px[i + c] = Math.round((src[c] * a + px[i + c] * dstA * (1 - a)) / outA);
  }
  px[i + 3] = Math.round(outA * 255);
}

// Signed-distance test: clamp the sample to the inset rect, then check the corner radius.
function inside(sx, sy, x, y, w, h, r) {
  if (sx < x || sx > x + w || sy < y || sy > y + h) return false;
  const cx = Math.min(Math.max(sx, x + r), x + w - r);
  const cy = Math.min(Math.max(sy, y + r), y + h - r);
  const dx = sx - cx;
  const dy = sy - cy;
  return dx * dx + dy * dy <= r * r;
}

function roundRect(x, y, w, h, r, colorAt, alpha = 1) {
  const N = 4; // 4×4 supersampling for edge antialiasing
  for (let py = Math.max(0, Math.floor(y)); py < Math.min(S, Math.ceil(y + h)); py++) {
    for (let pxi = Math.max(0, Math.floor(x)); pxi < Math.min(S, Math.ceil(x + w)); pxi++) {
      let hits = 0;
      for (let sy = 0; sy < N; sy++) {
        for (let sx = 0; sx < N; sx++) {
          if (inside(pxi + (sx + 0.5) / N, py + (sy + 0.5) / N, x, y, w, h, r)) hits++;
        }
      }
      if (hits) blend(pxi, py, colorAt(pxi, py), (alpha * hits) / (N * N));
    }
  }
}

// Ground
roundRect(0, 0, S, S, 224, () => BG);

// The rail: cool at the top, warm at the bottom.
const RAIL_Y = 236;
const RAIL_H = 552;
roundRect(232, RAIL_Y, 40, RAIL_H, 20, (_x, y) => lerp(SOURCE, TARGET, (y - RAIL_Y) / RAIL_H));

// Source lines (muted, cool) above target lines (bright, warm).
roundRect(328, 300, 470, 52, 26, () => SOURCE, 0.5);
roundRect(328, 376, 322, 52, 26, () => SOURCE, 0.5);
roundRect(328, 596, 470, 52, 26, () => TARGET, 0.95);
roundRect(328, 672, 386, 52, 26, () => TARGET, 0.95);

// ── PNG encoding ───────────────────────────────────────────────────────────

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
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

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
// bytes 10-12 stay zero: deflate, adaptive filtering, no interlace

const stride = S * 4;
const raw = Buffer.alloc((stride + 1) * S);
for (let y = 0; y < S; y++) {
  raw[y * (stride + 1)] = 0; // filter type: none
  Buffer.from(px.buffer, y * stride, stride).copy(raw, y * (stride + 1) + 1);
}

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const out = fileURLToPath(new URL("../app-icon.png", import.meta.url));
writeFileSync(out, png);
console.log(`wrote ${out} (${S}×${S}, ${png.length} bytes)`);
