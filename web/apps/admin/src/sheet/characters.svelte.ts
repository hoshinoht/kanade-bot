/**
 * Who each party member brings to a run (module `roster`; boards
 * B_RosterRunPick, B_PhoneRosterRunPick, per boss B_RunBossPicks) or a weekly
 * timing (modules `roster` and `character_data`; B_FixedReady*, B_FixedBoss*):
 * `GET /api/admin/{runs,fixed}/{id}/characters`, `PUT …/characters/{member}`
 * (the first boss and every boss without its own pick), `PUT …/bosses/{key}`
 * (a later boss) and `POST …/same` (same for every boss). Picks save directly,
 * never through the timing's form, and a warning never blocks a save. UI only: characters never reach a model path.
 */
import type { CharacterChoice, CharacterClash, RunCharacterSlot, Week } from '@kanade/api-types';
import { ApiRequestError, createClient, type Client } from '@kanade/client';
import { dayLabel, type Outcome } from '@kanade/ui';
import { errorText, isUnbuilt, live, send } from '../resource.svelte';

/**
 * Module off after all: the server answered the generic 404 for a character
 * route. Kept for the session (the session's `roster` flag is read once too),
 * so the run sheet shows no character UI and asks no more.
 */
export const rosterRoutes = $state({ gone: false });

export class CharacterSlots<T extends { slots: RunCharacterSlot[] }> {
  data = $state<T | null>(null);
  error = $state('');
  #path: string;
  #seq = 0;
  #client = createClient();
  /** Set on the generic 404: the module is off after all. */
  readonly #routes: { gone: boolean };

  constructor(kind: 'runs' | 'fixed', id: string, routes: { gone: boolean } = rosterRoutes) {
    this.#path = `/api/admin/${kind}/${encodeURIComponent(id)}/characters`;
    this.#routes = routes;
  }

  /** Reads every slot; a later read or a saved pick wins over an earlier answer. */
  async load(): Promise<void> {
    if (this.#routes.gone) return;
    const mine = ++this.#seq;
    try {
      const value = await this.#client.get<T>(this.#path);
      if (mine !== this.#seq) return;
      this.data = value;
      this.error = '';
    } catch (error) {
      if (mine !== this.#seq) return;
      if (error instanceof ApiRequestError && isUnbuilt(error)) this.#routes.gone = true;
      else this.error = error instanceof ApiRequestError ? errorText(error) : 'Could not load.';
    }
  }

  /** Another admin's pick or a party change elsewhere arrives as a schedule hint. */
  follow(): () => void {
    return live.subscribe(['schedule'], () => void this.load());
  }

  slot(member: string): RunCharacterSlot | null {
    return this.data?.slots.find((s) => s.member_id === member) ?? null;
  }

  /** Picks (or with `null` clears) at the slot version shown; a 409 `stale` re-reads. */
  pick(member: string, characterId: string | null): Promise<Outcome<RunCharacterSlot>> {
    const version = this.slot(member)?.version ?? 0;
    return this.#write(member, (c, at) => c.put<RunCharacterSlot>(at, { character_id: characterId, version }));
  }

  /**
   * A later boss's own pick (or with `null` its clear: back to the first boss's
   * character, or the timing's pick for it), at that boss's version shown.
   */
  pickBoss(member: string, key: string, characterId: string | null): Promise<Outcome<RunCharacterSlot>> {
    const version = this.slot(member)?.bosses.find((b) => b.key === key)?.version ?? 0;
    return this.#write(member, (c, at) => c.put<RunCharacterSlot>(`${at}/bosses/${encodeURIComponent(key)}`, { character_id: characterId, version }));
  }

  /** "Same for every boss": clears the member's per-boss picks in one write, at the versions shown. */
  same(member: string): Promise<Outcome<RunCharacterSlot>> {
    const versions = Object.fromEntries((this.slot(member)?.bosses.slice(1) ?? []).map((b) => [b.key, b.version]));
    return this.#write(member, (c, at) => c.post<RunCharacterSlot>(`${at}/same`, { versions }));
  }

  async #write(member: string, request: (c: Client, at: string) => Promise<RunCharacterSlot>): Promise<Outcome<RunCharacterSlot>> {
    const result = await send((c) => request(c, `${this.#path}/${encodeURIComponent(member)}`));
    if (result.ok) {
      ++this.#seq;
      if (this.data) this.data = { ...this.data, slots: this.data.slots.map((s) => (s.member_id === member ? result.value : s)) };
    } else if (result.unbuilt) this.#routes.gone = true;
    else if (result.status === 409 || result.status === 404 || result.status === 422) await this.load();
    return result;
  }
}

/** The character a slot shows: the one the member brings (picked, from the timing, the main or the only one). */
export function shownCharacter(slot: Pick<RunCharacterSlot, 'candidates' | 'brings'>): CharacterChoice | null {
  return slot.candidates.find((c) => c.id === slot.brings) ?? null;
}

type Basis = RunCharacterSlot['basis'];

/**
 * The run sheet's tag for a character the run itself did not pick (B_FixedReadyStates2):
 * "from timing", "main" or "only character" ("only" on phones); none for the run's own pick.
 */
export function runBasisTag(basis: Basis, wide = true): string | null {
  switch (basis) {
    case 'timing_pick':
      return 'from timing';
    case 'main':
      return 'main';
    case 'only':
      return wide ? 'only character' : 'only';
    default:
      return null;
  }
}

/** The other run in words: "Sat 3 21:00 NStar" (its shared bosses only). */
export function clashRun(week: Week, clash: CharacterClash): string {
  return `${dayLabel(week, clash.day)} ${clash.time ?? 'own time'} ${clash.bosses.join(' + ')}`;
}
