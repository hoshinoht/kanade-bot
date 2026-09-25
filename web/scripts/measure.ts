// Bundle sizes per app: initial (what index.html pulls in) vs total, gzip -9
// for JS/CSS/HTML; fonts reported raw (woff2 is already compressed).
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { gzipSync } from 'node:zlib';

const kb = (n: number) => (n / 1024).toFixed(1);
const gz = (path: string) => gzipSync(readFileSync(path), { level: 9 }).length;

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((n) => (statSync(join(dir, n)).isDirectory() ? walk(join(dir, n)) : [join(dir, n)]));
}

for (const app of ['public', 'admin']) {
  const dist = join(import.meta.dir, '..', 'apps', app, 'dist');
  const html = readFileSync(join(dist, 'index.html'), 'utf8');
  const initial = [...html.matchAll(/(?:src|href)="\/(assets\/[^"]+\.(?:js|css))"/g)].map((m) => join(dist, m[1]!));
  const files = walk(dist);
  const sum = (list: string[], ext: string, fn: (p: string) => number) => list.filter((f) => f.endsWith(ext)).reduce((a, f) => a + fn(f), 0);
  const woff2 = files.filter((f) => f.endsWith('.woff2'));
  console.log(`\n${app}`);
  console.log(`  initial JS  ${kb(sum(initial, '.js', gz))} KB gz (${initial.filter((f) => f.endsWith('.js')).map((f) => relative(dist, f)).join(', ')})`);
  console.log(`  initial CSS ${kb(sum(initial, '.css', gz))} KB gz`);
  console.log(`  index.html  ${kb(gz(join(dist, 'index.html')))} KB gz`);
  console.log(`  total JS    ${kb(sum(files, '.js', gz))} KB gz incl. sw.js ${kb(gz(join(dist, 'sw.js')))} KB`);
  console.log(`  total CSS   ${kb(sum(files, '.css', gz))} KB gz`);
  console.log(`  fonts woff2 ${kb(woff2.reduce((a, f) => a + statSync(f).size, 0))} KB raw across ${woff2.length} files (woff fallbacks emitted, never fetched or precached)`);
}
