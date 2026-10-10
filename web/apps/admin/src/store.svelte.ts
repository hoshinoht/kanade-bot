import type { Channel, ConfigView, EventTopic, Identity, Me, MemberRow, Role, MoveResult, Run, RunResult, RunStatus, Session, Stats, Summary, SwapResult, Week, WeekKey } from '@kanade/api-types';
import { ApiRequestError, createClient, createPoller, wroteWithin, type LiveEvents, type Poller } from '@kanade/client';
import { arrival, live, OWN_ECHO_MS } from './resource.svelte';
import { clockTime, runTitle, whenLabel, type FreshState } from '@kanade/ui';
import { directory } from './names/directory.svelte';
import type { Slot } from '@kanade/ui';
import { swapSlots } from './planner/dropTime';
import { tick } from 'svelte';

const POLL_MS = 15_000;
/** While live hints arrive, polling is only the safety net. */
export const FALLBACK_POLL_MS = 60_000;
/** What the board, its tiles and the Inbox badge show: a hint of these re-reads them. */
export const WEEK_TOPICS: readonly EventTopic[] = ['schedule', 'inbox', 'delivery', 'settings'];

/**
 * A polled week, its stats, which week was asked for, whether it came from
 * elsewhere (a hint or a timed poll, not the admin's own refresh or write),
 * and the event stream's restart count when it was read.
 */
type Snapshot = [Week, Stats, WeekKey, boolean, number];

/** The last planner change, for one-step undo: a move goes back; a swap is swapped again. */
export type LastMove =
  | { kind: 'move'; revision: number; runId: string; from: Slot; to: Slot }
  | { kind: 'swap'; revision: number; runId: string; withId: string };

export type MoveOutcome = { ok: true; message: string } | { ok: false; message: string };

export const STATUS_LABELS: Record<Exclude<RunStatus, 'at_risk'>, string> = {
  planned: 'Planned',
  confirmed: 'Confirmed',
  otot: 'Own time',
  done: 'Done',
  cancelled: 'Cancelled',
};

/** Admin week: bounded polling plus optimistic, server-confirmed moves with one-step undo. */
/** Done and cancelled runs: the Week hides them until asked, and does not count them. */
export const isPast = (run: Run) => run.status === 'done' || run.status === 'cancelled';

export class AdminWeek {
  week = $state<Week | null>(null);
  stats = $state<Stats | null>(null);
  fresh = $state<FreshState>('loading');
  updated = $state('');
  lastMove = $state<LastMove | null>(null);
  summary = $state<Summary | null>(null);
  which = $state<WeekKey>('this');
  members = $state<MemberRow[]>([]);
  channels = $state<Channel[]>([]);
  identity = $state<Identity | null>(null);
  session = $state<Session | null>(null);
  /** The signed-in admin as the guild sees them (`member` null for the token and Tailscale). */
  me = $state<Me | null>(null);
  /** The planner's keyboard time step: Config → Run lengths default minutes (30 until it loads). */
  runStep = $state(30);
  /** This boss week, polled alongside while Next week is on screen (read-only). */
  #current = $state<Week | null>(null);
  /** A 403 `discord_session_required` arrived anyway (e.g. a session read before sign-in changed). */
  #refusedProposals = $state(false);

  /** Only a Discord session may approve or reject Kanade's proposals (like ✅ on the card). */
  get proposalsLocked(): boolean {
    return this.#refusedProposals || (this.session?.method !== undefined && this.session.method !== 'discord');
  }

  set proposalsLocked(value: boolean) {
    this.#refusedProposals = value;
  }

  #client = createClient();
  #poller: Poller;
  #events: LiveEvents;
  #pendingMoves = 0;
  #nextUndoRevision = 1;
  #holding = false;
  /** Newest snapshot that arrived while a lift or move was open. */
  #buffered: Snapshot | null = null;
  /** Bumped by each Config save's quiet-mode answer (`setQuiet`). */
  #quietSaves = 0;
  /** The next read was asked for by the admin (`refresh()`): nothing it brings is an arrival. */
  #asked = false;
  /** The stream's restart count when the shown week was taken. */
  #epoch = 0;

  constructor(events: LiveEvents = live) {
    this.#events = events;
    this.#poller = createPoller<Snapshot>({
      task: async (signal) => {
        const which = this.which;
        const remote = !this.#asked;
        this.#asked = false;
        const epoch = this.#events.epoch;
        const query = `?week=${which}`;
        const quietSaves = this.#quietSaves;
        const [week, stats, read, current] = await Promise.all([
          this.#client.get<Week>(`/api/admin/week${query}`, { signal }),
          this.#client.get<Stats>(`/api/admin/stats${query}`, { signal }),
          this.#client.get<Summary>('/api/admin/summary', { signal }),
          // Next week on screen: the Glance's next run still lives in this week.
          which === 'next' ? this.#client.get<Week>('/api/admin/week?week=this', { signal }) : null,
        ]);
        // A save answered while this read was out is newer than the read's quiet mode.
        const summary = quietSaves !== this.#quietSaves && this.summary ? { ...read, quiet_mode: this.summary.quiet_mode } : read;
        // The tiles describe "right now", not the board, so they never wait for a hold.
        if (JSON.stringify(this.summary) !== JSON.stringify(summary)) {
          if (remote && this.summary && !this.#echo()) arrival.seq += 1;
          this.summary = summary;
        }
        if (current && !same(this.#current, current)) this.#current = current;
        return [week, stats, which, remote, epoch] as Snapshot;
      },
      intervalMs: POLL_MS,
      maxIntervalMs: 2 * 60_000,
      maxFailures: 8,
      onData: (snapshot) => this.#receive(snapshot),
      onError: (error) => {
        const offline = !navigator.onLine || (error instanceof ApiRequestError && error.kind === 'network');
        // Signed out (401) is the sign-in page's job, and another refusal (4xx)
        // is not an outage: only the network, timeouts and 5xx read as unreachable.
        const refused = error instanceof ApiRequestError && error.status !== null && error.status < 500;
        if (refused) {
          if (this.fresh === 'loading' || this.fresh === 'error') this.fresh = this.week ? 'stale' : 'loading';
          return;
        }
        this.fresh = offline ? 'offline' : this.week ? 'stale' : 'error';
      },
    });
  }

  /** True while a drag or keyboard lift is open; polled data waits so the board does not shift underneath. */
  get holding(): boolean {
    return this.#holding;
  }

  set holding(value: boolean) {
    this.#holding = value;
    if (!value) this.#flush();
  }

  /** Planner writes are serial: a rollback can therefore restore only its own snapshot. */
  get mutating(): boolean {
    return this.#pendingMoves > 0;
  }

  #receive(snapshot: Snapshot) {
    // Reachable again: the connection is live even if this snapshot waits.
    this.fresh = 'live';
    const [week, , which, , epoch] = snapshot;
    // Asked for before a this/next switch: not the week on screen any more.
    if (which !== this.which) return;
    // A response that left the server before our own move committed is older
    // than what we hold, unless the server restarted since (a restore can
    // lower the version).
    if (this.#older(week, epoch)) return;
    if (this.#holding || this.#pendingMoves > 0) {
      if (!this.#buffered || week.version >= this.#buffered[0].version || epoch !== this.#buffered[4]) this.#buffered = snapshot;
      return;
    }
    this.#apply(snapshot);
  }

  #older(week: Week, epoch: number): boolean {
    return this.week !== null && week.version < this.week.version && epoch === this.#epoch;
  }

  /** The page wrote a moment ago (here or on any page): what changed is its echo. */
  #echo(): boolean {
    return wroteWithin(OWN_ECHO_MS);
  }

  /**
   * Called just before a week from elsewhere (a hint or a timed poll) changes
   * the board, with the runs it adds: the board glides moved cards (FLIP)
   * and marks the new ones. Never for the admin's own writes or refreshes.
   */
  beforeArrival: ((added: string[]) => void) | null = null;

  #apply([week, stats, , remote, epoch]: Snapshot) {
    this.#epoch = epoch;
    // An unchanged poll keeps the objects on screen: new-but-equal objects
    // re-run every derived value and attachment on the page for nothing.
    if (!same(this.week, week)) {
      if (remote && this.week && !this.#echo()) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a lookup built and read here, never state
        const had = new Set(this.week.runs.map((r) => r.id));
        this.beforeArrival?.(week.runs.filter((r) => !had.has(r.id)).map((r) => r.id));
        arrival.seq += 1;
      }
      this.week = week;
    } else if (this.week) this.week.generated_at = week.generated_at;
    if (JSON.stringify(this.stats) !== JSON.stringify(stats)) this.stats = stats;
    // "Updated" names the data on screen, not the last response received.
    this.updated = clockTime(week.generated_at, week.timezone, false);
  }

  #flush() {
    if (this.#holding || this.#pendingMoves > 0 || !this.#buffered) return;
    const snapshot = this.#buffered;
    this.#buffered = null;
    if (!this.#older(snapshot[0], snapshot[4])) this.#apply(snapshot);
  }

  /** Switch between this and next boss week; the board shows loading until it arrives. */
  setWeek(which: WeekKey): void {
    if (which === this.which) return;
    this.which = which;
    this.#buffered = null;
    this.week = null;
    this.stats = null;
    this.lastMove = null;
    this.fresh = 'loading';
    void this.#poller.refresh();
  }

  /** Reference data that changes rarely: loaded once per page. */
  async #loadReference(): Promise<void> {
    const get = <T>(path: string) => this.#client.get<T>(path).catch(() => null);
    const [members, channels, identity, session, roles, config, me] = await Promise.all([
      get<MemberRow[]>('/api/admin/members'),
      get<Channel[]>('/api/admin/channels'),
      get<Identity>('/api/identity'),
      get<Session>('/api/admin/session'),
      get<Role[]>('/api/admin/roles'),
      get<ConfigView>('/api/admin/config'),
      get<Me>('/api/admin/me'),
    ]);
    if (config?.run_lengths) this.runStep = config.run_lengths.default_minutes;
    this.members = members ?? [];
    this.channels = channels ?? [];
    this.identity = identity;
    // One lookup for every Discord name the pages show.
    directory.setMembers(this.members);
    directory.setChannels(this.channels);
    directory.setIdentity(identity);
    directory.setGuildRoles(roles ?? []);
    this.session = session;
    this.me = me;
  }

  /** Ends the session on the server (the cookie is cleared there); the page then shows sign-in. */
  async signOut(): Promise<void> {
    await this.#client.post('/api/admin/auth/logout', {}).catch(() => undefined);
    this.session = null;
    this.me = null;
    this.#refusedProposals = false;
  }

  start(): () => void {
    void this.#loadReference();
    this.#poller.start();
    const online = () => void this.#poller.refresh();
    window.addEventListener('online', online);
    const unfollow = this.#follow();
    return () => {
      window.removeEventListener('online', online);
      unfollow();
      this.#poller.stop();
    };
  }

  /** Read now, as the admin asked (Refresh, after an own decision): what it brings is not an arrival. */
  refresh(): Promise<void> {
    // Joining a read already out (a hint's) does not make that read ours.
    if (this.#poller.state !== 'running') this.#asked = true;
    return this.#poller.refresh();
  }

  /**
   * Hints re-read the week now (a hold buffers the answer like any poll);
   * polling slows to a safety net while the stream is open and returns to
   * its normal cadence when it drops.
   */
  #follow(): () => void {
    const pace = (healthy: boolean) => this.#poller.setInterval(healthy ? FALLBACK_POLL_MS : POLL_MS);
    pace(this.#events.healthy);
    const unhealth = this.#events.onHealth(pace);
    const unfollow = this.#events.subscribe(WEEK_TOPICS, () => void this.#poller.refresh());
    return () => {
      unhealth();
      unfollow();
    };
  }

  /** The loaded week's (This or Next) runs still to come, unfiltered: the drawer's Week count. */
  get openRuns(): number | null {
    return this.week ? this.week.runs.filter((r) => !isPast(r)).length : null;
  }

  /**
   * The "Quiet mode on" chip replaces the Live chip (B_States) while the
   * polled summary says quiet mode is on; Loading, Retrying, Offline and
   * "Can't reach" still show, as the summary may be out of date then.
   */
  get quietChip(): boolean {
    return this.fresh === 'live' && this.summary?.quiet_mode === true;
  }

  /** A Config save's answer: the chip follows it now rather than on the next poll. */
  setQuiet(on: boolean): void {
    this.#quietSaves++;
    if (this.summary && this.summary.quiet_mode !== on) this.summary = { ...this.summary, quiet_mode: on };
  }

  /** B_PhoneNav counts by section key; each is absent until its source loads. */
  get drawerCounts(): Partial<Record<string, number>> {
    const counts: Partial<Record<string, number>> = {};
    if (this.openRuns !== null) counts.week = this.openRuns;
    if (this.summary) {
      counts.members = this.summary.members;
      counts.reminders = this.summary.reminders;
    }
    return counts;
  }

  run(id: string): Run | undefined {
    return this.week?.runs.find((r) => r.id === id);
  }

  /** This boss week: the board's own week, or the copy polled while Next is shown. */
  get thisWeek(): Week | null {
    return this.which === 'this' ? this.week : this.#current;
  }

  /** The loaded week holding a run: the one on screen first, then this week. */
  weekOf(id: string): Week | null {
    if (this.run(id)) return this.week;
    return this.thisWeek?.runs.some((r) => r.id === id) ? this.thisWeek : null;
  }

  describe(runId: string, slot: Slot): string {
    const run = this.run(runId);
    return `${run ? runTitle(run) : 'Run'} to ${this.week ? whenLabel(this.week, slot.day, slot.time) : ''}`;
  }

  #replace(run: Run) {
    this.#replaceAll([run]);
  }

  /**
   * Called just before the admin's own change lands on the board (optimistic
   * move or swap, its confirmation or rollback, a sheet edit), so the board
   * can measure its cards for the FLIP. Polled weeks never call it.
   */
  beforeChange: (() => void) | null = null;

  /**
   * Ends a write. The board's FLIP measures "after" on the next tick, so a
   * buffered poll (another admin's change) waits until then: flushed sooner
   * it would land inside the FLIP window and glide as if it were our move.
   * The write still counts as pending meanwhile, so polls and a released
   * hold keep buffering.
   */
  async #settle(): Promise<void> {
    await tick();
    this.#pendingMoves -= 1;
    this.#flush();
  }

  /** Several runs in one assignment, so a swap never shows half-done. */
  #replaceAll(runs: Run[]) {
    if (!this.week) return;
    this.beforeChange?.();
    this.week = { ...this.week, runs: this.week.runs.map((r) => runs.find((n) => n.id === r.id) ?? r) };
  }

  async move(runId: string, to: Slot, { recordUndo = true } = {}): Promise<MoveOutcome> {
    if (this.mutating) return { ok: false, message: 'Saving the last change…' };
    const week = this.week;
    const run = this.run(runId);
    if (!week || !run) return { ok: false, message: 'That run is no longer on the board.' };
    const from = { day: run.day, time: run.time };

    this.#replace({ ...run, day: to.day, time: run.status === 'otot' ? run.time : to.time });
    this.#pendingMoves += 1;
    try {
      const result = await this.#client.post<MoveResult>(`/api/admin/runs/${encodeURIComponent(runId)}/move`, {
        day: to.day,
        time: to.time,
        version: week.version,
      });
      this.#replace(result.run);
      if (this.week) this.week = { ...this.week, version: result.version };
      if (recordUndo) this.lastMove = { kind: 'move', revision: this.#nextUndoRevision++, runId, from, to };
      const label = `${runTitle(result.run)} to ${whenLabel(week, result.run.day, result.run.time)}`;
      return { ok: true, message: recordUndo ? `Moved ${label}.` : `Move undone: ${label}.` };
    } catch (error) {
      this.#replace(run);
      const reason = error instanceof ApiRequestError ? error.message : 'Something went wrong.';
      if (error instanceof ApiRequestError && error.status === 409) void this.refresh();
      return { ok: false, message: `Couldn't move ${runTitle(run)}: ${reason}` };
    } finally {
      await this.#settle();
    }
  }

  /**
   * Exchange two runs' slots in one server change (POST /runs/{id}/swap).
   * Both cards move together on screen and both go back on a refusal or a
   * conflict; like a move it is sent against the week on screen.
   */
  async swap(runId: string, withId: string, { recordUndo = true } = {}): Promise<MoveOutcome> {
    if (this.mutating) return { ok: false, message: 'Saving the last change…' };
    const week = this.week;
    const a = this.run(runId);
    const b = this.run(withId);
    if (!week || !a || !b) return { ok: false, message: 'That run is no longer on the board.' };
    const to = swapSlots(a, a.status === 'otot', b, b.status === 'otot');
    this.#replaceAll([
      { ...a, ...to.a },
      { ...b, ...to.b },
    ]);
    this.#pendingMoves += 1;
    try {
      const result = await this.#client.post<SwapResult>(`/api/admin/runs/${encodeURIComponent(runId)}/swap`, { with: withId, version: week.version });
      this.#replaceAll(result.runs);
      if (this.week) this.week = { ...this.week, version: result.version };
      if (recordUndo) this.lastMove = { kind: 'swap', revision: this.#nextUndoRevision++, runId, withId };
      const [first, second] = result.runs;
      const label = `${runTitle(first)} to ${whenLabel(week, first.day, first.time)}, ${runTitle(second)} to ${whenLabel(week, second.day, second.time)}`;
      return { ok: true, message: recordUndo ? `Swapped ${label}.` : `Swap undone: ${label}.` };
    } catch (error) {
      this.#replaceAll([a, b]);
      const reason = error instanceof ApiRequestError ? error.message : 'Something went wrong.';
      if (error instanceof ApiRequestError && error.status === 409) void this.refresh();
      return { ok: false, message: `Couldn't swap ${runTitle(a)} with ${runTitle(b)}: ${reason}` };
    } finally {
      await this.#settle();
    }
  }

  async undo(revision?: number): Promise<MoveOutcome | null> {
    const last = this.lastMove;
    if (!last || (revision !== undefined && last.revision !== revision)) return null;
    this.lastMove = null;
    const outcome = last.kind === 'swap' ? await this.swap(last.runId, last.withId, { recordUndo: false }) : await this.move(last.runId, last.from, { recordUndo: false });
    if (!outcome.ok) this.lastMove = last;
    return outcome;
  }

  /** Shared path for the run-sheet edits: server-confirmed, then replaces the run. */
  async #edit(runId: string, method: 'post' | 'patch', action: string, body: object, done: (run: Run) => string): Promise<MoveOutcome> {
    const week = this.week;
    const run = this.run(runId);
    if (!week || !run) return { ok: false, message: 'That run is no longer on the board.' };
    this.#pendingMoves += 1;
    try {
      const path = `/api/admin/runs/${encodeURIComponent(runId)}/${action}`;
      const result = await this.#client[method]<RunResult>(path, { ...body, version: week.version });
      this.#replace(result.run);
      if (this.week) this.week = { ...this.week, version: result.version };
      return { ok: true, message: done(result.run) };
    } catch (error) {
      const reason = error instanceof ApiRequestError ? error.message : 'Something went wrong.';
      if (error instanceof ApiRequestError && error.status === 409) void this.refresh();
      return { ok: false, message: `Couldn't update ${runTitle(run)}: ${reason}` };
    } finally {
      await this.#settle();
    }
  }

  setStatus(runId: string, status: RunStatus): Promise<MoveOutcome> {
    const label = STATUS_LABELS[status as keyof typeof STATUS_LABELS] ?? status;
    return this.#edit(runId, 'patch', 'status', { status }, (run) => `${runTitle(run)} is now ${label.toLowerCase()}.`);
  }

  rsvp(runId: string, memberId: string, answer: 'yes' | 'no' | 'clear'): Promise<MoveOutcome> {
    const word = { yes: 'on', no: 'out', clear: 'no answer' }[answer];
    return this.#edit(runId, 'post', 'rsvp', { member_id: memberId, answer }, (run) => {
      const who = run.participants.find((p) => p.id === memberId)?.name ?? 'Member';
      return `${who} marked ${word} for ${runTitle(run)}.`;
    });
  }

  roster(runId: string, change: { add?: string; remove?: string }): Promise<MoveOutcome> {
    const name = (id?: string) => this.members.find((m) => m.id === id)?.name ?? 'Member';
    return this.#edit(runId, 'patch', 'participants', change, (run) =>
      change.add ? `${name(change.add)} added to ${runTitle(run)} this week.` : `${name(change.remove)} taken off ${runTitle(run)} this week.`,
    );
  }

  /** Put an amended run back on its weekly timing (day, time and roster). */
  resetToFixed(runId: string): Promise<MoveOutcome> {
    return this.#edit(runId, 'post', 'reset', {}, (run) => `${runTitle(run)} is back on its weekly timing.`);
  }

  async ping(runId: string): Promise<MoveOutcome> {
    try {
      const result = await this.#client.post<{ message: string }>(`/api/admin/runs/${encodeURIComponent(runId)}/ping`, {});
      return { ok: true, message: result.message };
    } catch (error) {
      return { ok: false, message: `Couldn't post the preview: ${error instanceof ApiRequestError ? error.message : 'Something went wrong.'}` };
    }
  }
}

/** Equal apart from when it was generated. */
function same(a: Week | null, b: Week): boolean {
  return a !== null && JSON.stringify({ ...a, generated_at: '' }) === JSON.stringify({ ...b, generated_at: '' });
}
