import type { FixedRow, Week } from '@kanade/api-types';
import { ApiRequestError, createClient, type Client } from '@kanade/client';

/**
 * The weekly timings together with the week version they were read at.
 *
 * An editor pins `version` with the row it opens, and the server's per-field
 * check trusts that pair: a new version beside old values would let a save
 * silently undo someone else's edit. So the version is read first (older is
 * only a needless 409), held until the rows arrive, and both are published
 * at once; a load that a newer one overtook publishes nothing.
 */
export class FixedSnapshot {
  rows = $state<FixedRow[] | null>(null);
  version = $state<number | null>(null);
  error = $state('');
  #client: Client;
  #seq = 0;

  constructor(client: Client = createClient()) {
    this.#client = client;
  }

  /** Resolves once this load published, or was overtaken (false). */
  async load(): Promise<boolean> {
    const mine = ++this.#seq;
    try {
      const { version } = await this.#client.get<Week>('/api/admin/week');
      const rows = await this.#client.get<FixedRow[]>('/api/admin/fixed');
      if (mine !== this.#seq) return false;
      this.rows = rows;
      this.version = version;
      this.error = '';
    } catch (error) {
      if (mine !== this.#seq) return false;
      // The last good pair stays: still consistent, at worst stale (a 409).
      this.error = error instanceof ApiRequestError ? error.message : 'Could not load.';
    }
    return true;
  }
}
