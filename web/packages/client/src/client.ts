import type { ApiError } from '@kanade/api-types';

/**
 * `reauth`: a 401 `reauth_required` (public origin). The session lives on;
 * only this change needs the member to sign in with Discord again.
 */
export type FailureKind = 'network' | 'timeout' | 'http' | 'reauth' | 'parse' | 'aborted';

export class ApiRequestError extends Error {
  readonly kind: FailureKind;
  readonly status: number | null;
  readonly body: ApiError | null;

  constructor(kind: FailureKind, message: string, status: number | null = null, body: ApiError | null = null) {
    super(message);
    this.name = 'ApiRequestError';
    this.kind = kind;
    this.status = status;
    this.body = body;
  }

  /** Worth retrying automatically: the request may succeed unchanged later. */
  get transient(): boolean {
    return this.kind === 'network' || this.kind === 'timeout' || (this.status !== null && this.status >= 500);
  }
}

/**
 * Session-bound CSRF token for guarded writes (admin-api API-5). The API paths
 * come from the app, so the public bundle never names the admin API.
 */
export interface CsrfGuard {
  /** Read whose `X-Kanade-CSRF` answer header carries the token. */
  sessionPath: string;
  /** Which unsafe requests carry it. */
  guards: (path: string) => boolean;
  token: string | null;
  refreshing: Promise<void> | null;
}

export const CSRF_HEADER = 'X-Kanade-CSRF';
const UNSAFE = new Set(['POST', 'PATCH', 'PUT', 'DELETE']);
let pageCsrf: CsrfGuard | null = null;

export function createCsrfGuard(sessionPath: string, guards: CsrfGuard['guards']): CsrfGuard {
  return { sessionPath, guards, token: null, refreshing: null };
}

/** Page-wide guard for every client without its own, so a refreshed token reaches them all. */
export function guardWrites(sessionPath: string, guards: CsrfGuard['guards']): void {
  pageCsrf = createCsrfGuard(sessionPath, guards);
}

let unauthenticated: ((path: string) => void) | null = null;

/** When this page last completed a write (any client), for telling its own echoes from arrivals. */
let wroteAt = Number.NEGATIVE_INFINITY;

/** This page completed a write less than `ms` ago: changes read now are likely its own echo. */
export function wroteWithin(ms: number): boolean {
  return Date.now() - wroteAt < ms;
}

/**
 * Page-wide: told of every 401 that means the session is gone (signed out,
 * ended or expired), so the app can send the user to sign in. A 401
 * `reauth_required` is not one of them: the session lives on and only this
 * change needs a fresh sign-in, so the caller handles the `reauth` error.
 */
export function onUnauthenticated(handler: ((path: string) => void) | null): void {
  unauthenticated = handler;
}

/** The 401 code for "sign in with Discord again to make this change" (session kept). */
const REAUTH_REQUIRED = 'reauth_required';

/** A fresh `Idempotency-Key` (1–128 of `[A-Za-z0-9-_.:]`): one per user action. */
export function newIdempotencyKey(): string {
  const bytes = globalThis.crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export interface ClientOptions {
  /** Same-origin prefix; the CSP only allows `connect-src 'self'`. */
  base?: string;
  timeoutMs?: number;
  fetch?: typeof fetch;
  /** Defaults to the page-wide guard, if the app installed one. */
  csrf?: CsrfGuard;
}

export interface RequestOptions {
  signal?: AbortSignal;
  /** Pin the key to repeat an earlier action; otherwise each call is a new action. */
  idempotencyKey?: string;
}

export interface Client {
  get<T>(path: string, options?: RequestOptions): Promise<T>;
  post<T>(path: string, body: unknown, options?: RequestOptions): Promise<T>;
  patch<T>(path: string, body: unknown, options?: RequestOptions): Promise<T>;
  put<T>(path: string, body: unknown, options?: RequestOptions): Promise<T>;
  delete<T>(path: string, options?: RequestOptions): Promise<T>;
}

/** Tagged answers one client keeps for revalidation; the oldest goes first. */
const REMEMBERED_TAGS = 32;

export function createClient(options: ClientOptions = {}): Client {
  const base = options.base ?? '';
  // GET answers that carried an `ETag`: the next read of the path asks
  // `If-None-Match`, and a 304 hands back the very same object, so callers
  // that compare (or keep) what is on screen see "unchanged" for free.
  const tagged = new Map<string, { tag: string; value: unknown }>();
  const timeoutMs = options.timeoutMs ?? 10_000;
  const doFetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
  // Read per request: module-level clients may exist before the app installs its guard.
  const guard = () => options.csrf ?? pageCsrf;

  async function attempt<T>(method: string, path: string, body: unknown, opts: RequestOptions, extra: Record<string, string>): Promise<T> {
    const timeout = AbortSignal.timeout(timeoutMs);
    const signal = opts.signal ? AbortSignal.any([opts.signal, timeout]) : timeout;
    const known = method === 'GET' ? tagged.get(path) : undefined;
    if (known) extra = { ...extra, 'If-None-Match': known.tag };
    let response: Response;
    try {
      response = await doFetch(base + path, {
        method,
        signal,
        // Private API data must never land in the HTTP cache either.
        cache: 'no-store',
        credentials: 'same-origin',
        headers: { Accept: 'application/json', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...extra },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (error) {
      if (opts.signal?.aborted) throw new ApiRequestError('aborted', 'Request cancelled');
      if (timeout.aborted) throw new ApiRequestError('timeout', `No answer within ${Math.round(timeoutMs / 1000)} s`);
      throw new ApiRequestError('network', error instanceof Error ? error.message : 'Network unavailable');
    }

    // Any answer may carry it: sign-in and the session read, and on the public
    // origin any request that rotated the session. The newest one wins.
    const token = response.headers.get(CSRF_HEADER);
    const csrf = guard();
    if (token && csrf) csrf.token = token;

    if (response.status === 304 && known) return known.value as T;

    let parsed: unknown = null;
    const text = await response.text();
    if (text) {
      try {
        parsed = JSON.parse(text);
      } catch {
        throw new ApiRequestError('parse', 'The server sent something that is not JSON', response.status);
      }
    }
    if (!response.ok) {
      const apiError = isApiError(parsed) ? parsed : null;
      const reauth = response.status === 401 && apiError?.error === REAUTH_REQUIRED;
      if (response.status === 401 && !reauth) unauthenticated?.(path);
      throw new ApiRequestError(reauth ? 'reauth' : 'http', apiError?.message ?? `HTTP ${response.status}`, response.status, apiError);
    }
    if (method === 'GET') remember(path, response.headers.get('ETag'), parsed);
    return parsed as T;
  }

  function remember(path: string, tag: string | null, value: unknown) {
    tagged.delete(path);
    if (!tag) return;
    tagged.set(path, { tag, value });
    if (tagged.size > REMEMBERED_TAGS) tagged.delete(tagged.keys().next().value!);
  }

  /** Concurrent callers share one session read; a failure leaves the server to refuse. */
  function refreshCsrf(csrf: CsrfGuard): Promise<void> {
    csrf.refreshing ??= attempt('GET', csrf.sessionPath, undefined, {}, {})
      .then(
        () => undefined,
        () => undefined,
      )
      .finally(() => {
        csrf.refreshing = null;
      });
    return csrf.refreshing;
  }

  async function request<T>(method: string, path: string, body: unknown, opts: RequestOptions): Promise<T> {
    if (!UNSAFE.has(method)) return attempt(method, path, body, opts, {});
    // Kept across both retries below: they repeat this action, so the server replays rather than reapplies.
    const key = opts.idempotencyKey ?? newIdempotencyKey();
    const csrf = guard();
    const guarded = csrf !== null && csrf.guards(path);
    if (guarded && csrf.token === null) await refreshCsrf(csrf);
    let csrfRetried = false;
    let networkRetried = false;
    for (;;) {
      const headers: Record<string, string> = { 'Idempotency-Key': key };
      if (guarded && csrf.token) headers[CSRF_HEADER] = csrf.token;
      try {
        const done = await attempt<T>(method, path, body, opts, headers);
        wroteAt = Date.now();
        return done;
      } catch (error) {
        if (!(error instanceof ApiRequestError)) throw error;
        // One refresh per action: a token that is still refused means the session itself is gone.
        if (guarded && !csrfRetried && error.status === 403 && error.body?.error === 'csrf') {
          csrfRetried = true;
          await refreshCsrf(csrf);
          continue;
        }
        if (!networkRetried && error.kind === 'network') {
          networkRetried = true;
          continue;
        }
        throw error;
      }
    }
  }

  return {
    get: (path, opts = {}) => request('GET', path, undefined, opts),
    post: (path, body, opts = {}) => request('POST', path, body, opts),
    patch: (path, body, opts = {}) => request('PATCH', path, body, opts),
    put: (path, body, opts = {}) => request('PUT', path, body, opts),
    delete: (path, opts = {}) => request('DELETE', path, undefined, opts),
  };
}

function isApiError(value: unknown): value is ApiError {
  return typeof value === 'object' && value !== null && 'error' in value && 'message' in value;
}
