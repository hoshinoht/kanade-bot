import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MemberRun, MemberWeek, PublicSession } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import { Portal } from '../src/portal.svelte';
import { MemberWeeks, WEEK_POLL_MS } from '../src/weeks.svelte';

const week = (version: number, starts = '2026-09-24'): MemberWeek => ({
  starts,
  timezone: 'Asia/Singapore',
  reset: 'Thu 08:00',
  days: [],
  runs: [],
  generated_at: '2026-09-28T04:00:00Z',
  version,
});

/** A client whose week reads answer `answer()` (a value or a thrown error). */
function fakeClient(answer: (path: string) => MemberWeek | Error) {
  const paths: string[] = [];
  const client = {
    get: vi.fn(async (path: string) => {
      paths.push(path);
      const value = answer(path);
      if (value instanceof Error) throw value;
      return value;
    }),
  } as unknown as Client;
  return { client, paths };
}

const gone = (error: unknown) => error instanceof ApiRequestError && (error.status === 401 || error.status === 503);
const shown: MemberWeeks[] = [];
function weeksFor(client: Client) {
  const weeks = new MemberWeeks(client, gone);
  shown.push(weeks);
  return weeks;
}

afterEach(() => {
  for (const weeks of shown.splice(0)) weeks.clear();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('MemberWeeks', () => {
  it('reads this week and the next together', async () => {
    const { client, paths } = fakeClient((path) => (path.endsWith('next') ? week(2, '2026-10-01') : week(1)));
    const weeks = weeksFor(client);
    await weeks.refresh();
    expect(paths).toEqual(['/api/public/week', '/api/public/week?week=next']);
    expect(weeks.week('this')?.starts).toBe('2026-09-24');
    expect(weeks.week('next')?.starts).toBe('2026-10-01');
    expect(weeks.updated).not.toBeNull();
    expect(weeks.offline).toBe(false);
  });

  it('offline keeps the last-seen weeks in memory with their time, and writes no storage', async () => {
    const setItem = vi.fn();
    vi.stubGlobal('localStorage', { setItem, getItem: vi.fn(), removeItem: vi.fn() });
    let up = true;
    const { client } = fakeClient(() => (up ? week(1) : new ApiRequestError('network', 'Failed to fetch')));
    const weeks = weeksFor(client);
    await weeks.refresh();
    const seen = weeks.updated;
    up = false;
    await weeks.refresh();
    expect(weeks.offline).toBe(true);
    expect(weeks.this?.version).toBe(1);
    expect(weeks.next?.version).toBe(1);
    expect(weeks.updated).toBe(seen);
    // Back online: the next answer replaces the notice.
    up = true;
    await weeks.refresh();
    expect(weeks.offline).toBe(false);
    expect(setItem).not.toHaveBeenCalled();
  });

  it('a timeout reads as offline; a refusal is an error, not offline', async () => {
    let failure: Error = new ApiRequestError('timeout', 'Timed out');
    const { client } = fakeClient(() => failure);
    const weeks = weeksFor(client);
    await weeks.refresh();
    expect(weeks.offline).toBe(true);
    failure = new ApiRequestError('http', 'Server error', 500);
    await weeks.refresh();
    expect(weeks.offline).toBe(false);
    expect(weeks.error).toBe('Server error');
  });

  for (const [why, error] of [
    ['an ended session (401)', new ApiRequestError('http', 'Unauthenticated', 401, { error: 'unauthenticated', message: '' })],
    ['a closed portal (503)', new ApiRequestError('http', 'Closed', 503, { error: 'closed', message: '' })],
  ] as const) {
    it(`${why} forgets every week and stops reading`, async () => {
      vi.useFakeTimers();
      let fail = false;
      const { client, paths } = fakeClient(() => (fail ? error : week(1)));
      const weeks = weeksFor(client);
      weeks.start();
      await vi.advanceTimersByTimeAsync(0);
      expect(weeks.this).not.toBeNull();
      fail = true;
      await vi.advanceTimersByTimeAsync(WEEK_POLL_MS);
      expect([weeks.this, weeks.next, weeks.updated, weeks.offline, weeks.error]).toEqual([null, null, null, false, '']);
      const reads = paths.length;
      await vi.advanceTimersByTimeAsync(WEEK_POLL_MS * 10);
      expect(paths.length).toBe(reads);
    });
  }

  it('a timed read that changes a week is an arrival: the hook sees the new runs first', async () => {
    vi.useFakeTimers();
    let runs = ['r-1'];
    const { client } = fakeClient((path) => ({ ...week(runs.length, path.endsWith('next') ? '2026-10-01' : '2026-09-24'), runs: path.endsWith('next') ? [] : runs.map((id) => ({ id }) as MemberRun) }));
    const weeks = weeksFor(client);
    const seen: string[][] = [];
    weeks.beforeArrival = (added) => seen.push([...added, `shown ${weeks.this?.runs.length}`]);
    weeks.start();
    await vi.advanceTimersByTimeAsync(0);
    // The first week is not an arrival.
    expect([weeks.arrival, seen]).toEqual([0, []]);
    runs = ['r-1', 'r-2'];
    await vi.advanceTimersByTimeAsync(WEEK_POLL_MS);
    expect(weeks.arrival).toBe(1);
    expect(seen).toEqual([['r-2', 'shown 1']]);
    expect(weeks.this?.runs.map((r) => r.id)).toEqual(['r-1', 'r-2']);
  });

  it("the member's own Refresh is not an arrival, and an unchanged read keeps the week objects", async () => {
    let runs = ['r-1'];
    let at = '2026-09-28T04:00:00Z';
    const { client } = fakeClient(() => ({ ...week(runs.length), generated_at: at, runs: runs.map((id) => ({ id }) as MemberRun) }));
    const weeks = weeksFor(client);
    const hook = vi.fn();
    weeks.beforeArrival = hook;
    await weeks.refresh();
    runs = ['r-1', 'r-2'];
    await weeks.refresh();
    expect([weeks.arrival, hook.mock.calls.length, weeks.this?.runs.length]).toEqual([0, 0, 2]);
    const kept = weeks.this;
    at = '2026-09-28T04:00:30Z';
    await weeks.refresh();
    expect(weeks.this).toBe(kept);
    expect(weeks.this?.generated_at).toBe(at);
  });
});

describe('Portal sign-out', () => {
  it('drops the last-seen weeks with the session', async () => {
    const session: PublicSession = { member: { id: '42', display: 'Rin', avatar: '' }, fresh_until: '2026-10-09T12:00:00Z' };
    vi.stubGlobal('navigator', { onLine: true });
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        if (url.endsWith('/logout')) return new Response(null, { status: 204 });
        const body = url.endsWith('/status')
          ? { portal: 'open' }
          : url.endsWith('/session')
            ? session
            : url.startsWith('/api/public/week')
              ? week(1)
              : { sessions: [], generated_at: '2026-10-09T11:00:00Z' };
        return new Response(JSON.stringify(body), { status: 200, headers: { 'x-kanade-csrf': 'token' } });
      }),
    );
    const portal = new Portal('none');
    shown.push(portal.weeks);
    await portal.load();
    await portal.weeks.refresh();
    expect(portal.weeks.this?.version).toBe(1);
    expect(await portal.signOut()).toBeNull();
    expect(portal.screen).toEqual({ kind: 'signin', notice: 'signed-out' });
    expect([portal.weeks.this, portal.weeks.next, portal.weeks.updated]).toEqual([null, null, null]);
  });
});
