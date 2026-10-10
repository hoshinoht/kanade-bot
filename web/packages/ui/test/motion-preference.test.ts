import { readdirSync, readFileSync } from 'node:fs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Toaster } from '../src/components/toaster.svelte';
import { reducedMotion } from '../src/motion/easing';
import { enter } from '../src/motion/enter';
import { flip } from '../src/motion/flip';
import { MOTION_KEY, motionPreference } from '../src/motion/preference.svelte';
import { Presence } from '../src/motion/presence.svelte';

const read = (p: string) => readFileSync(new URL(p, import.meta.url), 'utf8');

let os = false;
let root: { dataset: Record<string, string>; style: { setProperty: () => void } };
let store: Map<string, string>;

beforeEach(() => {
  os = false;
  root = { dataset: {}, style: { setProperty: () => {} } };
  store = new Map();
  vi.stubGlobal('document', { documentElement: root });
  vi.stubGlobal('matchMedia', (query: string) => ({ matches: query.includes('reduce') && os }));
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
});
afterEach(() => {
  motionPreference.reduce = false;
  vi.unstubAllGlobals();
});

describe('Reduce motion switch', () => {
  it('off follows the device for both settings; on reduces with the device at no-preference', () => {
    motionPreference.set(false);
    os = false;
    expect(reducedMotion()).toBe(false);
    os = true;
    expect(reducedMotion()).toBe(true);
    os = false;
    motionPreference.set(true);
    expect(reducedMotion()).toBe(true);
    expect(motionPreference.reduced).toBe(true);
  });

  it('is kept in this browser and reflected on <html> at once; off forgets it', () => {
    motionPreference.set(true);
    expect([root.dataset.motion, store.get(MOTION_KEY)]).toEqual(['reduce', 'reduce']);
    motionPreference.set(false);
    expect([root.dataset.motion, store.has(MOTION_KEY)]).toEqual([undefined, false]);
  });

  it('survives blocked storage for this page', () => {
    vi.stubGlobal('localStorage', {
      setItem: () => {
        throw new Error('blocked');
      },
      removeItem: () => {
        throw new Error('blocked');
      },
    });
    motionPreference.set(true);
    expect([motionPreference.reduce, root.dataset.motion]).toEqual([true, 'reduce']);
  });

  it('the scripted motion helpers treat the switch as the device setting', async () => {
    motionPreference.set(true);
    const pane = new Presence('a');
    pane.set(null);
    expect(pane.shown).toBeNull();
    const node = { animate: vi.fn() } as unknown as HTMLElement & { animate: ReturnType<typeof vi.fn> };
    enter('k')(node);
    expect(node.animate).not.toHaveBeenCalled();
    const toaster = new Toaster();
    toaster.dismiss(toaster.show({ message: 'x' }));
    expect(toaster.shown).toEqual([]);
    // A bare object would throw if flip measured it.
    expect(await flip({} as HTMLElement, { selector: '*', key: () => 'x' })).toEqual([]);
  });

  it('theme-boot applies a stored choice before first paint, and nothing else', () => {
    const boot = read('../src/theme/theme-boot.js');
    new Function(boot)();
    expect(root.dataset.motion).toBeUndefined();
    store.set(MOTION_KEY, 'reduce');
    new Function(boot)();
    expect(root.dataset.motion).toBe('reduce');
    root.dataset = {};
    store.set(MOTION_KEY, 'junk');
    new Function(boot)();
    expect(root.dataset.motion).toBeUndefined();
  });
});

describe('Reduce motion in the stylesheets', () => {
  it('every reduced-motion query also answers to the switch', () => {
    const dir = new URL('../src/styles/', import.meta.url);
    const partials = readdirSync(dir).filter((name) => name.endsWith('.scss') && name !== '_motion-query.scss');
    expect(partials.length).toBeGreaterThan(20);
    for (const name of partials) {
      expect(readFileSync(new URL(name, dir), 'utf8'), name).not.toMatch(/prefers-reduced-motion/);
    }
    // NapArt keeps its own (CSP-extracted) copy of the nap's style: every animated rule waits for the switch.
    const nap = /@media \(prefers-reduced-motion: no-preference\)\{\n([\s\S]*?)\n\}/.exec(read('../src/components/NapArt.svelte'))![1]!;
    for (const rule of nap.split('}').filter((r) => r.trim())) expect(rule.trim()).toMatch(/^:where\(:root:not\(\[data-motion=reduce\]\)\) /);
  });
});
