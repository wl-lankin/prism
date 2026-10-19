// Placeholder icon generator: draws a simple spectrum square and lets the Tauri CLI build every icon size.
// Run with `npm run icons`. A real logo replaces this later.
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { deflateSync } from 'node:zlib';

const root = new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const SIZE = 1024;
const bands = [
  [0xff, 0x4d, 0x6d],
  [0xff, 0xc9, 0x4d],
  [0x4d, 0xff, 0xb2],
  [0x4d, 0xa6, 0xff],
];

function crc32(buf) {
  let c;
  let crc = 0xffffffff;
  for (let n = 0; n < buf.length; n++) {
    c = (crc ^ buf[n]) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, 'ascii');
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}

function png() {
  const margin = SIZE / 8;
  const raw = Buffer.alloc((SIZE * 4 + 1) * SIZE);
  for (let y = 0; y < SIZE; y++) {
    const row = y * (SIZE * 4 + 1);
    for (let x = 0; x < SIZE; x++) {
      const inside = x >= margin && x < SIZE - margin && y >= margin && y < SIZE - margin;
      const band = bands[Math.min(3, Math.floor(((x - margin) / (SIZE - 2 * margin)) * 4))] ?? bands[0];
      const o = row + 1 + x * 4;
      raw[o] = inside ? band[0] : 0x0d;
      raw[o + 1] = inside ? band[1] : 0x0d;
      raw[o + 2] = inside ? band[2] : 0x10;
      raw[o + 3] = 0xff;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(SIZE, 0);
  ihdr.writeUInt32BE(SIZE, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

const work = mkdtempSync(join(tmpdir(), 'prism-icons-'));
try {
  const source = join(work, 'icon.png');
  writeFileSync(source, png());
  execFileSync('npx', ['tauri', 'icon', source, '-o', 'src-tauri/icons'], { cwd: root, stdio: 'inherit', shell: true });
} finally {
  rmSync(work, { recursive: true, force: true });
}
