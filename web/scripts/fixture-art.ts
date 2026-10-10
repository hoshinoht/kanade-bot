// Writes synthetic boss art for tests and CI captures (never the real art,
// which is deployment-private). Deterministic: re-running changes nothing.
// Some bosses deliberately have no files, to exercise "absent means absent".
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { deflateSync } from 'node:zlib';

const root = join(import.meta.dir, '..', 'e2e', 'fixtures', 'boss');

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc = (buf: Uint8Array) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

function chunk(type: string, data: Uint8Array): Buffer {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, 'ascii');
  const body = Buffer.concat([head.subarray(4), data]);
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(crc(body), 0);
  return Buffer.concat([head.subarray(0, 4), body, tail]);
}

type Rgb = [number, number, number];

/** A diagonal two-tone stripe field with a disc: legible as "art", obviously fake. */
function png(width: number, height: number, a: Rgb, b: Rgb): Buffer {
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 3 + 1)] = 0;
    for (let x = 0; x < width; x++) {
      const stripe = Math.floor((x + y) / Math.max(8, width / 12)) % 2 === 0;
      const dx = x - width * 0.62;
      const dy = y - height * 0.45;
      const disc = dx * dx + dy * dy < (Math.min(width, height) * 0.28) ** 2;
      const [r, g, bl] = disc ? [255, 255, 255] : stripe ? a : b;
      const o = y * (width * 3 + 1) + 1 + x * 3;
      raw[o] = r;
      raw[o + 1] = g;
      raw[o + 2] = bl;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', new Uint8Array()),
  ]);
}

const palette: Record<string, [Rgb, Rgb]> = {
  Carling: [[208, 32, 183], [120, 40, 160]],
  MaleficStar: [[248, 221, 74], [210, 150, 40]],
  Kalos: [[240, 120, 37], [150, 60, 30]],
  Limbo: [[146, 38, 197], [70, 30, 110]],
  BM: [[60, 50, 50], [150, 40, 40]],
  FA: [[169, 216, 245], [60, 110, 170]],
};

const sets: [string, string[], number, number][] = [
  ['portraits', ['Carling', 'MaleficStar', 'Kalos', 'Limbo', 'BM'], 128, 128],
  ['portraits/icon', ['Carling', 'MaleficStar'], 64, 64],
  // Limbo has a portrait but no entry art; Baldrix, Bellona, Jupiter, Seren, Lotus have nothing.
  ['artwork/entry', ['Carling', 'MaleficStar', 'Kalos', 'BM', 'FA'], 480, 320],
];

for (const [dir, keys, w, h] of sets) {
  mkdirSync(join(root, dir), { recursive: true });
  for (const key of keys) {
    const [a, b] = palette[key]!;
    writeFileSync(join(root, dir, `${key}.png`), png(w, h, a, b));
  }
}
console.log(`fixture art written under ${root}`);
