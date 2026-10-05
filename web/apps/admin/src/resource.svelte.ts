import type { EventTopic } from '@kanade/api-types';
import { ApiRequestError, createClient, createLiveEvents, type Client, type LiveEvents } from '@kanade/client';

/** The server's generic 404 for a route it does not serve (an unbuilt feature, not a missing item). */
const UNBUILT = 'No such endpoint on this origin.';

/** An error in words; a route this server has not built yet is not an outage. */
export function errorText(error: ApiRequestError): string {
  if (error.status === 404 && error.body?.message === UNBUILT) return "This isn't available on this server yet.";
  return error.message;
}

/** The admin app's one change-hint stream (`GET /api/admin/events`), opened while anything listens. */
export const live: LiveEvents = createLiveEvents({ url: '/api/admin/events' });

export interface ResourceOptions<T> {
  /** Hints that make an open page re-read this resource. */
  topics?: readonly EventTopic[];
  /** A version that only grows (history head, `version`): an older read never replaces a newer one. */
  version?: (data: T) => number;
  client?: Client;
  events?: LiveEvents;
}

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
  #path: string;
  #client: Client;
  #topics: readonly EventTopic[];
  #version: ((data: T) => number) | undefined;
  #events: LiveEvents;
  /** Requests issued, and the number of the one whose answer is shown (a local write counts as one). */
  #issued = 0;
  #shown = 0;
  #loads = 0;
  #refreshes = 0;

  constructor(path: string, options: ResourceOptions<T> = {}) {
    this.#path = path;
    this.#client = options.client ?? createClient();
    this.#topics = options.topics ?? [];
    this.#version = options.version;
    this.#events = options.events ?? live;
  }

  get data(): T | null {
    return this.#data;
  }

  /** A local write (a save's answer) is newer than any read still in flight. */
  set data(value: T | null) {
    this.#data = value;
    this.#shown = ++this.#issued;
  }

  /** Applies `value` unless something newer is already shown; an equal answer keeps the objects on screen. */
  #accept(request: number, value: T): void {
    if (request < this.#shown) return;
    const current = this.#data;
    if (current !== null && this.#version && this.#version(value) < this.#version(current)) return;
    this.#shown = request;
    if (current === null || JSON.stringify(current) !== JSON.stringify(value)) this.#data = value;
  }

  async load(): Promise<void> {
    const request = ++this.#issued;
    this.#loads++;
    this.loading = true;
    try {
      this.#accept(request, await this.#client.get<T>(this.#path));
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
   * reason it failed, for callers that report it.
   */
  async refresh(): Promise<string> {
    if (this.#data === null) {
      await this.load();
      return this.error;
    }
    const request = ++this.#issued;
    this.#refreshes++;
    this.refreshing = true;
    try {
      this.#accept(request, await this.#client.get<T>(this.#path));
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
    return this.#events.subscribe(this.#topics, () => void this.refresh());
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
