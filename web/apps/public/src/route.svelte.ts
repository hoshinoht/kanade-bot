// The portal's address (public-portal-plan § Screens and routes): `/` the
// Week (`?week=next`, `?view=list`, `?run=<id>` the open run), `/mine` My runs
// (`?week=next`), `/account` Account (`?tab=`). Pages change with
// `history.pushState`; filters and selections replace the entry.

export type Page = 'week' | 'mine' | 'account';

import { SvelteURLSearchParams } from 'svelte/reactivity';

const PAGES: Record<string, Page> = { '/': 'week', '/mine': 'mine', '/account': 'account' };

export class Route {
  path = $state(location.pathname);
  params = $state(new SvelteURLSearchParams(location.search));
  readonly page: Page = $derived(PAGES[this.path] ?? 'week');

  /** Back and Forward: the address is the state. */
  watch(): () => void {
    const pop = () => this.#read();
    addEventListener('popstate', pop);
    return () => removeEventListener('popstate', pop);
  }

  /** Another page (a new history entry); `params` are its whole query. */
  go(path: string, params: Record<string, string> = {}): void {
    history.pushState(null, '', this.#href(path, params));
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
