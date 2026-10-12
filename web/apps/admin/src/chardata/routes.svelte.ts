/**
 * Character data routes (module `character_data`, MapleScouter estimates):
 * there is no session flag for the module, so the first generic 404 (the
 * module off, or no scouter URL) is remembered for the session and nothing
 * more is asked or drawn, as the roster's `rosterRoutes` does.
 */
import type { CharacterLink, CharacterLookup, EstimateJob, EstimateStarted, EstimateStatus, LookupStatus, RunReadiness } from '@kanade/api-types';
import { send, type Outcome } from '../resource.svelte';

export const chardataRoutes = $state({ gone: false, estimatesGone: false });

/**
 * Lookups this tab saw end (any target): readiness reads it, so its
 * MapleScouter chips follow a finished Estimate panel or strip lookup.
 */
export const estimateEnds = $state({ count: 0 });

const linkPath = (id: string) => `/api/admin/characters/${encodeURIComponent(id)}`;

const LOOKUP = '/api/admin/characters/lookup';

/**
 * Whether NEXON is connected for the Add character dialog: `false` when the
 * module is off (and remembered as gone) or has no key, `null` when the read
 * failed (the dialog then keeps the manual fields).
 */
export async function lookupConnected(): Promise<boolean | null> {
  if (chardataRoutes.gone) return false;
  const result = await send((c) => c.get<LookupStatus>(LOOKUP));
  if (result.ok) return result.value.connected;
  if (result.unbuilt) chardataRoutes.gone = true;
  return result.unbuilt ? false : null;
}

/** The Add character lookup: shown only, never stored by anyone. */
export function lookUp(ign: string, world: string): Promise<Outcome<CharacterLookup>> {
  return send((c) => c.post<CharacterLookup>(LOOKUP, { ign, world }));
}

/**
 * A run's readiness (B_CharReady): computed on read by the server, never
 * stored and never blocking. Anything but an answer draws nothing.
 */
export class RunReadinessView {
  data = $state<RunReadiness | null>(null);
  readonly #path: string;
  #seq = 0;

  constructor(runId: string) {
    this.#path = `/api/admin/runs/${encodeURIComponent(runId)}/readiness`;
  }

  /** A later read wins over an earlier answer. */
  async load(): Promise<void> {
    if (chardataRoutes.gone) return;
    const mine = ++this.#seq;
    const result = await send((c) => c.get<RunReadiness>(this.#path));
    if (mine !== this.#seq) return;
    if (result.ok) this.data = result.value;
    else {
      this.data = null;
      if (result.unbuilt) chardataRoutes.gone = true;
    }
  }
}

/** One character's link and NEXON data; null while unread, off or failed (`error`). */
export class CharacterLinkView {
  data = $state<CharacterLink | null>(null);
  error = $state('');
  readonly id: string;

  constructor(id: string) {
    this.id = id;
  }

  async load(): Promise<void> {
    if (chardataRoutes.gone) return;
    const result = await send((c) => c.get<CharacterLink>(`${linkPath(this.id)}/link`));
    if (result.ok) {
      this.data = result.value;
      this.error = '';
    } else if (result.unbuilt) chardataRoutes.gone = true;
    else this.error = result.message;
  }

  /** Link… (pending until the member confirms), Unlink… and Refresh; a refusal re-reads. */
  async act(action: 'link' | 'unlink' | 'refresh'): Promise<Outcome> {
    const path = linkPath(this.id);
    const result =
      action === 'link'
        ? await send((c) => c.post<CharacterLink>(`${path}/link`, { version: this.data?.version ?? 0 }))
        : action === 'refresh'
          ? await send((c) => c.post<CharacterLink>(`${path}/refresh`, {}))
          : await send((c) => c.delete<unknown>(`${path}/link`));
    if (!result.ok && result.unbuilt) chardataRoutes.gone = true;
    else if (result.ok && action !== 'unlink') this.data = result.value as CharacterLink;
    else await this.load();
    return result;
  }
}

/** The server's clock minus the browser's, learnt from a job this tab started. */
let serverOffset: number | null = null;

const POLL_MS = 1_500;

/**
 * One estimate target (a character, or a run's party): its newest job and the
 * one running now. A running job is read again until it ends (one lookup runs
 * at a time, so another target's job blocks this one until it does).
 */
export class EstimateLookup {
  job = $state<EstimateJob | null>(null);
  running = $state<EstimateJob | null>(null);
  loaded = $state(false);
  starting = $state(false);
  /** A refused start (409, 422, …): what and why. */
  refused = $state<{ code: string; message: string } | null>(null);
  /** Seconds since the running job started; ticks while one runs. */
  elapsed = $state(0);
  /** Busy (429): the Look up button waits this many seconds. */
  wait = $state(0);
  readonly #query: string;
  readonly #body: { character_ids: string[] } | { run_id: string };
  #timer: number | undefined;
  readonly #ticker: number;
  /** The running job this tab first saw (not started here) and when. */
  #seen = { id: '', at: 0 };
  #closed = false;

  constructor(target: { character_ids: string[] } | { run_id: string }) {
    this.#body = target;
    this.#query = 'run_id' in target ? `run_id=${encodeURIComponent(target.run_id)}` : `character_ids=${target.character_ids.map(encodeURIComponent).join(',')}`;
    this.#ticker = window.setInterval(() => this.#tick(), 1_000);
  }

  /** The running job that blocks this target: another target's. */
  get blocker(): EstimateJob | null {
    return this.running && this.running.id !== this.job?.id ? this.running : null;
  }

  async load(): Promise<void> {
    if (chardataRoutes.gone || chardataRoutes.estimatesGone) return;
    const result = await send((c) => c.get<EstimateStatus>(`/api/admin/estimates?${this.#query}`));
    if (this.#closed) return;
    if (result.ok) {
      this.job = result.value.job;
      this.running = result.value.running;
      this.#busyFrom(this.job);
      this.#follow();
    } else if (result.unbuilt) chardataRoutes.estimatesGone = true;
    this.loaded = true;
  }

  async start(refresh: boolean): Promise<void> {
    if (this.starting) return;
    this.starting = true;
    this.refused = null;
    const result = await send((c) => c.post<EstimateStarted>('/api/admin/estimates', { ...this.#body, ...(refresh ? { refresh: true } : {}) }));
    this.starting = false;
    if (this.#closed) return;
    if (result.ok) {
      const job = result.value.job;
      serverOffset = Date.parse(job.started_at) - Date.now();
      this.job = job;
      this.running = job.state === 'running' ? job : null;
      if (job.state !== 'running') estimateEnds.count += 1;
      this.#busyFrom(job);
      this.#follow();
      return;
    }
    if (result.unbuilt) {
      chardataRoutes.estimatesGone = true;
      return;
    }
    this.refused = { code: result.code ?? '', message: result.message };
    // 409: another lookup is running; read who started it, and wait for it.
    if (result.status === 409) await this.load();
  }

  close(): void {
    this.#closed = true;
    clearTimeout(this.#timer);
    clearInterval(this.#ticker);
  }

  #busyFrom(job: EstimateJob | null) {
    const seconds = job?.state === 'failed' && job.error?.code === 'busy' ? job.error.retry_after_seconds : null;
    this.wait = seconds ?? 0;
  }

  #tick() {
    if (this.wait > 0) this.wait -= 1;
    const job = this.job?.state === 'running' ? this.job : this.blocker;
    if (!job) return;
    if (serverOffset !== null) this.elapsed = Math.max(0, (Date.now() + serverOffset - Date.parse(job.started_at)) / 1000);
    else {
      // Found running, not started here: count from when this tab first saw it.
      if (this.#seen.id !== job.id) this.#seen = { id: job.id, at: Date.now() };
      this.elapsed = (Date.now() - this.#seen.at) / 1000;
    }
  }

  /** Reads the running job (this target's, or the one blocking it) again until it ends. */
  #follow() {
    clearTimeout(this.#timer);
    const next = this.job?.state === 'running' ? this.job : this.blocker;
    if (!next || this.#closed) return;
    this.#timer = window.setTimeout(async () => {
      const result = await send((c) => c.get<EstimateJob>(`/api/admin/estimates/${encodeURIComponent(next.id)}`));
      if (this.#closed) return;
      if (result.ok) {
        const job = result.value;
        if (job.id === this.job?.id) {
          this.job = job;
          this.#busyFrom(job);
        }
        this.running = job.state === 'running' ? job : null;
        if (!this.running) estimateEnds.count += 1;
        if (!this.running && this.refused?.code === 'estimate_running') this.refused = null;
        this.#follow();
      } else if (result.unbuilt) chardataRoutes.estimatesGone = true;
      else if (result.status === 404) await this.load();
    }, POLL_MS);
  }
}
