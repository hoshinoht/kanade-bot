/**
 * Change hints over server-sent events: `GET /api/admin/events` (numbered
 * `{topic, seq}` hints) and the member portal's `GET /api/public/events`
 * (`{topic}` only). A hint says only that something of a kind changed;
 * subscribers re-read their own endpoint. Polling stays the fallback:
 * `healthy` tells pollers to slow down while the stream is open and to return
 * to their normal cadence when it drops.
 *
 * A hidden tab holds no stream and runs no hint-driven reads (as polls pause
 * while hidden), so a forgotten tab still reaches the session's idle limit.
 * Shown again it reconnects; the `ready` seq tells whether anything changed
 * meanwhile (a stream without seqs re-reads after every `ready` but the
 * first), and wakes owed from before hiding are delivered then.
 */
import type { EventTopic } from '@kanade/api-types';
import { documentVisibility, type VisibilitySource } from './poll';

/** A `ready` event: the admin stream numbers its hints, the member stream does not. */
interface Ready {
  seq?: number;
  boot: string;
}

interface Hint<T extends string> {
  topic: T;
  seq?: number;
}

/** The part of `EventSource` this module uses (tests pass a fake). */
export interface EventSourceLike {
  readonly readyState: number;
  onopen: ((event: Event) => void) | null;
  onerror: ((event: Event) => void) | null;
  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void;
  close(): void;
}

export type EventSourceFactory = (url: string) => EventSourceLike;

export interface LiveEventsOptions {
  url: string;
  /** Defaults to the browser's `EventSource`; without one the stream never opens. */
  source?: EventSourceFactory | null;
  /** First reconnect delay after the server refused the stream; doubles per failure. */
  retryMs?: number;
  maxRetryMs?: number;
  /** How long a browser-side reconnect may take before polling speeds up again. */
  graceMs?: number;
  /** Hints for one subscriber within this window wake it once. */
  coalesceMs?: number;
  /** Defaults to the document's visibility. */
  visibility?: VisibilitySource;
}

export interface LiveEvents<T extends string = EventTopic> {
  /**
   * Calls `wake` after hints for any of `topics` (coalesced), and after a
   * reconnect that may have missed some. Opens the stream with the first
   * subscriber and closes it after the last. Returns the unsubscribe.
   */
  subscribe(topics: readonly T[], wake: () => void): () => void;
  /** The stream is open: polls may run at their slower fallback cadence. */
  readonly healthy: boolean;
  /** Called on every change of `healthy`; returns the unsubscribe. */
  onHealth(listener: (healthy: boolean) => void): () => void;
  /**
   * Bumped when the server restarted (a new `boot` id), e.g. after a backup
   * restore: versions may have gone down, so readers take their next answer
   * whatever its version.
   */
  readonly epoch: number;
}

const CLOSED = 2;

interface Subscriber<T extends string> {
  topics: ReadonlySet<T>;
  wake: () => void;
  timer: ReturnType<typeof setTimeout> | null;
}

function browserSource(): EventSourceFactory | null {
  const Source = (globalThis as { EventSource?: new (url: string) => EventSourceLike }).EventSource;
  return Source ? (url) => new Source(url) : null;
}

export function createLiveEvents<T extends string = EventTopic>(options: LiveEventsOptions): LiveEvents<T> {
  const factory = options.source === undefined ? browserSource() : options.source;
  const retryMs = options.retryMs ?? 1_000;
  const maxRetryMs = options.maxRetryMs ?? 60_000;
  const graceMs = options.graceMs ?? 5_000;
  const coalesceMs = options.coalesceMs ?? 250;
  const visibility = options.visibility ?? documentVisibility();

  const subscribers = new Set<Subscriber<T>>();
  const healthListeners = new Set<(healthy: boolean) => void>();
  let source: EventSourceLike | null = null;
  let healthy = false;
  let failures = 0;
  let reconnect: ReturnType<typeof setTimeout> | null = null;
  let grace: ReturnType<typeof setTimeout> | null = null;
  /** The last seq heard; null until the first `ready`. */
  let lastSeq: number | null = null;
  let lastBoot: string | null = null;
  let epoch = 0;
  /** Subscribers woken while hidden: they read when the tab is shown. */
  const owed = new Set<Subscriber<T>>();
  let unwatch: (() => void) | null = null;

  const setHealthy = (next: boolean) => {
    if (healthy === next) return;
    healthy = next;
    for (const listener of [...healthListeners]) listener(next);
  };

  const wake = (subscriber: Subscriber<T>) => {
    if (subscriber.timer !== null) return;
    subscriber.timer = setTimeout(() => {
      subscriber.timer = null;
      if (!subscribers.has(subscriber)) return;
      if (visibility.isVisible()) subscriber.wake();
      else owed.add(subscriber);
    }, coalesceMs);
  };

  const wakeTopic = (topic: T) => {
    for (const subscriber of subscribers) if (subscriber.topics.has(topic)) wake(subscriber);
  };

  const wakeAll = () => {
    for (const subscriber of subscribers) wake(subscriber);
  };

  const clearTimers = () => {
    if (reconnect !== null) clearTimeout(reconnect);
    if (grace !== null) clearTimeout(grace);
    reconnect = grace = null;
  };

  const disconnect = () => {
    clearTimers();
    if (source) {
      source.onopen = source.onerror = null;
      source.close();
    }
    source = null;
    setHealthy(false);
  };

  const connect = () => {
    if (source || !factory || subscribers.size === 0 || !visibility.isVisible()) return;
    const opened = factory(options.url);
    source = opened;
    opened.onopen = () => {
      if (grace !== null) clearTimeout(grace);
      grace = null;
      failures = 0;
      setHealthy(true);
    };
    opened.onerror = () => {
      if (source !== opened) return;
      if (opened.readyState === CLOSED) {
        // Refused (signed out, at the cap, not served here): poll, and retry later.
        opened.onopen = opened.onerror = null;
        opened.close();
        source = null;
        setHealthy(false);
        if (grace !== null) clearTimeout(grace);
        grace = null;
        failures += 1;
        reconnect = setTimeout(
          () => {
            reconnect = null;
            connect();
          },
          Math.min(maxRetryMs, retryMs * 2 ** Math.min(failures - 1, 16)),
        );
        return;
      }
      // The browser reconnects by itself (a stream that ended, a blip); only
      // a reconnect that takes too long counts as the stream being down.
      grace ??= setTimeout(() => {
        grace = null;
        setHealthy(false);
      }, graceMs);
    };
    opened.addEventListener('ready', (event) => {
      const { seq, boot } = JSON.parse(event.data) as Ready;
      const first = lastBoot === null;
      const restarted = !first && boot !== lastBoot;
      // The first `ready` cannot tell whether the reads before it came from
      // this process (a restart in between is possible, if unlikely): it opens
      // a new epoch too, so the next read takes whatever version it gets. Read
      // order still keeps late answers out; it costs no extra read.
      if (restarted || first) epoch += 1;
      // A new process or missed hints: whatever is on screen may be stale.
      // Without seqs, any reconnect may have missed some.
      const missed = seq === undefined ? !first : lastSeq !== null && seq !== lastSeq;
      if (restarted || missed) wakeAll();
      lastSeq = seq ?? null;
      lastBoot = boot;
    });
    opened.addEventListener('message', (event) => {
      const hint = JSON.parse(event.data) as Hint<T>;
      if (hint.seq === undefined) {
        wakeTopic(hint.topic);
        return;
      }
      if (lastSeq !== null && hint.seq > lastSeq + 1) wakeAll();
      else wakeTopic(hint.topic);
      lastSeq = Math.max(lastSeq ?? 0, hint.seq);
    });
  };

  const onVisibility = () => {
    if (!visibility.isVisible()) {
      // Pending wakes are owed, not dropped: the tab reads them when shown.
      for (const subscriber of subscribers) {
        if (subscriber.timer === null) continue;
        clearTimeout(subscriber.timer);
        subscriber.timer = null;
        owed.add(subscriber);
      }
      disconnect();
      return;
    }
    for (const subscriber of [...owed]) if (subscribers.has(subscriber)) wake(subscriber);
    owed.clear();
    failures = 0;
    connect();
  };

  return {
    subscribe(topics, callback) {
      const subscriber: Subscriber<T> = { topics: new Set(topics), wake: callback, timer: null };
      subscribers.add(subscriber);
      unwatch ??= visibility.subscribe(onVisibility);
      connect();
      return () => {
        if (subscriber.timer !== null) clearTimeout(subscriber.timer);
        subscribers.delete(subscriber);
        owed.delete(subscriber);
        if (subscribers.size === 0) {
          disconnect();
          unwatch?.();
          unwatch = null;
        }
      };
    },
    get healthy() {
      return healthy;
    },
    get epoch() {
      return epoch;
    },
    onHealth(listener) {
      healthListeners.add(listener);
      return () => healthListeners.delete(listener);
    },
  };
}
