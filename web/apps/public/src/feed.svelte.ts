import type { PublicWeek } from '@kanade/api-types';
import { ApiRequestError, createClient, createPoller, type Poller } from '@kanade/client';
import { clockTime, type FreshState } from '@kanade/ui';

const POLL_MS = 30_000;

/** A closed portal is a state, not a failure: polling keeps its normal cadence so a reopening shows up. */
type Read = { open: true; week: PublicWeek } | { open: false };

/** Read-only week feed: bounded polling, never cached beyond this page's memory. */
export class WeekFeed {
  week = $state<PublicWeek | null>(null);
  fresh = $state<FreshState>('loading');
  updated = $state('');
  /** The admins closed the public portal; the old week is dropped, not shown stale. */
  closed = $state(false);
  #poller: Poller;

  constructor(path: string) {
    const client = createClient();
    this.#poller = createPoller<Read>({
      task: (signal) =>
        client.get<PublicWeek>(path, { signal }).then(
          (week): Read => ({ open: true, week }),
          (error: unknown): Read => {
            if (error instanceof ApiRequestError && error.body?.error === 'closed') return { open: false };
            throw error;
          },
        ),
      intervalMs: POLL_MS,
      maxIntervalMs: 5 * 60_000,
      maxFailures: 6,
      onData: (read) => {
        if (!read.open) {
          this.closed = true;
          this.week = null;
          // Neutral, not green: the portal is closed, and checked on the usual cadence.
          this.updated = new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(Date.now());
          this.fresh = 'closed';
          return;
        }
        const { week } = read;
        this.week = week;
        this.updated = clockTime(week.generated_at, week.timezone);
        this.fresh = 'live';
        this.closed = false;
      },
      onError: (error) => {
        const offline = !navigator.onLine || (error instanceof ApiRequestError && error.kind === 'network');
        this.fresh = offline ? 'offline' : this.week ? 'stale' : 'error';
      },
    });
  }

  start(): () => void {
    this.#poller.start();
    const online = () => void this.#poller.refresh();
    window.addEventListener('online', online);
    return () => {
      window.removeEventListener('online', online);
      this.#poller.stop();
    };
  }

  refresh(): Promise<void> {
    return this.#poller.refresh();
  }
}
