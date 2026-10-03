import { tick } from 'svelte';
import { SPRING, SPRING_MS, reducedMotion } from './easing';

export interface Point {
  x: number;
  y: number;
}

/** Less than this is layout rounding, not a move. */
const MIN_PX = 1;

/** Where each keyed element sits now (viewport coordinates of its top-left corner). */
export function measure(root: ParentNode, selector: string, key: (el: HTMLElement) => string | undefined): Map<string, Point> {
  const at = new Map<string, Point>();
  for (const el of root.querySelectorAll<HTMLElement>(selector)) {
    const id = key(el);
    if (!id) continue;
    const box = el.getBoundingClientRect();
    at.set(id, { x: box.left, y: box.top });
  }
  return at;
}

/** How far each element that is still present moved (old minus new, the FLIP "invert"). */
export function deltas(before: Map<string, Point>, after: Map<string, Point>): Map<string, Point> {
  const moved = new Map<string, Point>();
  for (const [id, now] of after) {
    const then = before.get(id);
    if (!then) continue;
    const dx = then.x - now.x;
    const dy = then.y - now.y;
    if (Math.abs(dx) >= MIN_PX || Math.abs(dy) >= MIN_PX) moved.set(id, { x: dx, y: dy });
  }
  return moved;
}

export interface FlipOptions {
  selector: string;
  key: (el: HTMLElement) => string | undefined;
  duration?: number;
  easing?: string;
}

/**
 * FLIP for keyed elements under `root`: call before a change is applied; the
 * returned promise resolves once the moved elements started animating from
 * their old place to the new one (Web Animations, transform only). Elements
 * that are new or gone are left alone. Reduced motion does nothing.
 */
export function flip(root: ParentNode | null, { selector, key, duration = SPRING_MS, easing = SPRING }: FlipOptions): Promise<Animation[]> {
  if (!root || reducedMotion()) return Promise.resolve([]);
  const before = measure(root, selector, key);
  return tick().then(() => {
    const els = new Map<string, HTMLElement>();
    for (const el of root.querySelectorAll<HTMLElement>(selector)) {
      const id = key(el);
      if (id) els.set(id, el);
    }
    const after = new Map<string, Point>();
    for (const [id, el] of els) {
      const box = el.getBoundingClientRect();
      after.set(id, { x: box.left, y: box.top });
    }
    const started: Animation[] = [];
    for (const [id, d] of deltas(before, after)) {
      const el = els.get(id)!;
      if (typeof el.animate !== 'function') continue;
      // A card remounted in another day column would also play its mount
      // fade; the move replaces it.
      if (typeof CSSAnimation !== 'undefined') for (const running of el.getAnimations()) if (running instanceof CSSAnimation) running.finish();
      started.push(el.animate([{ transform: `translate(${d.x}px, ${d.y}px)` }, { transform: 'none' }], { duration, easing }));
    }
    return started;
  });
}
