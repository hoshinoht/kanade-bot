// Admin service worker: precaches the static build only. Navigations go to
// the network first; offline they get the precached app shell, which renders
// its own offline state. The API is never intercepted, so private data and
// credentials never reach a cache.
import { cleanupOutdatedCaches, matchPrecache, precache, type PrecacheEntry } from 'workbox-precaching';

declare global {
  interface WorkerGlobalScope {
    /** Replaced at build time by vite-plugin-pwa (workbox injectManifest). */
    __WB_MANIFEST: (PrecacheEntry | string)[];
  }
}

const sw = self as unknown as ServiceWorkerGlobalScope;
const SHELL = 'index.html';

precache(self.__WB_MANIFEST);
cleanupOutdatedCaches();

sw.addEventListener('message', (event) => {
  if ((event.data as { type?: string } | null)?.type === 'SKIP_WAITING') void sw.skipWaiting();
});

// Animated art goes straight to the network: its Range requests need the
// server's 206 replies, and a worker answering or caching them breaks Safari playback.
function bypass(url: URL): boolean {
  return (
    url.origin !== sw.location.origin ||
    url.pathname.startsWith('/api/') ||
    url.pathname.startsWith('/__mock/') ||
    url.pathname.startsWith('/art/animated/') ||
    url.pathname === '/csp-report'
  );
}

sw.addEventListener('fetch', (event) => {
  const { request } = event;
  const url = new URL(request.url);
  if (request.method !== 'GET' || bypass(url)) return;

  if (request.mode === 'navigate') {
    event.respondWith(fetch(request).catch(async () => (await matchPrecache(SHELL)) ?? Response.error()));
    return;
  }
  event.respondWith(matchPrecache(request.url).then((cached) => cached ?? fetch(request)));
});
