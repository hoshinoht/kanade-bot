import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Run, Stats, Week } from '@kanade/api-types';
import { AdminWeek } from '../src/store.svelte';

const run = (id: string, day: number): Run => ({
  id,
  day,
  time: '21:00',
  status: 'planned',
  short_id: 'abc',
  bosses: [
    { token: 'HFA', key: 'FA', name: 'The First Adversary', difficulty: 'h', level: 270, portrait: null, portrait_sm: null, art: null, hue: 0 },
  ],
  tally: { on: 0, total: 0 },
  participants: [],
  party: 'p',
  channel: '#p',
  cards: [],
  fixed_id: null,
  amended: false,
  roster_change: null,
});

const week = (version: number, day = 0): Week => ({
  starts: '2026-09-24',
  timezone: 'Asia/Kuala_Lumpur',
  reset: 'Thu 00:00',
  days: [],
  runs: [run('r1', day)],
  generated_at: `2026-09-24T12:00:0${version}Z`,
  version,
});
const stats: Stats = { per_day: [] };

/** Serves the given weeks in order to successive GET /api/admin/week calls. */
function serve(...weeks: Week[]) {
  const queue = [...weeks];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      const path = url.split('?')[0]!;
      const body = path.endsWith('/stats')
        ? stats
        : path.endsWith('/summary')
          ? { next: null, unanswered: 0, inbox: 0, model: { busy: false, holder: null } }
          : path.endsWith('/week')
            ? queue.shift()
            : [];
      return new Response(JSON.stringify(body), { status: 200 });
    }),
  );
}

afterEach(() => vi.unstubAllGlobals());

describe('AdminWeek snapshots', () => {
  it('ignores a snapshot older than the week it holds', async () => {
    const store = new AdminWeek();
    serve(week(3, 2), week(2, 5));
    await store.refresh();
    expect(store.week?.version).toBe(3);
    await store.refresh();
    expect(store.week?.version).toBe(3);
    expect(store.week?.runs[0]?.day).toBe(2);
    expect(store.fresh).toBe('live');
  });

  it('buffers the newest snapshot while holding and applies it on release', async () => {
    const store = new AdminWeek();
    serve(week(1), week(2, 3), week(4, 6), week(3, 1));
    await store.refresh();
    const shownAt = store.updated;
    store.holding = true;
    await store.refresh();
    await store.refresh();
    await store.refresh(); // version 3 arrives after 4 and must not replace it in the buffer
    expect(store.week?.version).toBe(1);
    expect(store.updated).toBe(shownAt);
    store.holding = false;
    expect(store.week?.version).toBe(4);
    expect(store.week?.runs[0]?.day).toBe(6);
    expect(store.updated).not.toBe(shownAt);
  });
});
