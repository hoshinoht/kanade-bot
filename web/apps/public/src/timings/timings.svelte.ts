// The weekly timings the signed-in member is on (`GET /api/public/timings`)
// and their ownership writes. Read when My runs opens and after every write;
// kept in memory for this tab only, dropped with the session.
import type { MemberTimings } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import { writePath, type OwnWrite } from './ownership';

/**
 * A write's outcome: done; `reauth` (the owner change needs a fresh Discord
 * sign-in, nothing saved); refused or failed with the error; or null when
 * the session ended or the portal closed (the portal replaces the screen).
 */
export type WriteOutcome = { done: true } | { done: false; reauth: true } | { done: false; reauth: false; error: unknown } | null;

export class MemberTimingsList {
  data = $state<MemberTimings | null>(null);
  /** Why the last read failed; empty once one answers. */
  error = $state('');
  /** The write in flight (its path), so its buttons show busy and others wait. */
  busy = $state('');
  #client: Client;
  #gone: (error: unknown) => boolean;
  #loading: Promise<void> | null = null;
  /** A hint landed while a read was out: read once more when it settles. */
  #stale = false;

  constructor(client: Client, gone: (error: unknown) => boolean) {
    this.#client = client;
    this.#gone = gone;
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
      this.data = await this.#client.get<MemberTimings>('/api/public/timings');
      this.error = '';
    } catch (error) {
      if (this.#gone(error)) this.clear();
      else this.error = error instanceof Error ? error.message : 'Your weekly timings did not load.';
    }
  }

  /** One write at a time; the list is read again after anything but a fresh-sign-in refusal. */
  async write(write: OwnWrite): Promise<WriteOutcome> {
    if (this.busy) return null;
    const path = writePath(write);
    this.busy = path;
    try {
      await this.#client.post(path, write.kind === 'hand' ? { to: write.to.id } : {});
      await this.load();
      return { done: true };
    } catch (error) {
      if (this.#gone(error)) return null;
      if (error instanceof ApiRequestError && error.kind === 'reauth') return { done: false, reauth: true };
      await this.load();
      return { done: false, reauth: false, error };
    } finally {
      this.busy = '';
    }
  }

  clear(): void {
    this.data = null;
    this.error = '';
    this.#stale = false;
  }
}
