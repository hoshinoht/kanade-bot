import type { EventTopic } from '@kanade/api-types';
import { ApiRequestError, createClient, createLiveEvents, wroteWithin, type Client, type LiveEvents } from '@kanade/client';
import { ARRIVAL_FALLBACK_MS } from '@kanade/ui';

/** The server's generic 404 for a route it does not serve (an unbuilt feature, not a missing item). */
const UNBUILT = 'No such endpoint on this origin.';

/** An error in words; a route this server has not built yet is not an outage. */
export function errorText(error: ApiRequestError): string {
  if (error.status === 404 && error.body?.message === UNBUILT) return "This isn't available on this server yet.";
  return error.message;
}

/** The admin app's one change-hint stream (`GET /api/admin/events`), opened while anything listens. */
export const live: LiveEvents = createLiveEvents({ url: '/api/admin/events' });

/**
 * Bumped whenever data from elsewhere lands on screen (a hint or a poll that
 * changed something); the user's own writes and explicit reloads never bump
 * it. Number ticks pulse only when a value changed together with it.
 */
export const arrival = $state({ seq: 0 });

/** Changes read this soon after the page's own write are its echo, not an arrival. */
export const OWN_ECHO_MS = 3_000;

export interface ResourceOptions<T> {
  /** Hints that make an open page re-read this resource. */
  topics?: readonly EventTopic[];
  /**
   * A version that only grows (history head, `version`): an older read never
   * replaces a newer one, except right after a server restart (a restore may
   * lower it) or on an explicit `load()`.
   */
  version?: (data: T) => number;
  /** Row ids, so rows a hint brought in can be marked new (`fresh`). */
  keys?: (data: T) => Iterable<string>;
  client?: Client;
  events?: LiveEvents;
}

/** A row-id set that is replaced whole, never changed in place. */
function ids(keys: Iterable<string>): ReadonlySet<string> {
  // Replaced whole on each arrival, never mutated, so a plain Set is enough.
  return new Set(keys);
}

/** How an answer came: the page asked (`load`, a quiet `refresh`) or a hint woke it. */
type Origin = 'asked' | 'hinted';

/**
 * A page's read model: loaded when the page opens, reloaded after its own
 * edits, and refreshed in place when a live hint (or a reconnect) says it
 * may have changed elsewhere. A response applies only if it is newer than
 * what is shown: requests are numbered, and a later request's answer, a local
 * write (`data = …`) or a higher `version` always wins over an earlier one.
 */
export class Resource<T> {
  #data = $state<T | null>(null);
  error = $state('');
  /** A visible load: the first, a retry, or one the page asked for. */
  loading = $state(false);
  /** A silent re-read after a hint: never shown as loading. */
  refreshing = $state(false);
  /**
   * Rows the last hinted change brought in (`keys` given), for a one-off
   * `data-new` mark; emptied once the mark has played, so a list that mounts
   * again later (a tab switch, coming back to the page) does not replay it.
   */
  fresh = $state<ReadonlySet<string>>(ids([]));
  #path: string;
  #client: Client;
  #topics: readonly EventTopic[];
  #version: ((data: T) => number) | undefined;
  #keys: ((data: T) => Iterable<string>) | undefined;
  #events: LiveEvents;
  /** Requests issued, and the number of the one whose answer is shown (a local write counts as one). */
  #issued = 0;
  #shown = 0;
  #loads = 0;
  #refreshes = 0;
  /** The stream's restart count when the shown answer was taken. */
  #epoch: number;

  constructor(path: string, options: ResourceOptions<T> = {}) {
    this.#path = path;
    this.#client = options.client ?? createClient();
    this.#topics = options.topics ?? [];
    this.#version = options.version;
    this.#keys = options.keys;
    this.#events = options.events ?? live;
    this.#epoch = this.#events.epoch;
  }

  get data(): T | null {
    return this.#data;
  }

  /** A local write (a save's answer) is newer than any read still in flight. */
  set data(value: T | null) {
    this.#data = value;
    this.#shown = ++this.#issued;
    this.fresh = ids([]);
  }

  /**
   * Applies `value` unless something newer is already shown; an equal answer
   * keeps the objects on screen. `anyVersion`: an explicit load, or the server
   * restarted since the shown answer, so a lower version is the truth now.
   */
  #accept(request: number, value: T, origin: Origin, anyVersion: boolean, epoch: number): void {
    if (request < this.#shown) return;
    const current = this.#data;
    if (!anyVersion && current !== null && this.#version && this.#version(value) < this.#version(current)) return;
    this.#shown = request;
    this.#epoch = epoch;
    if (current !== null && JSON.stringify(current) === JSON.stringify(value)) return;
    const arrived = origin === 'hinted' && current !== null && !wroteWithin(OWN_ECHO_MS);
    if (arrived) {
      if (this.#keys) {
        const before = ids(this.#keys(current));
        const fresh = ids([...this.#keys(value)].filter((key) => !before.has(key)));
        this.fresh = fresh;
        if (fresh.size)
          setTimeout(() => {
            if (this.fresh === fresh) this.fresh = ids([]);
          }, ARRIVAL_FALLBACK_MS + 100);
      }
      arrival.seq += 1;
    } else if (this.fresh.size) {
      this.fresh = ids([]);
    }
    this.#data = value;
  }

  async load(): Promise<void> {
    const request = ++this.#issued;
    const epoch = this.#events.epoch;
    this.#loads++;
    this.loading = true;
    try {
      this.#accept(request, await this.#client.get<T>(this.#path), 'asked', true, epoch);
      if (request >= this.#shown) this.error = '';
    } catch (error) {
      if (request >= this.#shown) this.error = error instanceof ApiRequestError ? errorText(error) : 'Could not load.';
    } finally {
      this.loading = --this.#loads > 0;
    }
  }

  /**
   * Re-read in place: no loading state, and a failure keeps what is shown.
   * Resolves to '' once read (a newer answer may already be shown), or the
   * reason it failed, for callers that report it. The page's own re-reads
   * are quiet; hint-driven ones (`follow`) mark what arrived.
   */
  refresh(): Promise<string> {
    return this.#refresh('asked');
  }

  async #refresh(origin: Origin): Promise<string> {
    if (this.#data === null) {
      await this.load();
      return this.error;
    }
    const request = ++this.#issued;
    // The restart count as this read leaves: a restart noticed while it is
    // out belongs to the next read, which the restart's wake-up sends.
    const epoch = this.#events.epoch;
    const restarted = epoch !== this.#epoch;
    this.#refreshes++;
    this.refreshing = true;
    try {
      this.#accept(request, await this.#client.get<T>(this.#path), origin, restarted, epoch);
      if (request >= this.#shown) this.error = '';
      return '';
    } catch (error) {
      // The next hint or poll tries again; the page keeps its data meanwhile.
      return error instanceof ApiRequestError ? errorText(error) : 'Could not load.';
    } finally {
      this.refreshing = --this.#refreshes > 0;
    }
  }

  /** Load, then follow this resource's hints until the returned cleanup runs. */
  watch(): () => void {
    void this.load();
    return this.follow();
  }

  /** Follow hints without loading now (the page loads on its own schedule). */
  follow(): () => void {
    if (this.#topics.length === 0) return () => {};
    return this.#events.subscribe(this.#topics, () => void this.#refresh('hinted'));
  }
}

/**
 * The row a list-detail page opens by default (the first on the page),
 * pinned: a refresh that adds rows above it keeps it open, while a new
 * result set (`source` changes), another page or its removal picks the first again.
 */
export function pinnedFirst(source: () => unknown): (rows: readonly { id: string }[], page: number) => string {
  let pin = '';
  let from: unknown;
  let at = 0;
  return (rows, page) => {
    const owner = source();
    if (owner !== from || page !== at || !rows.some((row) => row.id === pin)) {
      pin = rows[0]?.id ?? '';
      from = owner;
      at = page;
    }
    return pin;
  };
}

export type Outcome<T = unknown> = { ok: true; value: T } | { ok: false; message: string; status?: number | null; code?: string | null };

/** One request with a typed outcome, for forms that keep their input on failure. */
export async function send<T>(work: (client: Client) => Promise<T>): Promise<Outcome<T>> {
  try {
    return { ok: true, value: await work(createClient()) };
  } catch (error) {
    if (error instanceof ApiRequestError) return { ok: false, message: errorText(error), status: error.status, code: error.body?.error ?? null };
    return { ok: false, message: 'Something went wrong.' };
  }
}
