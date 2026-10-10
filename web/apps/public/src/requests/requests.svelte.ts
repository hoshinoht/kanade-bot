// The member's requests to the admins (member-writes contract Phase B):
// `GET /api/public/requests/mine` (their own, newest first, with the limits
// and the form's choices), send and withdraw. Read when the masthead first
// shows its count and after every write; in memory for this tab only.
import type { MemberRequest, MemberRequests } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import type { RequestBody } from './form';

/**
 * A send or withdraw: done; `reauth` (sending needs a fresh sign-in, nothing
 * sent); `limit` (the open or daily cap, nothing sent); refused with the
 * server's reason; or null when the session ended or the portal closed.
 */
export type RequestOutcome =
  | { kind: 'done'; request: MemberRequest }
  | { kind: 'reauth' }
  | { kind: 'limit'; limit: 'open' | 'today'; words: string }
  | { kind: 'refused'; words: string; code: string }
  | null;

export class MemberRequestsList {
  data = $state<MemberRequests | null>(null);
  /** Why the last read failed; empty once one answers. */
  error = $state('');
  /** A send or withdraw is out. */
  busy = $state(false);
  #client: Client;
  #gone: (error: unknown) => boolean;
  #loading: Promise<void> | null = null;
  /** A hint landed while a read was out: read once more when it settles. */
  #stale = false;

  constructor(client: Client, gone: (error: unknown) => boolean) {
    this.#client = client;
    this.#gone = gone;
  }

  /** Waiting on the admins: the masthead's and the drawer's count. */
  get waiting(): number | null {
    return this.data ? this.data.requests.filter((r) => r.state === 'waiting').length : null;
  }

  /** Read now; a call while a read runs joins it. */
  load(): Promise<void> {
    this.#loading ??= this.#reads().finally(() => {
      this.#loading = null;
    });
    return this.#loading;
  }

  /** A hint said the list changed: a read already out may predate it, so one more follows that read. */
  hinted(): Promise<void> {
    if (this.#loading) this.#stale = true;
    return this.load();
  }

  async #reads(): Promise<void> {
    do {
      this.#stale = false;
      await this.#read();
      // Gone (cleared) meanwhile: nothing more to read.
    } while (this.#stale && this.data);
  }

  async #read(): Promise<void> {
    try {
      this.data = await this.#client.get<MemberRequests>('/api/public/requests/mine');
      this.error = '';
    } catch (error) {
      if (this.#gone(error)) this.clear();
      else this.error = error instanceof Error ? error.message : 'Your requests did not load.';
    }
  }

  /** Send one request; `key` repeats the same action (a retry after a fresh sign-in sends anew). */
  send(body: RequestBody, key?: string): Promise<RequestOutcome> {
    return this.#write(() => this.#client.post<MemberRequest>('/api/public/requests', body, key ? { idempotencyKey: key } : {}));
  }

  withdraw(id: string): Promise<RequestOutcome> {
    return this.#write(() => this.#client.post<MemberRequest>(`/api/public/requests/${encodeURIComponent(id)}/withdraw`, {}));
  }

  async #write(send: () => Promise<MemberRequest>): Promise<RequestOutcome> {
    if (this.busy) return null;
    this.busy = true;
    try {
      const request = await send();
      await this.load();
      return { kind: 'done', request };
    } catch (error) {
      if (this.#gone(error)) return null;
      if (error instanceof ApiRequestError && error.kind === 'reauth') return { kind: 'reauth' };
      const body = error instanceof ApiRequestError ? (error.body as (ApiRequestError['body'] & { limit?: string }) | null) : null;
      const words = error instanceof Error ? error.message : 'Try again.';
      await this.load();
      if (body?.error === 'request_limit') return { kind: 'limit', limit: body.limit === 'today' ? 'today' : 'open', words };
      return { kind: 'refused', words, code: body?.error ?? '' };
    } finally {
      this.busy = false;
    }
  }

  clear(): void {
    this.data = null;
    this.error = '';
    this.#stale = false;
  }
}
