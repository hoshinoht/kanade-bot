import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { initAsyncCompiler, type AsyncCompiler } from 'sass-embedded';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

// The style split (workplan step workspace/ui-core-tidy): `styles/core/` holds
// the generic primitives, `styles/screens/` the Kanade screens built on them.
// A screen consumes core through the cascade and the helper modules only, and
// never redefines a core selector: a primitive is fixed once, in core.
const STYLES = join(import.meta.dirname, '../src/styles');
const CORE = join(STYLES, 'core');
const SCREENS = join(STYLES, 'screens');
// Core modules with no CSS output of their own (variables, mixins, queries).
const HELPERS = ['breakpoints', 'easing', 'motion-query', 'mixins'];

const partials = (dir: string) => readdirSync(dir).filter((f) => /^_.*\.scss$/.test(f));
const uses = (file: string) => [...readFileSync(file, 'utf8').matchAll(/^@use "([^"]+)"/gm)].map((m) => m[1]!);

/** Every complex selector of a stylesheet's style rules, keyframe steps left out. */
function selectors(css: string): string[] {
  const out: string[] = [];
  const open: string[] = [];
  let text = '';
  let parens = 0;
  for (const ch of css.replace(/\/\*[\s\S]*?\*\//g, '')) {
    if (ch === '(') parens++;
    else if (ch === ')') parens--;
    if (parens === 0 && ch === '{') {
      const head = text.trim();
      if (!head.startsWith('@') && !open.some((h) => h.startsWith('@keyframes')))
        for (const s of head.split(/,(?![^(]*\))/)) out.push(s.trim().replace(/\s+/g, ' '));
      open.push(head);
      text = '';
    } else if (parens === 0 && ch === '}') {
      open.pop();
      text = '';
    } else if (parens === 0 && ch === ';') text = '';
    else text += ch;
  }
  return out;
}

let sass: AsyncCompiler;
beforeAll(async () => {
  sass = await initAsyncCompiler();
});
afterAll(async () => {
  await sass.dispose();
});

describe('styles split', () => {
  it('keeps core free of screens', () => {
    for (const f of partials(CORE)) expect(uses(join(CORE, f)).filter((u) => u.includes('screens')), f).toEqual([]);
  });

  it('lets screens use only the core helpers and their own parts', () => {
    for (const f of partials(SCREENS)) {
      const core = uses(join(SCREENS, f)).filter((u) => u.startsWith('../core/'));
      expect(core.filter((u) => !HELPERS.includes(u.slice('../core/'.length))), f).toEqual([]);
    }
  });

  it('never redefines a core selector in a screen partial', async () => {
    const core = new Set<string>();
    for (const f of partials(CORE)) for (const s of selectors((await sass.compileAsync(join(CORE, f))).css)) core.add(s);
    const redefined: string[] = [];
    for (const f of partials(SCREENS))
      for (const s of new Set(selectors((await sass.compileAsync(join(SCREENS, f))).css))) if (core.has(s)) redefined.push(`${f}: ${s}`);
    expect(core.size).toBeGreaterThan(500);
    expect(redefined).toEqual([]);
  });
});
