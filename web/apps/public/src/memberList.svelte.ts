// A member's own list read from one public route (My requests, weekly timings):
// read on demand and after writes, in memory for this tab only, dropped with the session.
import type { Client } from '@kanade/client';

export class MemberList<T> {
  data = $state<T | null>(null);
  /** Why the last read failed; empty once one answers. */
  error = $state('');
  protected readonly client: Client;
  protected readonly gone: (error: unknown) => boolean;
  #path: string;
  #failed: string;
  #loading: Promise<void> | null = null;
  /** A hint landed while a read was out: read once more when it settles. */
  #stale = false;

  /** `failed` is the error line when a read fails without a message. */
  constructor(client: Client, gone: (error: unknown) => boolean, path: string, failed: string) {
    this.client = client;
    this.gone = gone;
    this.#path = path;
    this.#failed = failed;
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
      this.data = await this.client.get<T>(this.#path);
      this.error = '';
    } catch (error) {
      if (this.gone(error)) this.clear();
      else this.error = error instanceof Error ? error.message : this.#failed;
    }
  }

  clear(): void {
    this.data = null;
    this.error = '';
    this.#stale = false;
  }
}
