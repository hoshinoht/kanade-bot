/**
 * Who each party member brings to a run (module `roster`; boards
 * B_RosterRunPick, B_PhoneRosterRunPick): `GET /api/admin/runs/{id}/characters`
 * and `PUT …/characters/{member}`. Picks save directly, never through
 * run-sheet staging, and a warning never blocks a save. UI only: characters
 * never reach a model path.
 */
import type { CharacterChoice, CharacterClash, RunCharacterSlot, RunCharacters, Week } from '@kanade/api-types';
import { ApiRequestError, createClient } from '@kanade/client';
import { dayLabel } from '@kanade/ui';
import { errorText, isUnbuilt, live, send, type Outcome } from '../resource.svelte';

/**
 * Module off after all: the server answered the generic 404 for a character
 * route. Kept for the session (the session's `roster` flag is read once too),
 * so the run sheet shows no character UI and asks no more.
 */
export const rosterRoutes = $state({ gone: false });

export class RunCharacterSlots {
  data = $state<RunCharacters | null>(null);
  error = $state('');
  readonly runId: string;
  #path: string;
  #seq = 0;
  #client = createClient();

  constructor(runId: string) {
    this.runId = runId;
    this.#path = `/api/admin/runs/${encodeURIComponent(runId)}/characters`;
  }

  /** Reads every slot; a later read or a saved pick wins over an earlier answer. */
  async load(): Promise<void> {
    if (rosterRoutes.gone) return;
    const mine = ++this.#seq;
    try {
      const value = await this.#client.get<RunCharacters>(this.#path);
      if (mine !== this.#seq) return;
      this.data = value;
      this.error = '';
    } catch (error) {
      if (mine !== this.#seq) return;
      if (error instanceof ApiRequestError && isUnbuilt(error)) rosterRoutes.gone = true;
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
  async pick(member: string, characterId: string | null): Promise<Outcome<RunCharacterSlot>> {
    const version = this.slot(member)?.version ?? 0;
    const result = await send((c) =>
      c.put<RunCharacterSlot>(`${this.#path}/${encodeURIComponent(member)}`, { character_id: characterId, version }),
    );
    if (result.ok) {
      ++this.#seq;
      if (this.data) this.data = { ...this.data, slots: this.data.slots.map((s) => (s.member_id === member ? result.value : s)) };
    } else if (result.unbuilt) rosterRoutes.gone = true;
    else if (result.status === 409 || result.status === 404 || result.status === 422) await this.load();
    return result;
  }
}

/** The character a slot shows: the pick, else the only candidate (marked inferred). */
export function shownCharacter(slot: RunCharacterSlot): CharacterChoice | null {
  const id = slot.picked ?? slot.inferred;
  return slot.candidates.find((c) => c.id === id) ?? null;
}

/** "Shadower · Lv. 283"; either part may be missing. */
export function choiceMeta(c: CharacterChoice): string {
  return [c.class, c.level ? `Lv. ${c.level}` : null].filter(Boolean).join(' · ');
}

/** The other run in words: "Sat 3 21:00 NStar" (its shared bosses only). */
export function clashRun(week: Week, clash: CharacterClash): string {
  return `${dayLabel(week, clash.day)} ${clash.time ?? 'own time'} ${clash.bosses.join(' + ')}`;
}
