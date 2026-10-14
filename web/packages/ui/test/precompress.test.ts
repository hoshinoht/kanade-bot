import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { brotliDecompressSync, gunzipSync } from 'node:zlib';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { precompressDir } from '../src/vite';

describe('precompressDir', () => {
  let dir = '';
  const big = 'export const rows = [' + '"a fairly repetitive row",'.repeat(200) + '];\n';

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'kanade-precompress-'));
    mkdirSync(join(dir, 'assets'));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it('writes smaller br and gz siblings that decode to the source', () => {
    for (const name of ['assets/app.js', 'assets/app.css', 'index.html', 'sw.js', 'icon.svg', 'manifest.webmanifest', 'data.json']) {
      writeFileSync(join(dir, name), big);
    }
    const stats = precompressDir(dir);
    expect(stats.br).toBe(7);
    expect(stats.gz).toBe(7);
    const source = readFileSync(join(dir, 'assets/app.js'));
    const br = readFileSync(join(dir, 'assets/app.js.br'));
    const gz = readFileSync(join(dir, 'assets/app.js.gz'));
    expect(brotliDecompressSync(br)).toEqual(source);
    expect(gunzipSync(gz)).toEqual(source);
    expect(br.length).toBeLessThan(source.length);
    expect(stats.saved).toBe(7 * (source.length - Math.min(br.length, gz.length)));
  });

  it('skips small, incompressible and binary files', () => {
    writeFileSync(join(dir, 'assets/tiny.js'), 'let a=1;');
    writeFileSync(join(dir, 'assets/font.woff2'), big);
    writeFileSync(join(dir, 'icon.png'), big);
    // Random bytes do not shrink: no sibling is kept.
    writeFileSync(join(dir, 'assets/noise.js'), Buffer.from(Array.from({ length: 4096 }, () => Math.floor(Math.random() * 256))));
    expect(precompressDir(dir)).toEqual({ br: 0, gz: 0, raw: 0, saved: 0 });
    for (const name of ['assets/tiny.js', 'assets/font.woff2', 'icon.png', 'assets/noise.js']) {
      expect(existsSync(join(dir, `${name}.br`)), name).toBe(false);
      expect(existsSync(join(dir, `${name}.gz`)), name).toBe(false);
    }
  });

  it('is idempotent: reruns never compress siblings and drop stale ones', () => {
    writeFileSync(join(dir, 'assets/app.js'), big);
    writeFileSync(join(dir, 'assets/gone.js.br'), 'stale');
    precompressDir(dir);
    const again = precompressDir(dir);
    expect(again.br + again.gz).toBe(2);
    expect(existsSync(join(dir, 'assets/app.js.br.gz'))).toBe(false);
    expect(existsSync(join(dir, 'assets/gone.js.br'))).toBe(false);
  });
});
