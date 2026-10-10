// A link inside the portal (Ask for a change…, Move this week…, Open the
// run) changes the page in place; a modified or middle click keeps the
// browser's own behaviour (new tab, new window). The entry is tagged
// `{ back: true }` so the page it opens can go back to where the press was.
import type { Route } from '../route.svelte';

/** The history state an in-portal link leaves on the entry it opens. */
export interface Followed {
  back: true;
}

export function follow(route: Route, event: MouseEvent, href: string): void {
  if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  const url = new URL(href, location.origin);
  route.go(url.pathname, Object.fromEntries(url.searchParams), { state: { back: true } satisfies Followed });
}

/** The page was opened by an in-portal link, so Back returns to the press. */
export function followed(state: unknown): boolean {
  return typeof state === 'object' && state !== null && 'back' in state && state.back === true;
}
