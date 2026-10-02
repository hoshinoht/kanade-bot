import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { test as base, expect } from '@playwright/test';

// Pure file checks on the built output; no browser, no server.
const test = base;
const dist = (app: string) => join(import.meta.dirname, '..', 'apps', app, 'dist');

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

const text = (app: string) =>
  files(dist(app))
    .filter((f) => /\.(js|css|html|webmanifest)$/.test(f))
    .map((f) => ({ f, body: readFileSync(f, 'utf8') }));

// Strings that only admin code contains: its API, its libraries and its components.
const ADMIN_ONLY = [
  '/api/admin',
  'dnd-kit',
  'data-dnd',
  'beforedragstart',
  'uplot',
  'u-legend',
  'plan-card',
  'Picked up',
  'Command palette',
  'palette__',
  'Undo last move',
  'Kanade Admin',
];

test('public bundle contains no admin module strings', () => {
  const leaks = text('public').flatMap(({ f, body }) => ADMIN_ONLY.filter((s) => body.includes(s)).map((s) => `${f}: ${s}`));
  expect(leaks).toEqual([]);
});

test('admin bundle contains every marker (each one can catch a leak)', () => {
  const all = text('admin').map((x) => x.body).join('\n');
  expect(ADMIN_ONLY.filter((s) => !all.includes(s))).toEqual([]);
});

// `stripFidelityTags` removes them unless KANADE_FIDELITY=1; `bun run fidelity`
// rebuilds clean afterwards. A tagged build must never be what ships.
test('no bundle keeps the layout-fidelity tags', () => {
  for (const app of ['public', 'admin']) {
    expect(text(app).filter(({ body }) => body.includes('data-fid')).map(({ f }) => f)).toEqual([]);
  }
});

test('no bundle registers a pass-through Trusted Types policy or writes HTML strings', () => {
  for (const app of ['public', 'admin']) {
    const all = text(app).map((x) => x.body).join('\n');
    expect(all).not.toContain('svelte-trusted-html');
    expect(all).not.toMatch(/\.innerHTML\s*=|insertAdjacentHTML|document\.write/);
    expect(all).not.toMatch(/url\(\s*["']?data:font/);
  }
});
