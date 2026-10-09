import { describe, expect, it, vi } from 'vitest';
import type { Boss, MemberOwnerRequest, MemberTiming, MemberTimings } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import { returnPath } from '../src/timings/flow.svelte';
import { doneWords, expiresWords, freshWindow, incoming, pendingAsk, refusalWords, sortTimings, writePath } from '../src/timings/ownership';
import { MemberTimingsList } from '../src/timings/timings.svelte';

const NOW = '2026-09-29T04:00:00Z';
const boss = (token: string, name: string): Boss => ({ token, key: name, name, difficulty: 'x', level: null, portrait: null, portrait_sm: null, art: null }) as unknown as Boss;
const me = { id: '1001', name: 'Asahi' };
const ren = { id: '1002', name: 'Ren' };
const tsubame = { id: '1005', name: 'Tsubame' };

const ask = (requester = tsubame, mine = false, hours = 19): MemberOwnerRequest => ({
  id: `own-${requester.id}`,
  requester,
  created_at: NOW,
  expires_at: new Date(Date.parse(NOW) + hours * 3_600_000 + 20 * 60_000).toISOString(),
  status: 'open',
  mine,
});

const timing = (id: string, weekday: number, time: string, over: Partial<MemberTiming> = {}): MemberTiming => ({
  id,
  bosses: [boss('XKalos', 'Kalos')],
  weekday,
  time,
  party: [me, ren, tsubame],
  owner: me,
  owner_pinned: false,
  you_own: true,
  requests: [],
  ...over,
});

describe('weekly timing words', () => {
  it('orders timings from the boss week start, then by time', () => {
    const list = [timing('tue', 1, '23:30'), timing('fri', 4, '22:00'), timing('thu', 3, '22:00'), timing('thu-early', 3, '20:00'), timing('sun', 6, '20:00')];
    expect(sortTimings(list, 'Thu').map((t) => t.id)).toEqual(['thu-early', 'thu', 'fri', 'sun', 'tue']);
    expect(sortTimings(list, undefined).map((t) => t.id)).toEqual(['tue', 'thu-early', 'thu', 'fri', 'sun']);
  });

  it('shows the owner the open asks of others and anyone else only their own', () => {
    const owned = timing('fri', 4, '22:00', { requests: [ask(), { ...ask(ren), status: 'declined' }] });
    expect(incoming(owned).map((r) => r.requester.name)).toEqual(['Tsubame']);
    expect(pendingAsk(owned)).toBeNull();
    const theirs = timing('sun', 6, '20:00', { you_own: false, owner: ren, requests: [ask(me, true)] });
    expect(incoming(theirs)).toEqual([]);
    expect(pendingAsk(theirs)?.requester.name).toBe('Asahi');
  });

  it('says what is left of an ask in whole hours, then minutes, by the server clock', () => {
    expect(expiresWords(ask(tsubame, false, 19), NOW)).toBe('19 h');
    expect(expiresWords({ expires_at: '2026-09-29T04:40:00Z' }, NOW)).toBe('40 min');
    expect(expiresWords({ expires_at: '2026-09-29T03:00:00Z' }, NOW)).toBe('0 min');
  });

  it('reads the fresh-sign-in window from the session and this device', () => {
    expect(freshWindow({ fresh_until: '2026-09-29T04:15:00Z' }, { signed_in_at: '2026-09-29T04:00:00Z' })?.minutes).toBe(15);
    expect(freshWindow({ fresh_until: '2026-09-29T04:15:00Z' }, null)).toBeNull();
  });

  it('names the write paths and the return address after a fresh sign-in', () => {
    const t = timing('f-kalos', 4, '22:00');
    expect(writePath({ kind: 'hand', timing: t, to: ren })).toBe('/api/public/timings/f-kalos/owner');
    expect(writePath({ kind: 'ask', timing: t })).toBe('/api/public/timings/f-kalos/owner-requests');
    expect(writePath({ kind: 'withdraw', timing: t, request: ask() })).toBe('/api/public/owner-requests/own-1005/withdraw');
    expect(returnPath({ kind: 'hand', timing: t, to: ren })).toBe('/mine?week=timings&hand=f-kalos&to=1002');
    expect(returnPath({ kind: 'accept', timing: t, request: ask() })).toBe('/mine?week=timings');
  });

  it('says a new owner as Discord does, and a refusal with who it was about', () => {
    const t = timing('f-kalos', 4, '22:00');
    expect(doneWords({ kind: 'hand', timing: t, to: ren })).toBe('👑 Ren now owns weekly timing XKalos · Fri 22:00.');
    expect(doneWords({ kind: 'ask', timing: { ...t, owner: ren } })).toBe('Asked Ren to own XKalos · Fri 22:00. The ask expires in 24 h.');
    const left = new ApiRequestError('http', 'Ownership only moves between members of the party.', 409, { error: 'not_on_party', message: 'Ownership only moves between members of the party.' });
    expect(refusalWords({ kind: 'accept', timing: t, request: ask() }, left)).toBe(
      "Couldn't accept: Tsubame is no longer on the party. Ownership only moves between members of the party.",
    );
    const closed = new ApiRequestError('http', 'That request has already been decided.', 409, { error: 'request_closed', message: 'That request has already been decided.' });
    expect(refusalWords({ kind: 'withdraw', timing: t, request: ask() }, closed)).toBe("Couldn't withdraw: That request has already been decided.");
  });
});

describe('MemberTimingsList', () => {
  const list = (timings: MemberTiming[]): MemberTimings => ({ timings, generated_at: NOW });
  const gone = (error: unknown) => error instanceof ApiRequestError && error.status === 401 && error.kind !== 'reauth';

  function fake(post: (path: string, body: unknown) => unknown) {
    const gets: string[] = [];
    const posts: [string, unknown][] = [];
    const client = {
      get: vi.fn(async (path: string) => {
        gets.push(path);
        return list([timing('f-kalos', 4, '22:00')]);
      }),
      post: vi.fn(async (path: string, body: unknown) => {
        posts.push([path, body]);
        const answer = post(path, body);
        if (answer instanceof Error) throw answer;
        return answer;
      }),
    } as unknown as Client;
    return { client, gets, posts };
  }

  it('hands off with the member id and reads the list again', async () => {
    const { client, gets, posts } = fake(() => ({}));
    const timings = new MemberTimingsList(client, gone);
    const t = timing('f-kalos', 4, '22:00');
    expect(await timings.write({ kind: 'hand', timing: t, to: ren })).toEqual({ done: true });
    expect(posts).toEqual([['/api/public/timings/f-kalos/owner', { to: '1002' }]]);
    expect(gets).toEqual(['/api/public/timings']);
    expect(timings.busy).toBe('');
  });

  it('a fresh-sign-in refusal saves nothing and reads nothing; another refusal reads the list again', async () => {
    const reauth = new ApiRequestError('reauth', 'Sign in with Discord again to make this change.', 401, { error: 'reauth_required', message: '' });
    const first = fake(() => reauth);
    const timings = new MemberTimingsList(first.client, gone);
    const t = timing('f-kalos', 4, '22:00');
    expect(await timings.write({ kind: 'accept', timing: t, request: ask() })).toEqual({ done: false, reauth: true });
    expect(first.gets).toEqual([]);

    const refused = new ApiRequestError('http', 'They already own this timing.', 409, { error: 'already_owner', message: '' });
    const second = fake(() => refused);
    const again = new MemberTimingsList(second.client, gone);
    expect(await again.write({ kind: 'ask', timing: t })).toEqual({ done: false, reauth: false, error: refused });
    expect(second.gets).toEqual(['/api/public/timings']);
  });

  it('an ended session answers null (the portal shows Session ended)', async () => {
    const ended = new ApiRequestError('http', 'Signed out', 401, { error: 'unauthenticated', message: '' });
    const { client } = fake(() => ended);
    const timings = new MemberTimingsList(client, gone);
    await timings.load();
    expect(timings.data?.timings).toHaveLength(1);
    expect(await timings.write({ kind: 'ask', timing: timing('f-kalos', 4, '22:00') })).toBeNull();
  });
});
