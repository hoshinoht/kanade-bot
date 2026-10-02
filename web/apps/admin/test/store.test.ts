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
  generated_at: `2026-09-24T12:0${version}:00Z`, // minutes apart: the Live chip shows HH:MM
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

describe('AdminWeek: a drop during a held drag', () => {
  it('is sent against the week the drag began on, not a newer poll buffered meanwhile', async () => {
    const weeks = [week(1), week(5, 4)];
    const sent: { version: number }[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string, init?: RequestInit) => {
        const path = url.split('?')[0]!;
        if (init?.method === 'POST' && path.endsWith('/move')) {
          sent.push(JSON.parse(String(init.body)) as { version: number });
          return new Response(JSON.stringify({ error: 'stale', message: 'The week changed since it was loaded.' }), { status: 409 });
        }
        const body = path.endsWith('/stats') ? stats : path.endsWith('/week') ? (weeks.shift() ?? week(5, 4)) : [];
        return new Response(JSON.stringify(body), { status: 200 });
      }),
    );
    const store = new AdminWeek();
    await store.refresh();
    store.holding = true;
    await store.refresh(); // another admin's change arrives mid-drag: buffered
    expect(store.week?.version).toBe(1);
    // The planner starts the move first, then releases the hold.
    const moving = store.move('r1', { day: 3, time: '21:00' });
    store.holding = false;
    expect(store.week?.version).toBe(1);
    const outcome = await moving;
    expect(sent).toEqual([expect.objectContaining({ version: 1 })]);
    expect(outcome).toEqual({ ok: false, message: "Couldn't move HFA: The week changed since it was loaded." });
    // The buffered week then applies (the conflict also re-reads it).
    await store.refresh();
    expect(store.week?.version).toBe(5);
  });
});

describe('AdminWeek: swaps', () => {
  const r2 = (day = 3, time = '23:30'): Run => ({ ...run('r2', day), time });
  const pair = (version: number): Week => ({ ...week(version), runs: [run('r1', 0), r2()] });

  function serveSwap(answer: (body: { with: string; version: number }, path: string) => Response) {
    const posts: { path: string; body: { with: string; version: number } }[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string, init?: RequestInit) => {
        const path = url.split('?')[0]!;
        if (init?.method === 'POST') {
          const body = JSON.parse(String(init.body)) as { with: string; version: number };
          posts.push({ path, body });
          return answer(body, path);
        }
        const reply = path.endsWith('/stats') ? stats : path.endsWith('/week') ? pair(1) : [];
        return new Response(JSON.stringify(reply), { status: 200 });
      }),
    );
    return posts;
  }

  const slots = (store: AdminWeek) => store.week?.runs.map((r) => `${r.id} ${r.day} ${r.time}`);

  it('moves both cards at once, keeps the server’s result, and undo swaps them back in one step', async () => {
    let version = 1;
    const posts = serveSwap(() => {
      version += 1;
      // Odd answers swap them, even ones put them back (primary first each time).
      const swapped = version % 2 === 0;
      const runs = swapped ? [{ ...run('r1', 3), time: '23:30' }, r2(0, '21:00')] : [run('r1', 0), r2()];
      return new Response(JSON.stringify({ runs, version }), { status: 200 });
    });
    const store = new AdminWeek();
    await store.refresh();
    const pending = store.swap('r1', 'r2');
    // Optimistic: both cards, in one change.
    expect(slots(store)).toEqual(['r1 3 23:30', 'r2 0 21:00']);
    expect(await pending).toMatchObject({ ok: true, message: expect.stringMatching(/^Swapped HFA to .+, HFA to .+\.$/) });
    expect(store.week?.version).toBe(2);
    expect(store.lastMove).toEqual({ kind: 'swap', revision: 1, runId: 'r1', withId: 'r2' });
    expect(posts).toEqual([{ path: '/api/admin/runs/r1/swap', body: { with: 'r2', version: 1 } }]);

    const undone = await store.undo();
    expect(undone).toMatchObject({ ok: true, message: expect.stringMatching(/^Swap undone: /) });
    expect(slots(store)).toEqual(['r1 0 21:00', 'r2 3 23:30']);
    expect(posts[1]).toEqual({ path: '/api/admin/runs/r1/swap', body: { with: 'r2', version: 2 } });
    expect(store.lastMove).toBeNull();
  });

  it('own-time: only the days change on screen, clocks kept', async () => {
    serveSwap(() => new Response(JSON.stringify({ error: 'invalid', message: 'nope' }), { status: 422 }));
    const store = new AdminWeek();
    await store.refresh();
    store.week = { ...store.week!, runs: [run('r1', 0), { ...r2(), status: 'otot', time: null }] };
    const pending = store.swap('r1', 'r2');
    expect(slots(store)).toEqual(['r1 3 21:00', 'r2 0 null']);
    await pending;
  });

  it('a conflict puts both cards back and says why; nothing to undo', async () => {
    serveSwap(() => new Response(JSON.stringify({ error: 'stale', message: 'The week changed since it was loaded.' }), { status: 409 }));
    const store = new AdminWeek();
    await store.refresh();
    const outcome = await store.swap('r1', 'r2');
    expect(outcome).toEqual({ ok: false, message: "Couldn't swap HFA with HFA: The week changed since it was loaded." });
    expect(slots(store)).toEqual(['r1 0 21:00', 'r2 3 23:30']);
    expect(store.lastMove).toBeNull();
  });

  it('a refusal (the swap would leave the boss week) rolls both back with the server’s words', async () => {
    serveSwap(() => new Response(JSON.stringify({ error: 'invalid', message: 'that slot swap would move a run outside its boss week' }), { status: 422 }));
    const store = new AdminWeek();
    await store.refresh();
    const outcome = await store.swap('r1', 'r2');
    expect(outcome).toEqual({ ok: false, message: "Couldn't swap HFA with HFA: that slot swap would move a run outside its boss week" });
    expect(slots(store)).toEqual(['r1 0 21:00', 'r2 3 23:30']);
  });

  it('serializes a later move while a swap is pending, so its 409 rollback cannot clobber another write', async () => {
    let rejectSwap: ((value: Response) => void) | undefined;
    const posts: string[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string, init?: RequestInit) => {
        const path = url.split('?')[0]!;
        if (init?.method === 'POST' && path.endsWith('/swap')) {
          posts.push('swap');
          return new Promise<Response>((resolve) => (rejectSwap = resolve));
        }
        if (init?.method === 'POST') posts.push('move');
        const reply = path.endsWith('/stats') ? stats : path.endsWith('/week') ? pair(1) : [];
        return Promise.resolve(new Response(JSON.stringify(reply), { status: 200 }));
      }),
    );
    const store = new AdminWeek();
    await store.refresh();
    const pending = store.swap('r1', 'r2');
    expect(store.mutating).toBe(true);
    await expect(store.move('r1', { day: 2, time: '20:00' })).resolves.toEqual({ ok: false, message: 'Saving the last change…' });
    expect(posts).toEqual(['swap']);
    rejectSwap!(new Response(JSON.stringify({ error: 'stale', message: 'changed' }), { status: 409 }));
    await expect(pending).resolves.toMatchObject({ ok: false });
    expect(store.mutating).toBe(false);
    expect(slots(store)).toEqual(['r1 0 21:00', 'r2 3 23:30']);
  });
});
