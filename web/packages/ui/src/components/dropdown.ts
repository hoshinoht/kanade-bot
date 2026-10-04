// Shared popover plumbing for Select and MultiSelect: placement through CSSOM
// custom properties (strict CSP, as Portrait.svelte), and a press-away hook.

const GAP = 6;
const EDGE = 8;
const TALLEST = 440;

/** Under the trigger (flipped above when that side has more room), held inside the viewport. */
export function placePopover(trigger: HTMLElement, pop: HTMLElement, align: 'start' | 'end') {
  const r = trigger.getBoundingClientRect();
  pop.style.setProperty('--dd-min', `${Math.round(r.width)}px`);
  const below = innerHeight - r.bottom - GAP - EDGE;
  const above = r.top - GAP - EDGE;
  pop.style.setProperty('--dd-max', `${TALLEST}px`);
  const natural = pop.offsetHeight;
  const down = below >= natural || below >= above;
  const room = Math.max(120, Math.min(TALLEST, down ? below : above));
  pop.style.setProperty('--dd-max', `${room}px`);
  const h = pop.offsetHeight;
  const w = pop.offsetWidth;
  const left = align === 'end' ? r.right - w : r.left;
  pop.style.setProperty('--dd-left', `${Math.round(Math.max(EDGE, Math.min(left, innerWidth - w - EDGE)))}px`);
  pop.style.setProperty('--dd-top', `${Math.round(down ? r.bottom + GAP : Math.max(EDGE, r.top - GAP - h))}px`);
  pop.classList.toggle('dd-pop--up', !down);
}

/** Calls `away` on a press outside every node; returns the cleanup. */
export function pressAway(nodes: () => (Node | undefined)[], away: () => void): () => void {
  const onDown = (event: PointerEvent) => {
    const target = event.target as Node;
    if (!nodes().some((n) => n?.contains(target))) away();
  };
  document.addEventListener('pointerdown', onDown, true);
  return () => document.removeEventListener('pointerdown', onDown, true);
}

/** Keeps a placed popover with its trigger while the page scrolls or resizes. */
export function follow(pop: () => HTMLElement | undefined, again: () => void): () => void {
  const onScroll = (event: Event) => {
    if (!pop()?.contains(event.target as Node)) again();
  };
  addEventListener('resize', again);
  addEventListener('scroll', onScroll, true);
  return () => {
    removeEventListener('resize', again);
    removeEventListener('scroll', onScroll, true);
  };
}

/** Whether the native-picker frame applies now. */
export function watchMedia(query: string, on: (matches: boolean) => void): () => void {
  if (typeof matchMedia !== 'function') return () => {};
  const list = matchMedia(query);
  on(list.matches);
  const change = (e: MediaQueryListEvent) => on(e.matches);
  list.addEventListener('change', change);
  return () => list.removeEventListener('change', change);
}
