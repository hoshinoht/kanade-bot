/**
 * History-API routing. Real paths (`/fixed`, `/inbox`) rather than `#/fixed`:
 * deep links and back/forward work, fragments stay free for in-page anchors,
 * the server already falls back to index.html for extensionless paths
 * and the service worker serves the shell for any navigation. CSP is
 * indifferent (no inline script, no eval either way).
 */
export class Router {
  path = $state('/');
  search = $state('');

  constructor() {
    this.#sync();
    window.addEventListener('popstate', () => this.#sync());
    document.addEventListener('click', this.#onClick);
  }

  #sync() {
    this.path = location.pathname;
    this.search = location.search;
  }

  get query(): URLSearchParams {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- read-only parse of the reactive `search`
    return new URLSearchParams(this.search);
  }

  go(href: string, { replace = false } = {}): void {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a one-off parse, not state
    const url = new URL(href, location.href);
    if (url.pathname === this.path && url.search === this.search) return;
    history[replace ? 'replaceState' : 'pushState'](null, '', url.pathname + url.search + url.hash);
    this.#sync();
  }

  /** Same-origin plain left-clicks on links become route changes. */
  #onClick = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target instanceof Element ? event.target.closest('a') : null;
    if (!link || link.target || link.hasAttribute('download') || link.origin !== location.origin) return;
    // In-page anchors (the skip link) stay native.
    if (link.hash && link.pathname === location.pathname && link.search === location.search) return;
    if (link.pathname.startsWith('/api/') || link.pathname.startsWith('/art/') || link.pathname.startsWith('/identity/')) return;
    event.preventDefault();
    this.go(link.pathname + link.search + link.hash);
  };
}

export interface Match {
  key: string;
  params: Record<string, string>;
}

/** `/bosses/:boss/knowledge` style patterns; first match wins. */
export function match(path: string, routes: { key: string; pattern: string }[]): Match | null {
  const parts = path.replace(/\/+$/, '').split('/').filter(Boolean);
  for (const route of routes) {
    const want = route.pattern.split('/').filter(Boolean);
    if (want.length !== parts.length) continue;
    const params: Record<string, string> = {};
    const ok = want.every((segment, i) => {
      const got = decodeURIComponent(parts[i]!);
      if (segment.startsWith(':')) {
        params[segment.slice(1)] = got;
        return true;
      }
      return segment === got;
    });
    if (ok) return { key: route.key, params };
  }
  return null;
}
