import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createLiveEvents, type EventSourceLike } from '../src/events';
import type { VisibilitySource } from '../src/poll';

/** A scripted `EventSource`: the test opens, fails and feeds it. */
class FakeSource implements EventSourceLike {
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  closed = false;
  #listeners = new Map<string, ((event: MessageEvent<string>) => void)[]>();

  addEventListener(type: string, listener: (event: MessageEvent<string>) => void) {
    this.#listeners.set(type, [...(this.#listeners.get(type) ?? []), listener]);
  }
  close() {
    this.closed = true;
    this.readyState = 2;
  }
  open() {
    this.readyState = 1;
    this.onopen?.(new Event('open'));
  }
  /** The browser lost the stream and is reconnecting by itself. */
  drop() {
    this.readyState = 0;
    this.onerror?.(new Event('error'));
  }
  /** The server refused the stream (401, 429, 404): the browser gives up. */
  refuse() {
    this.readyState = 2;
    this.onerror?.(new Event('error'));
  }
  emit(type: string, data: object) {
    for (const listener of this.#listeners.get(type) ?? []) listener(new MessageEvent(type, { data: JSON.stringify(data) }));
  }
}

function page() {
  let visible = true;
  const listeners = new Set<() => void>();
  const source: VisibilitySource = {
    isVisible: () => visible,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  return {
    source,
    set(next: boolean) {
      visible = next;
      listeners.forEach((l) => l());
    },
    listeners,
  };
}

function setup(visibility = page()) {
  const sources: FakeSource[] = [];
  const events = createLiveEvents({
    visibility: visibility.source,
    url: '/api/admin/events',
    source: () => {
      const source = new FakeSource();
      sources.push(source);
      return source;
    },
    retryMs: 1_000,
    maxRetryMs: 8_000,
    graceMs: 5_000,
    coalesceMs: 100,
  });
  return { events, sources, visibility, last: () => sources[sources.length - 1]! };
}

describe('createLiveEvents', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('opens with the first subscriber, wakes by topic, coalesces and closes after the last', () => {
    const { events, sources, last } = setup();
    const week = vi.fn();
    const chat = vi.fn();
    const stopWeek = events.subscribe(['schedule', 'inbox'], week);
    const stopChat = events.subscribe(['chat'], chat);
    expect(sources).toHaveLength(1);
    last().open();
    last().emit('ready', { seq: 4 });
    last().emit('message', { topic: 'schedule', seq: 5 });
    last().emit('message', { topic: 'inbox', seq: 6 });
    expect(week).not.toHaveBeenCalled();
    vi.advanceTimersByTime(100);
    expect(week).toHaveBeenCalledTimes(1);
    expect(chat).not.toHaveBeenCalled();
    last().emit('message', { topic: 'chat', seq: 7 });
    vi.advanceTimersByTime(100);
    expect(chat).toHaveBeenCalledTimes(1);
    expect(week).toHaveBeenCalledTimes(1);

    stopWeek();
    expect(last().closed).toBe(false);
    stopChat();
    expect(last().closed).toBe(true);
    expect(events.healthy).toBe(false);
  });

  it('wakes everyone after a seq gap or a reconnect that may have missed hints', () => {
    const { events, last } = setup();
    const week = vi.fn();
    const chat = vi.fn();
    events.subscribe(['schedule'], week);
    events.subscribe(['chat'], chat);
    last().open();
    last().emit('ready', { seq: 10 });
    // 11 never arrived (the server's buffer overflowed).
    last().emit('message', { topic: 'schedule', seq: 12 });
    vi.advanceTimersByTime(100);
    expect([week.mock.calls.length, chat.mock.calls.length]).toEqual([1, 1]);

    // Same process, nothing missed: no wake.
    last().drop();
    last().open();
    last().emit('ready', { seq: 12 });
    vi.advanceTimersByTime(100);
    expect([week.mock.calls.length, chat.mock.calls.length]).toEqual([1, 1]);

    // The server restarted (or hints went by while away).
    last().drop();
    last().open();
    last().emit('ready', { seq: 3 });
    vi.advanceTimersByTime(100);
    expect([week.mock.calls.length, chat.mock.calls.length]).toEqual([2, 2]);
  });

  it('is healthy while open, rides out a quick reconnect and reports a slow one', () => {
    const { events, last } = setup();
    const health: boolean[] = [];
    events.onHealth((h) => health.push(h));
    events.subscribe(['schedule'], () => {});
    expect(events.healthy).toBe(false);
    last().open();
    expect(events.healthy).toBe(true);
    last().drop();
    vi.advanceTimersByTime(4_000);
    last().open();
    expect(events.healthy).toBe(true);
    last().drop();
    vi.advanceTimersByTime(5_000);
    expect(events.healthy).toBe(false);
    last().open();
    expect(health).toEqual([true, false, true]);
  });

  it('retries a refused stream with backoff, never faster than the browser would hammer it', () => {
    const { events, sources, last } = setup();
    events.subscribe(['schedule'], () => {});
    last().refuse();
    expect(events.healthy).toBe(false);
    expect(sources[0]!.closed).toBe(true);
    vi.advanceTimersByTime(999);
    expect(sources).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sources).toHaveLength(2);
    last().refuse();
    vi.advanceTimersByTime(1_999);
    expect(sources).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sources).toHaveLength(3);
    last().open();
    expect(events.healthy).toBe(true);
    // Open again: the next refusal starts from the shortest delay.
    last().refuse();
    vi.advanceTimersByTime(1_000);
    expect(sources).toHaveLength(4);
  });

  it('without EventSource never opens and stays unhealthy (polling only)', () => {
    const events = createLiveEvents({ url: '/api/admin/events', source: null });
    const wake = vi.fn();
    const stop = events.subscribe(['schedule'], wake);
    expect(events.healthy).toBe(false);
    stop();
  });

  it('a hidden tab holds no stream and reads nothing; shown again it reconnects and catches up', () => {
    const { events, sources, visibility, last } = setup();
    const week = vi.fn();
    const chat = vi.fn();
    const stop = events.subscribe(['schedule'], week);
    events.subscribe(['chat'], chat);
    last().open();
    last().emit('ready', { seq: 1 });
    // A hint lands just before the tab is hidden: owed, not read while hidden.
    last().emit('message', { topic: 'schedule', seq: 2 });
    visibility.set(false);
    expect(sources[0]!.closed).toBe(true);
    expect(events.healthy).toBe(false);
    vi.advanceTimersByTime(60_000);
    expect(sources).toHaveLength(1);
    expect(week).not.toHaveBeenCalled();

    // Shown: the owed wake and a fresh stream; changes made meanwhile wake everyone.
    visibility.set(true);
    expect(sources).toHaveLength(2);
    vi.advanceTimersByTime(100);
    expect(week).toHaveBeenCalledTimes(1);
    expect(chat).not.toHaveBeenCalled();
    last().open();
    last().emit('ready', { seq: 5 });
    vi.advanceTimersByTime(100);
    expect([week.mock.calls.length, chat.mock.calls.length]).toEqual([2, 1]);

    // Hidden again with nothing pending, shown with nothing new: no reads.
    visibility.set(false);
    visibility.set(true);
    last().open();
    last().emit('ready', { seq: 5 });
    vi.advanceTimersByTime(100);
    expect([week.mock.calls.length, chat.mock.calls.length]).toEqual([2, 1]);
    stop();
  });

  it('never opens while the tab starts hidden, and stops watching visibility after the last subscriber', () => {
    const visibility = page();
    visibility.set(false);
    const { events, sources } = setup(visibility);
    const stop = events.subscribe(['schedule'], () => {});
    expect(sources).toHaveLength(0);
    visibility.set(true);
    expect(sources).toHaveLength(1);
    stop();
    expect(visibility.listeners.size).toBe(0);
  });
});
