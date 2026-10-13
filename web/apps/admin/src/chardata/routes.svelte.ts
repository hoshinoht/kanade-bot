/**
 * Character data routes (module `character_data`, MapleScouter estimates):
 * there is no session flag for the module, so the first generic 404 (the
 * module off, or no scouter URL) is remembered for the session and nothing
 * more is asked or drawn, as the roster's `rosterRoutes` does.
 */
import type { CharacterLink, CharacterLookup, EstimateJob, EstimateStarted, EstimateStatus, LookupStatus, RunReadiness, TimingReadiness } from '@kanade/api-types';
import type { EstimateApi, LinkApi, Outcome } from '@kanade/ui';
import { send } from '../resource.svelte';

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

/** One character's link routes (the shared strip's Link…, Unlink… and Refresh). */
export function adminLinkApi(id: string): LinkApi {
  const path = `${linkPath(id)}/link`;
  return {
    read: () => send((c) => c.get<CharacterLink>(path)),
    request: (version) => send((c) => c.post<CharacterLink>(path, { version })),
    refresh: () => send((c) => c.post<CharacterLink>(`${linkPath(id)}/refresh`, {})),
    unlink: () => send((c) => c.delete<unknown>(path)),
  };
}

/** One estimate target's routes: a character (the strip) or a run's party (the Estimate panel). */
export function adminEstimateApi(target: { character_ids: string[] } | { run_id: string }): EstimateApi {
  const query = 'run_id' in target ? `run_id=${encodeURIComponent(target.run_id)}` : `character_ids=${target.character_ids.map(encodeURIComponent).join(',')}`;
  return {
    status: () => send((c) => c.get<EstimateStatus>(`/api/admin/estimates?${query}`)),
    start: (refresh): Promise<Outcome<EstimateStarted>> => send((c) => c.post<EstimateStarted>('/api/admin/estimates', { ...target, ...(refresh ? { refresh: true } : {}) })),
    job: (job) => send((c) => c.get<EstimateJob>(`/api/admin/estimates/${encodeURIComponent(job)}`)),
  };
}

/** A lookup step finished: readiness re-reads its MapleScouter chips. */
export function estimateStep(): void {
  estimateEnds.count += 1;
}
