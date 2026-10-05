import { render } from 'svelte/server';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import LoadingState from '../src/components/LoadingState.svelte';
import { TOAST_ACTION_MS, TOAST_MS, Toaster } from '../src/components/toaster.svelte';
import { experiments } from '../src/experiments/experiments.svelte';
import { Delay, LOADING_DELAY_MS } from '../src/motion/delay.svelte';
import { SPRING, SPRING_MS } from '../src/motion/easing';
import { enter, enterFrames } from '../src/motion/enter';
import { deltas, flip, measure } from '../src/motion/flip';
import { EXIT_FALLBACK_MS, Presence } from '../src/motion/presence.svelte';

let reduce = false;
beforeEach(() => {
  reduce = false;
  vi.useFakeTimers();
  vi.stubGlobal('matchMedia', (query: string) => ({ matches: query.includes('reduce') && reduce }));
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  experiments.on = true;
});

describe('Presence', () => {
  it('keeps the value while it leaves, then drops it on its own animationend', () => {
    const pane = new Presence<string>();
    pane.set('a');
    expect([pane.shown, pane.leaving]).toEqual(['a', false]);
    pane.set(null);
    expect([pane.shown, pane.leaving]).toEqual(['a', true]);
    // A child's animation ending is not the pane's exit.
    const node = {};
    pane.done({ target: {}, currentTarget: node } as unknown as Event);
    expect(pane.shown).toBe('a');
    pane.done({ target: node, currentTarget: node } as unknown as Event);
    expect([pane.shown, pane.leaving]).toEqual([null, false]);
  });

  it('falls back to a timeout when no animationend arrives', () => {
    const pane = new Presence('a');
    pane.set(null);
    vi.advanceTimersByTime(EXIT_FALLBACK_MS - 1);
    expect(pane.shown).toBe('a');
    vi.advanceTimersByTime(1);
    expect(pane.shown).toBeNull();
  });

  it('a new value during the exit cancels it (the fallback must not drop the new one)', () => {
    const pane = new Presence('a');
    pane.set(null);
    pane.set('b');
    expect([pane.shown, pane.leaving]).toEqual(['b', false]);
    vi.advanceTimersByTime(EXIT_FALLBACK_MS * 2);
    expect(pane.shown).toBe('b');
  });

  it('reduced motion removes at once', () => {
    reduce = true;
    const pane = new Presence('a');
    pane.set(null);
    expect([pane.shown, pane.leaving]).toEqual([null, false]);
  });
});

describe('Delay (the 200 ms loading standard)', () => {
  it('is due only after the delay, and stop resets it', () => {
    const delay = new Delay();
    delay.start();
    vi.advanceTimersByTime(LOADING_DELAY_MS - 1);
    expect(delay.due).toBe(false);
    vi.advanceTimersByTime(1);
    expect(delay.due).toBe(true);
    delay.stop();
    expect(delay.due).toBe(false);
  });

  it('a load that ends first never shows', () => {
    const delay = new Delay();
    delay.start();
    vi.advanceTimersByTime(150);
    delay.stop();
    vi.advanceTimersByTime(LOADING_DELAY_MS);
    expect(delay.due).toBe(false);
  });

  it('LoadingState renders its status region empty before the delay', () => {
    const out = render(LoadingState, { props: { text: 'Loading interactions…' } }).body;
    expect(out).toMatch(/<p class="loading-state__body" role="status">\s*(<!--[^>]*-->\s*)*<\/p>/);
    expect(out).not.toContain('aria-busy');
    expect(out).not.toContain('Loading interactions…');
  });
});

/** A fake element at a position that the test can move. */
function box(id: string, x: number, y: number) {
  const animate = vi.fn((frames: Keyframe[], options: KeyframeAnimationOptions) => ({ frames, options, cancel: vi.fn() }));
  const el = {
    dataset: { run: id },
    at: { x, y },
    getBoundingClientRect() {
      return { left: this.at.x, top: this.at.y } as DOMRect;
    },
    getAnimations: () => [],
    animate,
  };
  return el;
}

function root(els: ReturnType<typeof box>[]) {
  return { querySelectorAll: () => els } as unknown as ParentNode;
}

const key = (el: HTMLElement) => el.dataset.run;

describe('FLIP', () => {
  it('deltas: old minus new for elements still present; sub-pixel and new ones are left alone', () => {
    const before = new Map([
      ['a', { x: 0, y: 100 }],
      ['b', { x: 10, y: 10 }],
      ['gone', { x: 0, y: 0 }],
    ]);
    const after = new Map([
      ['a', { x: 200, y: 40 }],
      ['b', { x: 10.4, y: 10 }],
      ['new', { x: 5, y: 5 }],
    ]);
    expect([...deltas(before, after)]).toEqual([['a', { x: -200, y: 60 }]]);
  });

  it('measure keys elements by the given attribute', () => {
    const els = [box('a', 1, 2), box('', 3, 4)];
    expect([...measure(root(els), '[data-run]', key)]).toEqual([['a', { x: 1, y: 2 }]]);
  });

  it('animates each moved card from its old place to the new one, transform only', async () => {
    const a = box('a', 0, 100);
    const b = box('b', 0, 200);
    const pending = flip(root([a, b]), { selector: '[data-run]', key });
    // The change lands: a moves to another day column, b stays.
    a.at = { x: 300, y: 50 };
    const started = await pending;
    expect(started).toHaveLength(1);
    expect(b.animate).not.toHaveBeenCalled();
    expect(a.animate).toHaveBeenCalledWith([{ transform: 'translate(-300px, 50px)' }, { transform: 'none' }], { duration: SPRING_MS, easing: SPRING });
  });

  it('reduced motion moves nothing', async () => {
    reduce = true;
    const a = box('a', 0, 0);
    const pending = flip(root([a]), { selector: '[data-run]', key });
    a.at = { x: 100, y: 0 };
    expect(await pending).toEqual([]);
    expect(a.animate).not.toHaveBeenCalled();
  });
});

describe('easing', () => {
  it('the script spring is the stylesheet spring (samples and duration)', async () => {
    const { readFileSync } = await import('node:fs');
    const scss = readFileSync(new URL('../src/styles/_easing.scss', import.meta.url), 'utf8');
    const samples = (text: string) => /linear\(([^)]*)\)/.exec(text)![1]!.replace(/\s+/g, '');
    expect(samples(scss)).toBe(samples(SPRING));
    expect(/\$spring-ms:\s*(\d+)ms/.exec(scss)![1]).toBe(String(SPRING_MS));
  });
});

describe('enter (pane forward/backward)', () => {
  it('forward comes from the end, backward from the start; opacity and transform only', () => {
    expect(enterFrames('forward')[0]).toEqual({ opacity: 0, transform: 'translateX(24px)' });
    expect(enterFrames('backward')[0]).toEqual({ opacity: 0, transform: 'translateX(-24px)' });
  });

  it('plays for a key, nothing for null or under reduced motion; teardown cancels', () => {
    const node = box('x', 0, 0) as unknown as HTMLElement & { animate: ReturnType<typeof vi.fn> };
    expect(enter(null)(node)).toBeUndefined();
    expect(node.animate).not.toHaveBeenCalled();
    const stop = enter('m1')(node) as () => void;
    expect(node.animate).toHaveBeenCalledWith(enterFrames('forward'), { duration: 250, easing: 'cubic-bezier(0.2, 0, 0, 1)' });
    stop();
    expect(node.animate.mock.results[0]!.value.cancel).toHaveBeenCalled();
    reduce = true;
    node.animate.mockClear();
    enter('m2')(node);
    expect(node.animate).not.toHaveBeenCalled();
  });
});

describe('Toaster timing', () => {
  const timeout = (toast: Parameters<Toaster['show']>[0]) => {
    const toaster = new Toaster();
    const id = toaster.show(toast);
    return toaster.items.find((t) => t.id === id)?.timeoutMs;
  };
  const undo = { label: 'Undo', run: () => {} };

  it('success and info hide after 6 s, 10 s with an action; errors stay', () => {
    expect(timeout({ message: 'Saved.', tone: 'ok' })).toBe(TOAST_MS);
    expect(timeout({ message: 'Note.' })).toBe(TOAST_MS);
    expect(timeout({ message: 'Moved HFA.', tone: 'ok', action: undo })).toBe(TOAST_ACTION_MS);
    expect(timeout({ message: 'Failed.', tone: 'error' })).toBeNull();
    expect(timeout({ message: 'Failed.', tone: 'error', action: undo })).toBeNull();
    expect([TOAST_MS, TOAST_ACTION_MS]).toEqual([6000, 10_000]);
  });

  it('over the cap a timed toast goes first, so errors and Reload prompts stay', () => {
    const toaster = new Toaster();
    const error = toaster.show({ message: 'Failed.', tone: 'error' });
    const saved = toaster.show({ message: 'Saved.', tone: 'ok' });
    const moved = toaster.show({ message: 'Moved.', tone: 'ok' });
    expect(toaster.items.map((t) => t.id)).toEqual([error, moved]);
    expect(toaster.leaving.map((t) => t.id)).toEqual([saved]);
    const reload = toaster.show({ message: 'New version.', timeoutMs: null, action: { label: 'Reload', run: () => {} } });
    expect(toaster.items.map((t) => t.id)).toEqual([error, reload]);
  });

  it('when every older toast is persistent, the oldest goes', () => {
    const toaster = new Toaster();
    const first = toaster.show({ message: 'Failed.', tone: 'error' });
    const second = toaster.show({ message: 'Failed again.', tone: 'error' });
    const third = toaster.show({ message: 'Saved.', tone: 'ok' });
    expect(toaster.items.map((t) => t.id)).toEqual([second, third]);
    expect(toaster.leaving.map((t) => t.id)).toEqual([first]);
  });

  it('an explicit timeout wins, including null', () => {
    expect(timeout({ message: 'Re-reading…', timeoutMs: null })).toBeNull();
    expect(timeout({ message: 'x', tone: 'error', timeoutMs: 3000 })).toBe(3000);
  });
});

describe('Toaster exit', () => {
  it('a dismissed toast leaves the live list at once but stays shown until its exit ends', () => {
    const toaster = new Toaster();
    const first = toaster.show({ message: 'Moved HFA.' });
    const second = toaster.show({ message: 'Saved.' });
    toaster.dismiss(first);
    expect(toaster.items.map((t) => t.id)).toEqual([second]);
    expect(toaster.shown.map((t) => t.id)).toEqual([first, second]);
    toaster.gone(first);
    expect(toaster.shown.map((t) => t.id)).toEqual([second]);
  });

  it('the fallback removes it, and the third toast pushes the oldest out through its exit', () => {
    const toaster = new Toaster();
    const ids = [1, 2, 3].map((n) => toaster.show({ message: `t${n}` }));
    expect(toaster.items.map((t) => t.id)).toEqual(ids.slice(1));
    expect(toaster.leaving.map((t) => t.id)).toEqual([ids[0]]);
    vi.advanceTimersByTime(EXIT_FALLBACK_MS);
    expect(toaster.shown.map((t) => t.id)).toEqual(ids.slice(1));
  });

  it('reduced motion drops it at once', () => {
    reduce = true;
    const toaster = new Toaster();
    toaster.dismiss(toaster.show({ message: 'x' }));
    expect(toaster.shown).toEqual([]);
  });
});
