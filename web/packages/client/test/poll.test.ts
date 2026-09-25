import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPoller, MAX_INTERVAL_MS, MIN_INTERVAL_MS, nextDelay, type VisibilitySource } from '../src/poll';

function visibility(initial = true) {
  let visible = initial;
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

const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

describe('nextDelay', () => {
  it('is steady on success and doubles per failure up to the ceiling', () => {
    expect(nextDelay(1000, 8000, 0)).toBe(1000);
    expect(nextDelay(1000, 8000, 1)).toBe(2000);
    expect(nextDelay(1000, 8000, 3)).toBe(8000);
    expect(nextDelay(1000, 8000, 50)).toBe(8000);
  });
});

describe('createPoller', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('polls on its interval and never overlaps requests', async () => {
    const vis = visibility();
    let inFlight = 0;
    let maxInFlight = 0;
    const resolvers: (() => void)[] = [];
    const task = vi.fn(
      () =>
        new Promise<number>((resolve) => {
          inFlight++;
          maxInFlight = Math.max(maxInFlight, inFlight);
          resolvers.push(() => {
            inFlight--;
            resolve(1);
          });
        }),
    );
    const onData = vi.fn();
    const poller = createPoller({ task, onData, intervalMs: 5000, visibility: vis.source });
    poller.start();
    expect(task).toHaveBeenCalledTimes(1);

    // A manual refresh while a request is in flight coalesces into it.
    void poller.refresh();
    expect(task).toHaveBeenCalledTimes(1);

    resolvers.shift()!();
    await flush();
    expect(onData).toHaveBeenCalledTimes(1);
    expect(poller.state).toBe('waiting');

    await vi.advanceTimersByTimeAsync(4999);
    expect(task).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(task).toHaveBeenCalledTimes(2);
    expect(maxInFlight).toBe(1);
    poller.stop();
  });

  it('clamps the interval to its bounds', async () => {
    const vis = visibility();
    const task = vi.fn(async () => 1);
    const poller = createPoller({ task, onData: () => {}, intervalMs: 10, visibility: vis.source });
    poller.start();
    await flush();
    await vi.advanceTimersByTimeAsync(MIN_INTERVAL_MS - 1);
    expect(task).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(task).toHaveBeenCalledTimes(2);
    poller.stop();
    expect(MAX_INTERVAL_MS).toBe(300_000);
  });

  it('backs off exponentially and stops after maxFailures until refreshed', async () => {
    const vis = visibility();
    const task = vi.fn(async () => {
      throw new Error('down');
    });
    const onError = vi.fn();
    const states: string[] = [];
    const poller = createPoller({
      task,
      onData: () => {},
      onError,
      onState: (s) => states.push(s),
      intervalMs: 1000,
      maxIntervalMs: 4000,
      maxFailures: 3,
      visibility: vis.source,
    });
    poller.start();
    await flush();
    expect(poller.state).toBe('backoff');
    await vi.advanceTimersByTimeAsync(2000); // 1 failure -> 2 s
    await flush();
    await vi.advanceTimersByTimeAsync(4000); // 2 failures -> 4 s (capped)
    await flush();
    expect(task).toHaveBeenCalledTimes(3);
    expect(poller.state).toBe('stopped');
    await vi.advanceTimersByTimeAsync(60_000);
    expect(task).toHaveBeenCalledTimes(3);
    expect(onError).toHaveBeenLastCalledWith(expect.any(Error), 3);

    task.mockImplementation(async () => {
      throw new Error('still down');
    });
    await poller.refresh();
    expect(task).toHaveBeenCalledTimes(4);
    expect(states).toContain('stopped');
    poller.stop();
  });

  it('pauses while hidden and fetches immediately when visible again', async () => {
    const vis = visibility();
    const task = vi.fn(async () => 1);
    const poller = createPoller({ task, onData: () => {}, intervalMs: 1000, visibility: vis.source });
    poller.start();
    await flush();
    vis.set(false);
    expect(poller.state).toBe('paused');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(task).toHaveBeenCalledTimes(1);
    vis.set(true);
    await flush();
    expect(task).toHaveBeenCalledTimes(2);
    poller.stop();
    expect(vis.listeners.size).toBe(0);
  });

  it('aborts the in-flight request on stop and ignores its result', async () => {
    const vis = visibility();
    let signal: AbortSignal | undefined;
    const onData = vi.fn();
    const poller = createPoller({
      task: (s) => {
        signal = s;
        return new Promise<number>((resolve) => setTimeout(() => resolve(1), 500));
      },
      onData,
      intervalMs: 1000,
      visibility: vis.source,
    });
    poller.start();
    poller.stop();
    expect(signal?.aborted).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(onData).not.toHaveBeenCalled();
    expect(poller.state).toBe('idle');
  });

  it('restarts cleanly when stopped and started while a request is in flight', async () => {
    const vis = visibility();
    const resolvers: ((v: number) => void)[] = [];
    const task = vi.fn(() => new Promise<number>((resolve) => resolvers.push(resolve)));
    const onData = vi.fn();
    const poller = createPoller({ task, onData, intervalMs: 1000, visibility: vis.source });
    poller.start();
    poller.stop();
    poller.start();
    // The restart issues its own request rather than waiting on the aborted one.
    expect(task).toHaveBeenCalledTimes(2);
    resolvers[0]!(1); // the stale request settles late
    await flush();
    expect(onData).not.toHaveBeenCalled();
    expect(poller.state).toBe('running');
    resolvers[1]!(2);
    await flush();
    expect(onData).toHaveBeenCalledWith(2);
    expect(poller.state).toBe('waiting');
    await vi.advanceTimersByTimeAsync(1000);
    expect(task).toHaveBeenCalledTimes(3);
    poller.stop();
  });
});

