import type { PastItem, PastPage } from '@kanade/api-types';
import { ApiRequestError, createClient, type Client } from '@kanade/client';
import { errorText } from '../resource.svelte';

/** The Past tab's closed items, newest first: the first page on open, older pages appended. */
export class PastLog {
  items = $state<PastItem[] | null>(null);
  /** The `before` cursor of the next (older) page; null on the last page. */
  next = $state<string | null>(null);
  error = $state('');
  loading = $state(false);
  #client: Client;

  constructor(client: Client = createClient()) {
    this.#client = client;
  }

  async load(more = false): Promise<void> {
    if (this.loading || (more && !this.next)) return;
    this.loading = true;
    const query = more && this.next ? `?before=${encodeURIComponent(this.next)}` : '';
    try {
      const page = await this.#client.get<PastPage>(`/api/admin/inbox/past${query}`);
      this.items = more ? [...(this.items ?? []), ...page.items] : page.items;
      this.next = page.next_before;
      this.error = '';
    } catch (error) {
      this.error = error instanceof ApiRequestError ? errorText(error) : 'Could not load past decisions.';
    } finally {
      this.loading = false;
    }
  }
}
