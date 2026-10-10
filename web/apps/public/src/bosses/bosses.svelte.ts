// The member portal's Bosses (catalog C7/C8): the catalog, event bosses and
// guides from the member boss reads. Read when Bosses opens; kept in memory
// for this tab only and dropped with the session.
import type { BossRow, EventBoss, PublicKnowledge } from '@kanade/api-types';
import type { Client } from '@kanade/client';

export class MemberBosses {
  rows = $state<BossRow[] | null>(null);
  events = $state<EventBoss[]>([]);
  /** Why the list did not load; empty once it does. */
  error = $state('');
  /** The guide on screen and its key; a failed read keeps its key with the reason. */
  guide = $state<{ key: string; data: PublicKnowledge | null; error: string } | null>(null);
  #client: Client;
  #gone: (error: unknown) => boolean;

  constructor(client: Client, gone: (error: unknown) => boolean) {
    this.#client = client;
    this.#gone = gone;
  }

  async load(): Promise<void> {
    try {
      const [rows, events] = await Promise.all([
        this.#client.get<BossRow[]>('/api/public/bosses'),
        this.#client.get<EventBoss[]>('/api/public/bosses/events'),
      ]);
      this.rows = rows;
      this.events = events;
      this.error = '';
    } catch (error) {
      if (this.#gone(error)) this.clear();
      else this.error = error instanceof Error ? error.message : 'The boss list did not load.';
    }
  }

  /** The guide for `key`; a later call for another key wins over an earlier answer. */
  async open(key: string): Promise<void> {
    if (this.guide?.key === key && this.guide.data) return;
    this.guide = { key, data: null, error: '' };
    try {
      const data = await this.#client.get<PublicKnowledge>(`/api/public/bosses/${encodeURIComponent(key)}/knowledge`);
      if (this.guide?.key === key) this.guide = { key, data, error: '' };
    } catch (error) {
      if (this.#gone(error)) this.clear();
      else if (this.guide?.key === key) this.guide = { key, data: null, error: error instanceof Error ? error.message : 'The guide did not load.' };
    }
  }

  clear(): void {
    this.rows = null;
    this.events = [];
    this.error = '';
    this.guide = null;
  }
}
