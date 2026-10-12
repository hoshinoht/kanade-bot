/**
 * One page's presence at one place: checks in every CHECK_IN_MS while the tab
 * is visible (a hidden tab stops, so it fades and drops by itself elsewhere),
 * re-reads at once on a `presence` hint, checks in again when the mode
 * changes, and leaves (a best-effort `keepalive` DELETE) when the page goes.
 * A check-in is ambient: it is not this page's write and never extends the
 * session. Failures stay quiet; presence is a hint.
 */
import type { PresenceEntry, PresenceHere, PresenceKind, PresenceList, PresenceMode } from '@kanade/api-types';
import { createClient, createPoller, type Poller } from '@kanade/client';
import { live } from '../resource.svelte';
import { CHECK_IN_MS, editors, nextSeq, shown, startedEditing } from './presence';

const PATH = '/api/admin/presence';
const client = createClient();
/** This page's last call `seq` (every place, so it only ever grows here). */
let lastSeq = 0;
const callSeq = () => (lastSeq = nextSeq(lastSeq, Date.now()));

export class Presence {
  #here = $state<PresenceEntry[]>([]);
  #received = $state(0);
  #now = $state(Date.now());
  /** The polite announcement (`startedEditing`); the presence button is not a live region. */
  announcement = $state('');
  readonly kind: PresenceKind;
  #mode: PresenceMode = 'viewing';
  #poller: Poller | null = null;
  /** Answers are taken in the order their requests went out. */
  #sent = 0;
  #taken = 0;
  #editing: readonly string[] = [];

  constructor(kind: PresenceKind) {
    this.kind = kind;
  }

  /** The others here now, aged on this tab's clock. */
  get list() {
    return shown(this.#here, this.#received, this.#now);
  }

  #take(order: number, here: PresenceEntry[]) {
    if (order < this.#taken) return;
    this.#taken = order;
    this.#received = this.#now = Date.now();
    this.#here = here;
    const list = this.list;
    const said = startedEditing(this.#editing, list, this.#mode, this.kind);
    if (said) this.announcement = said;
    this.#editing = editors(list).map((entry) => entry.id);
  }

  /** Check in at `id` until the returned cleanup runs (an `$effect`). */
  watch(id: string): () => void {
    const at = `${this.kind}:${id}`;
    const query = `${PATH}?at=${encodeURIComponent(at)}`;
    const poller = createPoller<{ order: number; here: PresenceHere }>({
      intervalMs: CHECK_IN_MS,
      task: async (signal) => {
        const order = ++this.#sent;
        const here = await client.put<PresenceHere>(PATH, { at: { kind: this.kind, id }, mode: this.#mode, seq: callSeq() }, { signal, ambient: true });
        return { order, here };
      },
      onData: ({ order, here }) => this.#take(order, here.here),
    });
    this.#poller = poller;
    poller.start();
    const unfollow = live.subscribe(['presence'], () => {
      const order = ++this.#sent;
      client.get<PresenceList>(query).then(
        (list) => this.#take(order, list.here),
        () => {},
      );
    });
    // Ages tick on screen only while someone is listed.
    const tick = setInterval(() => {
      if (this.#here.length) this.#now = Date.now();
    }, 1_000);
    // The leave carries a newer seq than any check-in sent, so one cut off in flight cannot land after it and count.
    const leave = () => void client.delete(`${query}&seq=${callSeq()}`, { ambient: true, keepalive: true }).catch(() => {});
    addEventListener('pagehide', leave);
    return () => {
      poller.stop();
      if (this.#poller === poller) this.#poller = null;
      unfollow();
      clearInterval(tick);
      removeEventListener('pagehide', leave);
      leave();
      this.#here = [];
      this.#editing = [];
      this.announcement = '';
    };
  }

  /** What this page is doing here; a change checks in at once. */
  set mode(mode: PresenceMode) {
    if (mode === this.#mode) return;
    this.#mode = mode;
    void this.#poller?.refresh();
  }

  get mode() {
    return this.#mode;
  }
}
