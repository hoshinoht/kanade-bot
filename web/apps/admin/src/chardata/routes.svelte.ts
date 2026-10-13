/**
 * Character data routes (module `character_data`, MapleScouter estimates):
 * there is no session flag for the module, so the first generic 404 (the
 * module off, or no scouter URL) is remembered for the session and nothing
 * more is asked or drawn, as the roster's `rosterRoutes` does.
 */
import type { CharacterLink, CharacterLookup, EstimateJob, EstimateStarted, EstimateStatus, LookupStatus, RunReadiness, TimingReadiness } from '@kanade/api-types';
import { send, type Outcome } from '../resource.svelte';

export const chardataRoutes = $state({ gone: false, estimatesGone: false });

/**
 * Lookup steps this tab saw finish (a character found or failed, or a whole
 * lookup, any target): readiness reads it, so its MapleScouter chips follow
 * an Estimate panel or strip lookup as each character comes in.
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

/**
 * The Fixed editor's readiness and per-timing picks (modules `roster` and
 * `character_data`; B_FixedReady*): `on` is null until asked, then whether
 * both answer. Learnt once per session from the session's `roster` and the
 * module's lookup status (a generic 404 when it is off); a generic 404 from
 * a timing route later turns it off too (`gone`).
 */
export const fixedReadyRoutes = $state({ on: null as boolean | null, gone: false });

export async function probeFixedReady(roster: boolean): Promise<void> {
  if (fixedReadyRoutes.on !== null) return;
  if (!roster || chardataRoutes.gone) {
    fixedReadyRoutes.on = false;
    return;
  }
  const result = await send((c) => c.get<LookupStatus>(LOOKUP));
  const unbuilt = !result.ok && result.unbuilt === true;
  if (unbuilt) chardataRoutes.gone = true;
  // A failed read leaves it unknown: the next editor asks again.
  if (result.ok || unbuilt) fixedReadyRoutes.on = result.ok;
}

/** What the readiness is read for: the saved timing, or the editor's form as it stands. */
export type TimingTarget = { fixedId: string } | { fixed_run_id: string | null; bosses: string[]; participants: string[] };

/**
 * A weekly timing's readiness (B_FixedReady): computed on read, never stored and
 * never blocking. `error` keeps the rows (their basis and picks come from the
 * characters read) and drops only the values.
 */
export class TimingReadinessView {
  data = $state<TimingReadiness | null>(null);
  error = $state('');
  loading = $state(false);
  #seq = 0;
  #last: TimingTarget | null = null;

  async load(target: TimingTarget | null = this.#last): Promise<void> {
    if (!target || fixedReadyRoutes.gone) return;
    this.#last = target;
    const mine = ++this.#seq;
    this.loading = true;
    const result =
      'fixedId' in target
        ? await send((c) => c.get<TimingReadiness>(`/api/admin/fixed/${encodeURIComponent(target.fixedId)}/readiness`))
        : await send((c) => c.post<TimingReadiness>('/api/admin/fixed/readiness', target));
    if (mine !== this.#seq) return;
    this.loading = false;
    if (result.ok) {
      this.data = result.value;
      this.error = '';
    } else if (result.unbuilt) fixedReadyRoutes.gone = true;
    else {
      this.data = null;
      this.error = result.message;
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

/** Not finished yet: waiting in line or being looked up. */
export const activeJob = (job: EstimateJob | null): job is EstimateJob => job?.state === 'queued' || job?.state === 'running';

/**
 * One estimate target (a character, or a run's party): its newest job. A
 * queued or running job is read again until it ends; lookups never block one
 * another (a second one waits in line, with its place), and each character's
 * outcome shows as it comes in.
 */
export class EstimateLookup {
  job = $state<EstimateJob | null>(null);
  loaded = $state(false);
  starting = $state(false);
  /** A refused start (422, 429 queue full, …): what and why. */
  refused = $state<{ code: string; message: string } | null>(null);
  /** Seconds the job has been in its state (waiting, or being looked up); ticks while active. */
  elapsed = $state(0);
  /** Busy (429): the Look up button waits this many seconds. */
  wait = $state(0);
  readonly #query: string;
  readonly #body: { character_ids: string[] } | { run_id: string };
  #timer: number | undefined;
  readonly #ticker: number;
  /** Since when (browser clock) the job has been in its state. */
  #since = { key: '', at: 0 };
  /** The job this tab started, so its time starts now. */
  #mine = '';
  /** The finished steps last seen, so each new one is told once. */
  #steps = { id: '', count: 0 };
  #closed = false;

  constructor(target: { character_ids: string[] } | { run_id: string }) {
    this.#body = target;
    this.#query = 'run_id' in target ? `run_id=${encodeURIComponent(target.run_id)}` : `character_ids=${target.character_ids.map(encodeURIComponent).join(',')}`;
    this.#ticker = window.setInterval(() => this.#tick(), 1_000);
  }

  async load(): Promise<void> {
    if (chardataRoutes.gone || chardataRoutes.estimatesGone) return;
    const result = await send((c) => c.get<EstimateStatus>(`/api/admin/estimates?${this.#query}`));
    if (this.#closed) return;
    if (result.ok) {
      this.#take(result.value.job, false);
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
      if (job.id !== this.job?.id) this.#mine = job.id;
      this.#take(job, true);
      this.#follow();
      return;
    }
    if (result.unbuilt) {
      chardataRoutes.estimatesGone = true;
      return;
    }
    this.refused = { code: result.code ?? '', message: result.message };
  }

  close(): void {
    this.#closed = true;
    clearTimeout(this.#timer);
    clearInterval(this.#ticker);
  }

  /** Takes a read of the job; `told` says new finished steps (a found or failed character, or the end). */
  #take(job: EstimateJob | null, told: boolean) {
    this.job = job;
    const seconds = job?.state === 'failed' && job.error?.code === 'busy' ? job.error.retry_after_seconds : null;
    this.wait = seconds ?? 0;
    if (!job) return;
    const count = job.progress.done + (activeJob(job) ? 0 : 1);
    if (told && count > (job.id === this.#steps.id ? this.#steps.count : 0)) estimateEnds.count += 1;
    this.#steps = { id: job.id, count };
    this.#clock();
  }

  #tick() {
    if (this.wait > 0) this.wait -= 1;
    this.#clock();
  }

  #clock() {
    const job = this.job;
    if (!activeJob(job)) return;
    const key = `${job.id} ${job.state}`;
    if (this.#since.key !== key) {
      // A state seen change here starts now; one found already under way counts from when it was asked for.
      const seenChange = this.#since.key.startsWith(`${job.id} `) || job.id === this.#mine;
      const asked = serverOffset === null ? Date.now() : Date.parse(job.started_at) - serverOffset;
      this.#since = { key, at: seenChange ? Date.now() : Math.min(Date.now(), asked) };
    }
    this.elapsed = Math.max(0, (Date.now() - this.#since.at) / 1000);
  }

  /** Reads this target's queued or running job again until it ends. */
  #follow() {
    clearTimeout(this.#timer);
    const next = this.job;
    if (!activeJob(next) || this.#closed) return;
    this.#timer = window.setTimeout(async () => {
      const result = await send((c) => c.get<EstimateJob>(`/api/admin/estimates/${encodeURIComponent(next.id)}`));
      if (this.#closed) return;
      if (result.ok) {
        if (result.value.id === this.job?.id) this.#take(result.value, true);
        this.#follow();
      } else if (result.unbuilt) chardataRoutes.estimatesGone = true;
      else if (result.status === 404) await this.load();
    }, POLL_MS);
  }
}
