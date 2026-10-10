// This boss week and the next as the signed-in member sees them
// (`GET /api/public/week`, `?week=next`), read together every 30 s.
// Offline, the last week that arrived stays on screen with its time
// ("showing the week as of 11:42"); it lives in this object only, in memory
// for this tab: nothing is written to storage, and sign-out drops it.
// A timed read that changes a shown week is an arrival (as in the admin
// store): the board glides and marks it, numbers pulse. The member's own
// Refresh, a reconnect and a write's echo are not.
import type { MemberRun, MemberWeek } from '@kanade/api-types';
import { ApiRequestError, createPoller, wroteWithin, type Client, type Poller } from '@kanade/client';

export type WeekKey = 'this' | 'next';

/** The member's Week reads every 30 s, as the portal always has (public-portal-plan § Live updates). */
export const WEEK_POLL_MS = 30_000;

/** Changes read this soon after this page's own write are its echo, not an arrival (the admin's `OWN_ECHO_MS`). */
const OWN_ECHO_MS = 3_000;

/** The same week, whatever time it was read at. */
function same(a: MemberWeek | null, b: MemberWeek): boolean {
  return a !== null && JSON.stringify({ ...a, generated_at: '' }) === JSON.stringify({ ...b, generated_at: '' });
}

export class MemberWeeks {
  this = $state<MemberWeek | null>(null);
  next = $state<MemberWeek | null>(null);
  /** When the shown weeks arrived (epoch ms, this device's clock): the "as of" time offline. */
  updated = $state<number | null>(null);
  /** The last read failed at the network: the weeks shown are the last ones seen. */
  offline = $state(false);
  /** Why the last read failed (any reason but a gone session); empty once a read answers. */
  error = $state('');
  /**
   * Bumped whenever a week from elsewhere lands on screen (a timed read that
   * changed something); `pulse(value, weeks.arrival)` ticks numbers that
   * changed with it.
   */
  arrival = $state(0);
  /**
   * Called just before such a week replaces the shown one, with the runs it
   * adds (either week): the board glides moved cards (FLIP) and marks the new
   * ones. Never for the member's own Refresh or a write's echo.
   */
  beforeArrival: ((added: string[]) => void) | null = null;
  /** The next read was asked for (`refresh()`): nothing it brings is an arrival. */
  #asked = false;
  #poller: Poller;

  /**
   * `gone`: the session ended or the portal closed (the portal replaces the
   * screen); the reads stop and nothing is kept.
   */
  constructor(client: Client, gone: (error: unknown) => boolean) {
    this.#poller = createPoller({
      intervalMs: WEEK_POLL_MS,
      task: async (signal) => {
        const remote = !this.#asked;
        this.#asked = false;
        const [current, next] = await Promise.all([
          client.get<MemberWeek>('/api/public/week', { signal }),
          client.get<MemberWeek>('/api/public/week?week=next', { signal }),
        ]);
        return [current, next, remote] as const;
      },
      onData: ([current, next, remote]) => {
        const keepThis = same(this.this, current);
        const keepNext = same(this.next, next);
        if (remote && this.this && !(keepThis && keepNext) && !wroteWithin(OWN_ECHO_MS)) {
          // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a lookup built and read here, never state
          const had = new Set([...this.this.runs, ...(this.next?.runs ?? [])].map((r) => r.id));
          this.beforeArrival?.([...current.runs, ...next.runs].filter((r) => !had.has(r.id)).map((r) => r.id));
          this.arrival += 1;
        }
        // An unchanged read keeps the objects on screen (only its clock moves).
        if (keepThis && this.this) this.this.generated_at = current.generated_at;
        else this.this = current;
        if (keepNext && this.next) this.next.generated_at = next.generated_at;
        else this.next = next;
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

  /** The run with this id in either week, and which week holds it. */
  find(id: string): { run: MemberRun; which: WeekKey; week: MemberWeek } | null {
    for (const which of ['this', 'next'] as const) {
      const week = this.week(which);
      const run = week?.runs.find((r) => r.id === id);
      if (week && run) return { run, which, week };
    }
    return null;
  }

  /**
   * Puts this copy of a run on screen in place of the one with its id (an
   * optimistic answer, its rollback, or a write's answer with the new
   * `version`), without waiting for the next read.
   */
  put(run: MemberRun, version?: number): void {
    const found = this.find(run.id);
    if (!found) return;
    found.week.runs = found.week.runs.map((r) => (r.id === run.id ? run : r));
    if (version !== undefined) for (const week of [this.this, this.next]) if (week) week.version = version;
  }

  start(): void {
    this.#poller.start();
  }

  /** Read now, as the member asked (Refresh, Try again); a press while a read runs joins it. Restarts polling after repeated failures. */
  refresh(): Promise<void> {
    // Joining a timed read already out does not make that read ours.
    if (this.#poller.state !== 'running') this.#asked = true;
    return this.#poller.refresh();
  }

  /** Sign-out, an ended session or a closed portal: stop reading and forget every week. */
  clear(): void {
    this.#poller.stop();
    this.#asked = false;
    this.this = null;
    this.next = null;
    this.updated = null;
    this.offline = false;
    this.error = '';
  }
}
