import { afterEach, describe, expect, it, vi } from 'vitest';
import type { EventTopic } from '@kanade/api-types';
import type { LiveEvents } from '@kanade/client';
import { arrival, pinnedFirst, Resource } from '../src/resource.svelte';

type Doc = { version: number; label: string };

/** Answers GETs from a queue the test resolves in any order. */
function server() {
  const pending: { resolve: (body: unknown) => void; reject: (status: number) => void }[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          pending.push({
            resolve: (body) => resolve(new Response(JSON.stringify(body), { status: 200 })),
            reject: (status) => resolve(new Response(JSON.stringify({ error: 'unavailable', message: 'down' }), { status })),
          });
        }),
    ),
  );
  return pending;
}

/** A hint source the test fires by hand. */
function hints() {
  const subscribers = new Set<{ topics: readonly EventTopic[]; wake: () => void }>();
  let epoch = 0;
  const events: LiveEvents = {
    subscribe(topics, wake) {
      const entry = { topics, wake };
      subscribers.add(entry);
      return () => subscribers.delete(entry);
    },
    healthy: true,
    onHealth: () => () => {},
    get epoch() {
      return epoch;
    },
  };
  return {
    events,
    restart: () => (epoch += 1),
    fire: (topic: EventTopic) => [...subscribers].filter((s) => s.topics.includes(topic)).forEach((s) => s.wake()),
    count: () => subscribers.size,
  };
}

const settle = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

afterEach(() => vi.unstubAllGlobals());

describe('Resource refresh', () => {
  it('never lets an out-of-order answer roll back what a later request showed', async () => {
    const pending = server();
    const doc = new Resource<Doc>('/api/admin/doc');
    void doc.load();
    pending[0]!.resolve({ version: 1, label: 'first' });
    await settle();
    const older = doc.refresh();
    const newer = doc.refresh();
    pending[2]!.resolve({ version: 3, label: 'newer' });
    await newer;
    pending[1]!.resolve({ version: 2, label: 'older' });
    await older;
    expect(doc.data?.label).toBe('newer');
  });

  it('with a version, an answer older than the one shown never applies', async () => {
    const pending = server();
    const doc = new Resource<Doc>('/api/admin/doc', { version: (d) => d.version });
    void doc.load();
    pending[0]!.resolve({ version: 5, label: 'v5' });
    await settle();
    // A later request served by a lagging read still carries an older version.
    void doc.refresh();
    pending[1]!.resolve({ version: 4, label: 'v4' });
    await settle();
    expect(doc.data?.label).toBe('v5');
  });

  it('a local write (a save answer) beats any read already in flight', async () => {
    const pending = server();
    const doc = new Resource<Doc>('/api/admin/doc');
    void doc.load();
    pending[0]!.resolve({ version: 1, label: 'loaded' });
    await settle();
    void doc.refresh();
    doc.data = { version: 2, label: 'saved' };
    pending[1]!.resolve({ version: 1, label: 'stale read' });
    await settle();
    expect(doc.data?.label).toBe('saved');
  });

  it('refreshes silently: no loading state, and a failure keeps the data without an error', async () => {
    const pending = server();
    const doc = new Resource<Doc>('/api/admin/doc');
    void doc.load();
    expect(doc.loading).toBe(true);
    pending[0]!.resolve({ version: 1, label: 'shown' });
    await settle();
    expect(doc.loading).toBe(false);
    const shown = doc.data;
    void doc.refresh();
    expect([doc.loading, doc.refreshing]).toEqual([false, true]);
    pending[1]!.reject(503);
    await settle();
    expect([doc.loading, doc.refreshing, doc.error]).toEqual([false, false, '']);
    expect(doc.data).toBe(shown);
    // An equal answer keeps the very objects on screen (nothing re-renders).
    void doc.refresh();
    pending[2]!.resolve({ version: 1, label: 'shown' });
    await settle();
    expect(doc.data).toBe(shown);
  });

  it('watch() loads, refreshes on its topics only, and stops following on cleanup', async () => {
    const pending = server();
    const live = hints();
    const doc = new Resource<Doc>('/api/admin/doc', { topics: ['inbox'], events: live.events });
    const stop = doc.watch();
    pending[0]!.resolve({ version: 1, label: 'one' });
    await settle();
    live.fire('chat');
    expect(pending).toHaveLength(1);
    live.fire('inbox');
    expect(pending).toHaveLength(2);
    pending[1]!.resolve({ version: 2, label: 'two' });
    await settle();
    expect(doc.data?.label).toBe('two');
    stop();
    expect(live.count()).toBe(0);
  });
});

describe('Resource after a server restart', () => {
  it('takes a lower version once the stream reports a restart (a restore lowered the head)', async () => {
    const pending = server();
    const live = hints();
    const doc = new Resource<Doc>('/api/admin/doc', { topics: ['schedule'], version: (d) => d.version, events: live.events });
    doc.watch();
    pending[0]!.resolve({ version: 9, label: 'before the restore' });
    await settle();
    live.fire('schedule');
    pending[1]!.resolve({ version: 4, label: 'restored' });
    await settle();
    expect(doc.data?.label).toBe('before the restore');
    live.restart();
    live.fire('schedule');
    pending[2]!.resolve({ version: 4, label: 'restored' });
    await settle();
    expect(doc.data?.label).toBe('restored');
    // From then on the usual rule holds again.
    live.fire('schedule');
    pending[3]!.resolve({ version: 3, label: 'late' });
    await settle();
    expect(doc.data?.label).toBe('restored');
  });

  it('an explicit load takes what the server says, whatever its version', async () => {
    const pending = server();
    const doc = new Resource<Doc>('/api/admin/doc', { version: (d) => d.version });
    void doc.load();
    pending[0]!.resolve({ version: 9, label: 'nine' });
    await settle();
    void doc.load();
    pending[1]!.resolve({ version: 2, label: 'two' });
    await settle();
    expect(doc.data?.label).toBe('two');
  });
});

describe('Resource arrivals', () => {
  type List = { id: string }[];
  it('marks rows a hint brought in and counts the arrival; own re-reads mark nothing', async () => {
    const pending = server();
    const live = hints();
    const list = new Resource<List>('/api/admin/list', { topics: ['inbox'], keys: (rows) => rows.map((r) => r.id), events: live.events });
    list.watch();
    pending[0]!.resolve([{ id: 'a' }]);
    await settle();
    expect([...list.fresh]).toEqual([]);
    const seq = arrival.seq;

    live.fire('inbox');
    pending[1]!.resolve([{ id: 'b' }, { id: 'a' }]);
    await settle();
    expect([...list.fresh]).toEqual(['b']);
    expect(arrival.seq).toBe(seq + 1);

    // Unchanged: nothing new, nothing counted.
    live.fire('inbox');
    pending[2]!.resolve([{ id: 'b' }, { id: 'a' }]);
    await settle();
    expect(arrival.seq).toBe(seq + 1);

    // The page's own re-read (after its own decision) is not an arrival.
    void list.refresh();
    pending[3]!.resolve([{ id: 'c' }, { id: 'b' }, { id: 'a' }]);
    await settle();
    expect([...list.fresh]).toEqual([]);
    expect(arrival.seq).toBe(seq + 1);
  });
});

describe('Resource arrival marks', () => {
  afterEach(() => vi.useRealTimers());

  it('empties `fresh` once the mark has played, so a list mounted later is not marked again', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout'] });
    const pending = server();
    const live = hints();
    const list = new Resource<{ id: string }[]>('/api/admin/list', { topics: ['inbox'], keys: (rows) => rows.map((r) => r.id), events: live.events });
    list.watch();
    pending[0]!.resolve([{ id: 'a' }]);
    await settle();
    live.fire('inbox');
    pending[1]!.resolve([{ id: 'b' }, { id: 'a' }]);
    await settle();
    expect([...list.fresh]).toEqual(['b']);
    vi.advanceTimersByTime(2_000);
    expect([...list.fresh]).toEqual(['b']);
    vi.advanceTimersByTime(200);
    expect([...list.fresh]).toEqual([]);
  });
});

describe('pinnedFirst', () => {
  it('keeps the default row open when a refresh adds rows above it, until the set or page changes', () => {
    let set = 'a';
    const first = pinnedFirst(() => set);
    const ids = (...list: string[]) => list.map((id) => ({ id }));
    expect(first(ids('x', 'y'), 1)).toBe('x');
    expect(first(ids('new', 'x', 'y'), 1)).toBe('x');
    expect(first(ids('new', 'y'), 1)).toBe('new');
    expect(first(ids('p', 'q'), 2)).toBe('p');
    set = 'b';
    expect(first(ids('r', 'p'), 2)).toBe('r');
  });
});
