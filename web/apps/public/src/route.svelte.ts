// The portal's address (public-portal-plan § Screens and routes): `/` the
// Week (`?week=next`, `?view=list`, `?run=<id>` the open run), `/mine` My runs
// (`?week=next`, `?week=timings`; `&hand=<timing>&to=<member>` back from a
// fresh sign-in reopens Hand to…), `/account` Account (`?tab=`), `/bosses`
// Bosses and `/bosses/{key}` a guide (`?difficulty=`, `?tab=`, `?phase=`).
// Pages change with `history.pushState`; filters and selections replace the entry.

export type Page = 'week' | 'mine' | 'account' | 'bosses';

import { SvelteURLSearchParams } from 'svelte/reactivity';

const PAGES: Record<string, Page> = { '/': 'week', '/mine': 'mine', '/account': 'account' };

export class Route {
  path = $state(location.pathname);
  params = $state(new SvelteURLSearchParams(location.search));
  readonly page: Page = $derived(PAGES[this.path] ?? (this.path === '/bosses' || this.path.startsWith('/bosses/') ? 'bosses' : 'week'));

  /** Back and Forward: the address is the state. */
  watch(): () => void {
    const pop = () => this.#read();
    addEventListener('popstate', pop);
    return () => removeEventListener('popstate', pop);
  }

  /**
   * Another page (a new history entry, or in place with `replace`); `params`
   * are its whole query; `state` tags the entry (a single-pane boss pick).
   */
  go(path: string, params: Record<string, string> = {}, { state = null, replace = false }: { state?: unknown; replace?: boolean } = {}): void {
    if (replace) history.replaceState(state, '', this.#href(path, params));
    else history.pushState(state, '', this.#href(path, params));
    this.#read();
  }

  /** Change some of this page's query in place: an empty value removes the key. */
  set(changes: Record<string, string>): void {
    const next = new SvelteURLSearchParams(this.params);
    for (const [key, value] of Object.entries(changes)) {
      if (value) next.set(key, value);
      else next.delete(key);
    }
    history.replaceState(history.state, '', this.#href(this.path, Object.fromEntries(next)));
    this.#read();
  }

  #href(path: string, params: Record<string, string>): string {
    const query = new SvelteURLSearchParams(params).toString();
    return query ? `${path}?${query}` : path;
  }

  #read(): void {
    this.path = location.pathname;
    this.params = new SvelteURLSearchParams(location.search);
  }
}
