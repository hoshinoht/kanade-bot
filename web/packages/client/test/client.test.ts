import { describe, expect, it, vi } from 'vitest';
import { ApiRequestError, createClient } from '../src/client';

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
