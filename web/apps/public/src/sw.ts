// Public service worker: precaches the static build only. Navigations go to
// the network first; offline they get the precached offline page. `/api/`
// (status, session, devices, sign-in, the member's week and allowance) and
// `/art/` (served only behind the member session) are never intercepted, so
// no member data, art or session answer is ever stored by the worker.
import { cleanupOutdatedCaches, matchPrecache, precache, type PrecacheEntry } from 'workbox-precaching';

declare global {
  interface WorkerGlobalScope {
    /** Replaced at build time by vite-plugin-pwa (workbox injectManifest). */
    __WB_MANIFEST: (PrecacheEntry | string)[];
  }
}

const sw = self as unknown as ServiceWorkerGlobalScope;
const OFFLINE_PAGE = 'offline.html';

precache(self.__WB_MANIFEST);
cleanupOutdatedCaches();

sw.addEventListener('message', (event) => {
  if ((event.data as { type?: string } | null)?.type === 'SKIP_WAITING') void sw.skipWaiting();
});

// Member reads and boss art go straight to the network: both sit behind the
// member session, so a stored copy would outlive sign-out. Animated art also
// needs the server's 206 replies to Range requests (Safari playback).
function bypass(url: URL): boolean {
  return (
    url.origin !== sw.location.origin ||
    url.pathname.startsWith('/api/') ||
    url.pathname.startsWith('/__mock/') ||
    url.pathname.startsWith('/art/') ||
    url.pathname === '/csp-report'
  );
}

sw.addEventListener('fetch', (event) => {
  const { request } = event;
  const url = new URL(request.url);
  if (request.method !== 'GET' || bypass(url)) return;

  if (request.mode === 'navigate') {
    event.respondWith(fetch(request).catch(async () => (await matchPrecache(OFFLINE_PAGE)) ?? Response.error()));
    return;
  }
  event.respondWith(matchPrecache(request.url).then((cached) => cached ?? fetch(request)));
});
