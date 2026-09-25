import type { ApiError } from '@kanade/api-types';

export type FailureKind = 'network' | 'timeout' | 'http' | 'parse' | 'aborted';

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

export interface ClientOptions {
  /** Same-origin prefix; the CSP only allows `connect-src 'self'`. */
  base?: string;
  timeoutMs?: number;
  fetch?: typeof fetch;
}

export interface RequestOptions {
  signal?: AbortSignal;
}

export interface Client {
  get<T>(path: string, options?: RequestOptions): Promise<T>;
  post<T>(path: string, body: unknown, options?: RequestOptions): Promise<T>;
  patch<T>(path: string, body: unknown, options?: RequestOptions): Promise<T>;
  delete<T>(path: string, options?: RequestOptions): Promise<T>;
}

export function createClient(options: ClientOptions = {}): Client {
  const base = options.base ?? '';
  const timeoutMs = options.timeoutMs ?? 10_000;
  const doFetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));

  async function request<T>(method: string, path: string, body: unknown, opts: RequestOptions): Promise<T> {
    const timeout = AbortSignal.timeout(timeoutMs);
    const signal = opts.signal ? AbortSignal.any([opts.signal, timeout]) : timeout;
    let response: Response;
    try {
      response = await doFetch(base + path, {
        method,
        signal,
        // Private API data must never land in the HTTP cache either.
        cache: 'no-store',
        credentials: 'same-origin',
        headers: body === undefined ? { Accept: 'application/json' } : { Accept: 'application/json', 'Content-Type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (error) {
      if (opts.signal?.aborted) throw new ApiRequestError('aborted', 'Request cancelled');
      if (timeout.aborted) throw new ApiRequestError('timeout', `No answer within ${Math.round(timeoutMs / 1000)} s`);
      throw new ApiRequestError('network', error instanceof Error ? error.message : 'Network unavailable');
    }

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
      throw new ApiRequestError('http', apiError?.message ?? `HTTP ${response.status}`, response.status, apiError);
    }
    return parsed as T;
  }

  return {
    get: (path, opts = {}) => request('GET', path, undefined, opts),
    post: (path, body, opts = {}) => request('POST', path, body, opts),
    patch: (path, body, opts = {}) => request('PATCH', path, body, opts),
    delete: (path, opts = {}) => request('DELETE', path, undefined, opts),
  };
}

function isApiError(value: unknown): value is ApiError {
  return typeof value === 'object' && value !== null && 'error' in value && 'message' in value;
}
