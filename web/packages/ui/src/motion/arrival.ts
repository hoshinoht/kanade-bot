// Arrival motion for data that came from elsewhere (a live hint, a poll):
// a one-off `data-new` mark on rows that just appeared and a short `data-tick`
// pulse on numbers that just changed. Both are attributes that class-driven
// `@keyframes` in `_arrival.scss` play once (CSP-safe: no style attributes);
// reduced motion keeps only a colour fade there. Callers decide what counts as
// an arrival: the admin's own writes never mark or pulse.
import type { Attachment } from 'svelte/attachments';

/** Longer than either animation, in case `animationend` never fires (hidden tab, display: none). */
export const ARRIVAL_FALLBACK_MS = 2_000;

/** The keyframes each mark plays (`_arrival.scss`); the mark stays until all of them ended. */
const KEYFRAMES: Record<'data-new' | 'data-tick', ReadonlySet<string>> = {
  'data-new': new Set(['arrive-settle', 'arrive-tint']),
  'data-tick': new Set(['tick-colour', 'tick-scale']),
};

/** The pending clear for each element and mark. */
const pending = new WeakMap<Element, Map<string, () => void>>();

/** Whether `el` still plays one of the mark's own keyframes. */
function playing(el: Element, names: ReadonlySet<string>): boolean {
  if (typeof el.getAnimations !== 'function' || typeof CSSAnimation === 'undefined') return false;
  return el.getAnimations().some((a) => a instanceof CSSAnimation && names.has(a.animationName) && a.playState !== 'finished');
}

/**
 * Set `name` on `el` so its animation plays from the start, and clear it once
 * every keyframe it plays on `el` itself has ended (a short settle ends before
 * the tint; an inner element's animation bubbling up is ignored), or after
 * `ARRIVAL_FALLBACK_MS` whatever happens.
 */
export function replay(el: Element | null | undefined, name: 'data-new' | 'data-tick'): void {
  if (!el) return;
  const names = KEYFRAMES[name];
  const marks = pending.get(el) ?? new Map<string, () => void>();
  pending.set(el, marks);
  marks.get(name)?.();
  const ended = (event: Event) => {
    if (event.target !== el || !names.has((event as AnimationEvent).animationName)) return;
    if (!playing(el, names)) clear();
  };
  const timer = setTimeout(() => clear(), ARRIVAL_FALLBACK_MS);
  const clear = () => {
    el.removeAttribute(name);
    el.removeEventListener('animationend', ended);
    clearTimeout(timer);
    if (marks.get(name) === clear) marks.delete(name);
  };
  marks.set(name, clear);
  // A reflow between removing and setting restarts a running animation.
  void (el as HTMLElement).offsetWidth;
  el.setAttribute(name, '');
  el.addEventListener('animationend', ended);
}

const seen = new WeakMap<Element, { value: unknown; arrival: number }>();

/**
 * `{@attach pulse(count, arrival.seq)}`: the element pulses when `value`
 * changed together with an arrival (`arrival` moved too). A change the user
 * made (no arrival) or the first render stays still.
 */
export function pulse(value: unknown, arrival: number): Attachment {
  return (el) => {
    const was = seen.get(el);
    seen.set(el, { value, arrival });
    if (was && was.value !== value && was.arrival !== arrival) replay(el, 'data-tick');
  };
}
