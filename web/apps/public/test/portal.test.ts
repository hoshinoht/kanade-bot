import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PublicSession, PublicSessions } from '@kanade/api-types';
import { Portal } from '../src/portal.svelte';

const session: PublicSession = { member: { id: '42', display: 'Rin', avatar: '' }, fresh_until: '2026-10-09T12:00:00Z' };
const devices: PublicSessions = { sessions: [], generated_at: '2026-10-09T11:00:00Z' };

/** The portal's reads answer while `up`; afterwards every request fails at the network. */
function serve() {
  const state = { up: true, paths: [] as string[] };
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      state.paths.push(url);
      if (!state.up) throw new TypeError('Failed to fetch');
      const body = url.endsWith('/status') ? { portal: 'open' } : url.endsWith('/session') ? session : devices;
      return new Response(JSON.stringify(body), { status: 200 });
    }),
  );
  return state;
}

afterEach(() => vi.unstubAllGlobals());

describe('Portal offline retry', () => {
  it('keeps the signed-in Account when the retry finds no network', async () => {
    const net = serve();
    vi.stubGlobal('navigator', { onLine: true });
    const portal = new Portal('none');
    await portal.load();
    expect(portal.screen.kind).toBe('account');
    net.up = false;
    vi.stubGlobal('navigator', { onLine: false });
    const retry = portal.refresh(); // the offline notice's Try again
    expect(portal.screen.kind).toBe('account');
    await retry;
    expect(portal.screen).toEqual({ kind: 'account', session });
    expect(net.paths.slice(-1)).toEqual(['/api/public/session']);
  });

  it('refreshes the session and devices in place once the network is back', async () => {
    const net = serve();
    vi.stubGlobal('navigator', { onLine: true });
    const portal = new Portal('none');
    await portal.load();
    const shown = portal.screen;
    net.paths.length = 0;
    await portal.refresh();
    expect(portal.screen).toBe(shown);
    expect(net.paths).toEqual(['/api/public/session', '/api/public/sessions']);
  });

  it('a second press while a retry runs joins it, so an older answer never lands last', async () => {
    const net = serve();
    vi.stubGlobal('navigator', { onLine: true });
    const portal = new Portal('none');
    await portal.load();
    net.paths.length = 0;
    await Promise.all([portal.refresh(), portal.refresh()]);
    expect(net.paths).toEqual(['/api/public/session', '/api/public/sessions']);
  });
});
