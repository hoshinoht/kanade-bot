/** Replaces v4's SSE: one request at a time, a bounded cadence, bounded retries. */

export type PollState = 'idle' | 'running' | 'waiting' | 'backoff' | 'paused' | 'stopped';

export interface VisibilitySource {
  isVisible(): boolean;
  /** Returns an unsubscribe function. */
  subscribe(listener: () => void): () => void;
}

export interface PollOptions<T> {
  task: (signal: AbortSignal) => Promise<T>;
  onData: (value: T) => void;
  onError?: (error: unknown, consecutiveFailures: number) => void;
  onState?: (state: PollState) => void;
  intervalMs: number;
  /** Ceiling for exponential backoff after failures. */
  maxIntervalMs?: number;
  /** Consecutive failures before polling stops until `refresh()`. */
  maxFailures?: number;
  visibility?: VisibilitySource;
}

export interface Poller {
  start(): void;
  stop(): void;
  /** Run now (coalesced with an in-flight request); also restarts a stopped poller. */
  refresh(): Promise<void>;
  readonly state: PollState;
  readonly failures: number;
}

export const MIN_INTERVAL_MS = 1_000;
export const MAX_INTERVAL_MS = 300_000;

export function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}

/** Delay before the next attempt: steady on success, doubling per failure, capped. */
export function nextDelay(intervalMs: number, maxIntervalMs: number, failures: number): number {
  if (failures <= 0) return intervalMs;
  return Math.min(maxIntervalMs, intervalMs * 2 ** Math.min(failures, 16));
}

export function documentVisibility(): VisibilitySource {
  return {
    isVisible: () => typeof document === 'undefined' || document.visibilityState !== 'hidden',
    subscribe(listener) {
      if (typeof document === 'undefined') return () => {};
      document.addEventListener('visibilitychange', listener);
      return () => document.removeEventListener('visibilitychange', listener);
    },
  };
}

export function createPoller<T>(options: PollOptions<T>): Poller {
  const interval = clamp(options.intervalMs, MIN_INTERVAL_MS, MAX_INTERVAL_MS);
  const ceiling = clamp(options.maxIntervalMs ?? interval * 8, interval, MAX_INTERVAL_MS);
  const maxFailures = Math.max(1, options.maxFailures ?? 5);
  const visibility = options.visibility ?? documentVisibility();

  let state: PollState = 'idle';
  let failures = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let inFlight: Promise<void> | null = null;
  let controller: AbortController | null = null;
  let unsubscribe: (() => void) | null = null;
  // Bumped by stop(): a request from an earlier start can neither deliver data
  // nor clear the bookkeeping of the run that replaced it.
  let generation = 0;

  const setState = (next: PollState) => {
    if (state === next) return;
    state = next;
    options.onState?.(next);
  };

  const clearTimer = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };

  const schedule = () => {
    clearTimer();
    if (state === 'stopped' || state === 'idle') return;
    if (!visibility.isVisible()) {
      setState('paused');
      return;
    }
    setState(failures > 0 ? 'backoff' : 'waiting');
    timer = setTimeout(() => void run(), nextDelay(interval, ceiling, failures));
  };

  const run = (): Promise<void> => {
    if (inFlight) return inFlight;
    clearTimer();
    setState('running');
    const mine = generation;
    const own = new AbortController();
    controller = own;
    const signal = own.signal;
    const current: Promise<void> = options
      .task(signal)
      .then(
        (value) => {
          if (signal.aborted || mine !== generation) return;
          failures = 0;
          options.onData(value);
        },
        (error: unknown) => {
          if (signal.aborted || mine !== generation) return;
          failures += 1;
          options.onError?.(error, failures);
          if (failures >= maxFailures) {
            clearTimer();
            setState('stopped');
          }
        },
      )
      .finally(() => {
        if (mine !== generation) return;
        inFlight = null;
        controller = null;
        if (state === 'running') schedule();
      });
    inFlight = current;
    return current;
  };

  const onVisibility = () => {
    if (state === 'idle' || state === 'stopped') return;
    if (visibility.isVisible()) {
      if (state === 'paused') void run();
    } else if (!inFlight) {
      clearTimer();
      setState('paused');
    }
  };

  return {
    start() {
      if (state !== 'idle' && state !== 'stopped') return;
      failures = 0;
      unsubscribe ??= visibility.subscribe(onVisibility);
      setState('waiting');
      if (visibility.isVisible()) void run();
      else setState('paused');
    },
    stop() {
      generation += 1;
      clearTimer();
      controller?.abort();
      controller = null;
      inFlight = null;
      unsubscribe?.();
      unsubscribe = null;
      setState('idle');
    },
    refresh() {
      if (state === 'idle') {
        this.start();
        return inFlight ?? Promise.resolve();
      }
      if (state === 'stopped') {
        failures = 0;
        setState('waiting');
      }
      return run();
    },
    get state() {
      return state;
    },
    get failures() {
      return failures;
    },
  };
}
