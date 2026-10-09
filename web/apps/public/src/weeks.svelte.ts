// This boss week and the next as the signed-in member sees them
// (`GET /api/public/week`, `?week=next`), read together every 30 s.
// Offline, the last week that arrived stays on screen with its time
// ("showing the week as of 11:42"); it lives in this object only, in memory
// for this tab: nothing is written to storage, and sign-out drops it.
import type { MemberWeek } from '@kanade/api-types';
import { ApiRequestError, createPoller, type Client, type Poller } from '@kanade/client';

export type WeekKey = 'this' | 'next';

/** The member's Week reads every 30 s, as the portal always has (public-portal-plan § Live updates). */
export const WEEK_POLL_MS = 30_000;

export class MemberWeeks {
  this = $state<MemberWeek | null>(null);
  next = $state<MemberWeek | null>(null);
  /** When the shown weeks arrived (epoch ms, this device's clock): the "as of" time offline. */
  updated = $state<number | null>(null);
  /** The last read failed at the network: the weeks shown are the last ones seen. */
  offline = $state(false);
  /** Why the last read failed (any reason but a gone session); empty once a read answers. */
  error = $state('');
  #poller: Poller;

  /**
   * `gone`: the session ended or the portal closed (the portal replaces the
   * screen); the reads stop and nothing is kept.
   */
  constructor(client: Client, gone: (error: unknown) => boolean) {
    this.#poller = createPoller({
      intervalMs: WEEK_POLL_MS,
      task: (signal) =>
        Promise.all([client.get<MemberWeek>('/api/public/week', { signal }), client.get<MemberWeek>('/api/public/week?week=next', { signal })]),
      onData: ([current, next]) => {
        this.this = current;
        this.next = next;
        this.updated = Date.now();
        this.offline = false;
        this.error = '';
      },
      onError: (error) => {
        if (gone(error)) {
          this.clear();
          return;
        }
        this.offline = error instanceof ApiRequestError && (error.kind === 'network' || error.kind === 'timeout');
        this.error = error instanceof Error ? error.message : 'The week did not load.';
      },
    });
  }

  week(which: WeekKey): MemberWeek | null {
    return which === 'next' ? this.next : this.this;
  }

  start(): void {
    this.#poller.start();
  }

  /** Read now; a press while a read runs joins it. Restarts polling after repeated failures. */
  refresh(): Promise<void> {
    return this.#poller.refresh();
  }

  /** Sign-out, an ended session or a closed portal: stop reading and forget every week. */
  clear(): void {
    this.#poller.stop();
    this.this = null;
    this.next = null;
    this.updated = null;
    this.offline = false;
    this.error = '';
  }
}
