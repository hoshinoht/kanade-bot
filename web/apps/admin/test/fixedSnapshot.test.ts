import { afterEach, describe, expect, it, vi } from 'vitest';
import type { FixedRow } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import { FixedSnapshot } from '../src/fixed/snapshot.svelte';
import { send } from '../src/resource.svelte';

const row = (note: string) => ({ id: 'f-bm', note }) as unknown as FixedRow;

/** A client whose GETs wait until the test answers them, in any order. */
function gated() {
  const pending: { path: string; resolve: (v: unknown) => void; reject: (e: unknown) => void }[] = [];
  const get = vi.fn(
    (path: string) =>
      new Promise((resolve, reject) => {
        pending.push({ path, resolve, reject });
      }),
  );
  const client = { get } as unknown as Client;
  /** Answers the oldest waiting request for `path`. */
  const answer = async (path: string, value: unknown, fail = false) => {
    const i = pending.findIndex((p) => p.path === path);
    if (i < 0) throw new Error(`nothing waiting for ${path}`);
    const [p] = pending.splice(i, 1);
    if (fail) p!.reject(value);
    else p!.resolve(value);
    // Let the load's continuation run (and issue its next request).
    await new Promise((r) => setTimeout(r, 0));
  };
  return { client, answer };
}

describe('FixedSnapshot', () => {
  it('publishes the week version only together with the rows read after it', async () => {
    const { client, answer } = gated();
    const snap = new FixedSnapshot(client);
    const first = snap.load();
    await answer('/api/admin/week', { version: 5 });
    await answer('/api/admin/fixed', [row('old')]);
    await first;
    expect([snap.version, snap.rows?.[0]?.note]).toEqual([5, 'old']);

    // A reload after a 409: the new version must not sit beside the old rows
    // while the list is still on its way (an editor opened now would pin it).
    const again = snap.load();
    await answer('/api/admin/week', { version: 7 });
    expect([snap.version, snap.rows?.[0]?.note]).toEqual([5, 'old']);
    await answer('/api/admin/fixed', [row('new')]);
    await again;
    expect([snap.version, snap.rows?.[0]?.note]).toEqual([7, 'new']);
  });

  it('lets a newer load win; the overtaken one publishes nothing', async () => {
    const { client, answer } = gated();
    const snap = new FixedSnapshot(client);
    const slow = snap.load();
    await answer('/api/admin/week', { version: 5 });
    const fast = snap.load();
    await answer('/api/admin/week', { version: 6 });
    // Both lists are in flight; the newer answers first.
    await answer('/api/admin/fixed', [row('from the older load')]);
    expect(snap.version).toBeNull();
    await answer('/api/admin/fixed', [row('from the newer load')]);
    expect(await fast).toBe(true);
    expect(await slow).toBe(false);
    expect([snap.version, snap.rows?.[0]?.note]).toEqual([6, 'from the newer load']);
  });

  it('keeps the last consistent pair when a reload fails', async () => {
    const { client, answer } = gated();
    const snap = new FixedSnapshot(client);
    const ok = snap.load();
    await answer('/api/admin/week', { version: 5 });
    await answer('/api/admin/fixed', [row('old')]);
    await ok;
    const failing = snap.load();
    await answer('/api/admin/week', { version: 8 });
    await answer('/api/admin/fixed', new ApiRequestError('network', 'Network unavailable'), true);
    await failing;
    expect([snap.version, snap.rows?.[0]?.note, snap.error]).toEqual([5, 'old', 'Network unavailable']);
  });
});

describe('send', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('carries the error code, so 409 busy is not mistaken for stale', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('{"error":"busy","message":"Another change landed at the same moment; try again."}', { status: 409 })),
    );
    const result = await send((c) => c.get('/api/admin/fixed'));
    expect(result).toMatchObject({ ok: false, status: 409, code: 'busy' });
  });
});
