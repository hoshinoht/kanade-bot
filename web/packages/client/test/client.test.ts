import { describe, expect, it, vi } from 'vitest';
import { ApiRequestError, createClient, createCsrfGuard, onUnauthenticated } from '../src/client';

function respond(status: number, body: string): typeof fetch {
  return vi.fn(async () => new Response(body, { status })) as unknown as typeof fetch;
}

describe('createClient', () => {
  it('sends same-origin, uncached JSON requests', async () => {
    const fetchMock = respond(200, '{"ok":true}');
    const client = createClient({ fetch: fetchMock });
    await expect(client.post('/api/x', { a: 1 })).resolves.toEqual({ ok: true });
    const [url, init] = (fetchMock as unknown as ReturnType<typeof vi.fn>).mock.calls[0]!;
    expect(url).toBe('/api/x');
    expect(init).toMatchObject({ method: 'POST', cache: 'no-store', credentials: 'same-origin', body: '{"a":1}' });
  });

  it('maps API errors, parse errors and network failures to typed errors', async () => {
    const http = createClient({ fetch: respond(409, '{"error":"stale","message":"The week changed."}') });
    await expect(http.get('/x')).rejects.toMatchObject({ kind: 'http', status: 409, message: 'The week changed.' });

    const parse = createClient({ fetch: respond(200, '<html>') });
    await expect(parse.get('/x')).rejects.toMatchObject({ kind: 'parse' });

    const down = createClient({ fetch: vi.fn(async () => Promise.reject(new TypeError('Failed to fetch'))) as unknown as typeof fetch });
    const error = await down.get('/x').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiRequestError);
    expect((error as ApiRequestError).kind).toBe('network');
    expect((error as ApiRequestError).transient).toBe(true);
  });

  it('times out slow requests', async () => {
    const slow = vi.fn(
      (_url: string, init: RequestInit) =>
        new Promise<Response>((_resolve, reject) => init.signal!.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')))),
    ) as unknown as typeof fetch;
    const client = createClient({ fetch: slow, timeoutMs: 20 });
    await expect(client.get('/x')).rejects.toMatchObject({ kind: 'timeout' });
  });
});

type Answer = { status: number; body?: string; csrf?: string } | 'offline';

/** Answers in order and records what each request sent. */
function script(...answers: Answer[]) {
  const sent: { method: string; url: string; headers: Record<string, string> }[] = [];
  const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
    sent.push({ method: init.method ?? 'GET', url, headers: init.headers as Record<string, string> });
    const answer = answers.shift();
    if (!answer) throw new Error(`unexpected ${init.method} ${url}`);
    if (answer === 'offline') throw new TypeError('Failed to fetch');
    const headers = answer.csrf ? { 'X-Kanade-CSRF': answer.csrf } : undefined;
    return new Response(answer.body ?? '{}', { status: answer.status, headers });
  }) as unknown as typeof fetch;
  const csrf = createCsrfGuard('/api/admin/session', (path) => path.startsWith('/api/admin/') && !path.startsWith('/api/admin/auth/token'));
  return { client: createClient({ fetch: fetchMock, csrf }), sent, csrf };
}

const refused = '{"error":"csrf","message":"The request did not come from this portal."}';

describe('admin writes', () => {
  it('reads the CSRF token from the session before the first write and sends a key', async () => {
    const { client, sent } = script({ status: 200, body: '{"display":"a"}', csrf: 't1' }, { status: 200 });
    await client.post('/api/admin/runs/r1/move', { day: 1 });
    expect(sent.map((r) => `${r.method} ${r.url}`)).toEqual(['GET /api/admin/session', 'POST /api/admin/runs/r1/move']);
    expect(sent[1]!.headers['X-Kanade-CSRF']).toBe('t1');
    expect(sent[1]!.headers['Idempotency-Key']).toMatch(/^[A-Za-z0-9._:-]{1,128}$/);
  });

  it('gives every action its own key and leaves reads alone', async () => {
    const { client, sent, csrf } = script({ status: 200 }, { status: 200 }, { status: 200 });
    csrf.token = 't1';
    await client.patch('/api/admin/fixed/f1', {});
    await client.delete('/api/admin/fixed/f1');
    await client.get('/api/admin/week');
    expect(sent[0]!.headers['Idempotency-Key']).not.toBe(sent[1]!.headers['Idempotency-Key']);
    expect(sent[2]!.headers).not.toHaveProperty('Idempotency-Key');
    expect(sent[2]!.headers).not.toHaveProperty('X-Kanade-CSRF');
  });

  it('refreshes a refused token once and retries the same action', async () => {
    const { client, sent, csrf } = script({ status: 403, body: refused }, { status: 200, csrf: 't2' }, { status: 200, body: '{"ok":true}' });
    csrf.token = 'old';
    await expect(client.post('/api/admin/runs/r1/rsvp', {})).resolves.toEqual({ ok: true });
    expect(sent.map((r) => r.method)).toEqual(['POST', 'GET', 'POST']);
    expect([sent[0]!.headers['X-Kanade-CSRF'], sent[2]!.headers['X-Kanade-CSRF']]).toEqual(['old', 't2']);
    expect(sent[2]!.headers['Idempotency-Key']).toBe(sent[0]!.headers['Idempotency-Key']);
  });

  it('does not loop when the refreshed token is refused too', async () => {
    const { client, sent, csrf } = script({ status: 403, body: refused }, { status: 200, csrf: 't2' }, { status: 403, body: refused });
    csrf.token = 'old';
    await expect(client.patch('/api/admin/members/m1', {})).rejects.toMatchObject({ status: 403, body: { error: 'csrf' } });
    expect(sent).toHaveLength(3);
  });

  it('retries a network failure once with the same key', async () => {
    const { client, sent, csrf } = script('offline', { status: 200, body: '{"ok":true}' });
    csrf.token = 't1';
    await expect(client.post('/api/admin/fixed', {}, { idempotencyKey: 'form:1' })).resolves.toEqual({ ok: true });
    expect(sent.map((r) => r.headers['Idempotency-Key'])).toEqual(['form:1', 'form:1']);
  });

  it('sends no CSRF token outside the guarded paths and needs no session for them', async () => {
    const { client, sent } = script({ status: 200 }, { status: 200, csrf: 't9' });
    await client.post('/api/public/requests', {});
    await client.post('/api/admin/auth/token', { token: 'x' });
    expect(sent.map((r) => r.url)).toEqual(['/api/public/requests', '/api/admin/auth/token']);
    expect(sent.every((r) => !('X-Kanade-CSRF' in r.headers) && r.headers['Idempotency-Key'])).toBe(true);
  });

  it('sends keys but no token when the app installed no guard', async () => {
    const fetchMock = respond(200, '{}');
    await createClient({ fetch: fetchMock }).post('/api/admin/runs/r1/move', {});
    const calls = (fetchMock as unknown as ReturnType<typeof vi.fn>).mock.calls as [string, RequestInit][];
    expect(calls).toHaveLength(1);
    expect(calls[0]![1].headers).toHaveProperty('Idempotency-Key');
    expect(calls[0]![1].headers).not.toHaveProperty('X-Kanade-CSRF');
  });

  it('tells the page of a 401, once per refused request', async () => {
    const seen: string[] = [];
    onUnauthenticated((path) => seen.push(path));
    try {
      const client = createClient({ fetch: respond(401, '{"error":"unauthenticated","message":"Sign in to continue."}') });
      await expect(client.get('/api/admin/week')).rejects.toMatchObject({ status: 401 });
      expect(seen).toEqual(['/api/admin/week']);
    } finally {
      onUnauthenticated(null);
    }
  });
});

describe('member session (public origin)', () => {
  function member(...answers: Answer[]) {
    const sent: { method: string; url: string; headers: Record<string, string> }[] = [];
    const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
      sent.push({ method: init.method ?? 'GET', url, headers: init.headers as Record<string, string> });
      const answer = answers.shift();
      if (!answer || answer === 'offline') throw new Error(`unexpected ${init.method} ${url}`);
      const headers = answer.csrf ? { 'X-Kanade-CSRF': answer.csrf } : undefined;
      return new Response(answer.body ?? (answer.status === 204 ? null : '{}'), { status: answer.status, headers });
    }) as unknown as typeof fetch;
    const csrf = createCsrfGuard('/api/public/session', (path) => path.startsWith('/api/public/'));
    return { client: createClient({ fetch: fetchMock, csrf }), sent };
  }

  it('sends the newest token any answer carried, a write included', async () => {
    // The session read issues t1; a list read rotated it to t2; a sign-out rotated it again to t3.
    const { client, sent } = member({ status: 200, csrf: 't1' }, { status: 200, csrf: 't2' }, { status: 204, csrf: 't3' }, { status: 204 });
    await client.get('/api/public/session');
    await client.get('/api/public/sessions');
    await client.delete('/api/public/sessions/aaaaaaaaaaaaaaaaaaaaaaaa');
    await client.delete('/api/public/sessions/bbbbbbbbbbbbbbbbbbbbbbbb');
    expect(sent.map((r) => r.headers['X-Kanade-CSRF'])).toEqual([undefined, undefined, 't2', 't3']);
  });

  it('treats reauth_required as "confirm it is you", not as a session that ended', async () => {
    const seen: string[] = [];
    onUnauthenticated((path) => seen.push(path));
    try {
      const { client } = member(
        { status: 200, csrf: 't1' },
        { status: 401, body: '{"error":"reauth_required","message":"Sign in with Discord again to make this change."}' },
        { status: 401, body: '{"error":"unauthenticated","message":"Sign in to continue."}' },
      );
      await client.get('/api/public/session');
      const fresh = await client.post('/api/public/requests', {}).catch((e: unknown) => e);
      expect(fresh).toMatchObject({ kind: 'reauth', status: 401, body: { error: 'reauth_required' } });
      expect(seen).toEqual([]);
      await expect(client.get('/api/public/sessions')).rejects.toMatchObject({ kind: 'http', status: 401, body: { error: 'unauthenticated' } });
      expect(seen).toEqual(['/api/public/sessions']);
    } finally {
      onUnauthenticated(null);
    }
  });
});

describe('conditional reads', () => {
  it('revalidates a tagged read and hands back the same object on 304', async () => {
    const answers = [
      new Response('{"week":1}', { status: 200, headers: { ETag: '"a"' } }),
      new Response(null, { status: 304, headers: { ETag: '"a"' } }),
      new Response('{"week":2}', { status: 200, headers: { ETag: '"b"' } }),
      new Response('{"week":3}', { status: 200 }),
      new Response('{"week":4}', { status: 200 }),
    ];
    const fetchMock = vi.fn(async () => answers.shift()!);
    const client = createClient({ fetch: fetchMock as unknown as typeof fetch });
    const first = await client.get<{ week: number }>('/api/admin/week');
    const again = await client.get<{ week: number }>('/api/admin/week');
    expect(again).toBe(first);
    expect(await client.get('/api/admin/week')).toEqual({ week: 2 });
    // An untagged answer forgets the tag: the next read is unconditional.
    expect(await client.get('/api/admin/week')).toEqual({ week: 3 });
    await client.get('/api/admin/week');
    const sent = fetchMock.mock.calls.map((call) => ((call as unknown as [string, RequestInit])[1].headers as Record<string, string>)['If-None-Match']);
    expect(sent).toEqual([undefined, '"a"', '"a"', '"b"', undefined]);
  });

  it('never makes writes conditional', async () => {
    const fetchMock = vi.fn(async () => new Response('{"ok":true}', { status: 200, headers: { ETag: '"a"' } }));
    const client = createClient({ fetch: fetchMock as unknown as typeof fetch, csrf: createCsrfGuard('/s', () => false) });
    await client.get('/x');
    await client.post('/x', {});
    const headers = (fetchMock.mock.calls[1] as unknown as [string, RequestInit])[1].headers as Record<string, string>;
    expect(headers['If-None-Match']).toBeUndefined();
  });
});
