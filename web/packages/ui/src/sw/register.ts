// Service-worker registration shared by both apps. Progressive enhancement:
// every path works without it (blocked, unsupported, or `?sw=off`), because
// API data is never served from the worker.

interface TrustedTypePolicyLike {
  createScriptURL(input: string): unknown;
}

interface TrustedTypesLike {
  createPolicy(name: string, rules: { createScriptURL: (input: string) => string }): TrustedTypePolicyLike;
}

export interface RegisterOptions {
  url: string;
  /** A new worker is installed and waiting; `apply` activates it and reloads. */
  onUpdateReady: (apply: () => void) => void;
  onOfflineReady?: () => void;
  /**
   * A new worker took control without this page asking (another tab accepted
   * the update). Pages are not reloaded behind the reader's back: unsaved
   * input would be lost, so the app is expected to offer a reload instead.
   */
  onControllerChange?: () => void;
}

const UPDATE_CHECK_MS = 60 * 60 * 1000;
let policy: TrustedTypePolicyLike | null | undefined;

/** `navigator.serviceWorker.register` is a Trusted Types script-URL sink. */
function scriptURL(url: string): string {
  if (policy === undefined) {
    const tt = (globalThis as { trustedTypes?: TrustedTypesLike }).trustedTypes;
    policy = tt
      ? tt.createPolicy('kanade-sw', {
          createScriptURL: (input) => {
            if (input !== url) throw new TypeError(`Refusing service worker URL ${input}`);
            return input;
          },
        })
      : null;
  }
  return (policy ? policy.createScriptURL(url) : url) as string;
}

export function serviceWorkerDisabled(): boolean {
  return !('serviceWorker' in navigator) || new URLSearchParams(location.search).get('sw') === 'off';
}

export async function registerServiceWorker(options: RegisterOptions): Promise<ServiceWorkerRegistration | null> {
  if (!('serviceWorker' in navigator)) return null;
  if (serviceWorkerDisabled()) {
    const existing = await navigator.serviceWorker.getRegistrations();
    await Promise.all(existing.map((r) => r.unregister()));
    return null;
  }

  let registration: ServiceWorkerRegistration;
  try {
    registration = await navigator.serviceWorker.register(scriptURL(options.url), { scope: '/', updateViaCache: 'none' });
  } catch {
    return null;
  }

  let accepted = false;
  const offer = (worker: ServiceWorker) =>
    options.onUpdateReady(() => {
      accepted = true;
      worker.postMessage({ type: 'SKIP_WAITING' });
    });

  const hadController = navigator.serviceWorker.controller !== null;
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (accepted) location.reload();
    // The first claim of an uncontrolled page is not an update.
    else if (hadController) options.onControllerChange?.();
  });

  if (registration.waiting && navigator.serviceWorker.controller) offer(registration.waiting);

  registration.addEventListener('updatefound', () => {
    const worker = registration.installing;
    if (!worker) return;
    worker.addEventListener('statechange', () => {
      if (worker.state !== 'installed') return;
      if (navigator.serviceWorker.controller) offer(worker);
      else options.onOfflineReady?.();
    });
  });

  let lastCheck = Date.now();
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState !== 'visible' || Date.now() - lastCheck < UPDATE_CHECK_MS) return;
    lastCheck = Date.now();
    void registration.update().catch(() => {});
  });

  return registration;
}
