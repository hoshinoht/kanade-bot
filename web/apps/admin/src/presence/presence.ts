/**
 * Admin presence (boards B_Presence*, `/api/admin/presence`): who else has
 * this Inbox request or run sheet open. A hint, never a lock: nothing here is
 * read by a write, and nothing is ever disabled by it.
 *
 * Ages run on this tab's clock between answers: an entry's `seen_s` is its
 * age when the answer came, so `seen_s + (now - received)` is its age now.
 * From FADE_S it fades and says how long ago; at GONE_S it is gone (the
 * server forgets it then too).
 */
import type { PresenceEntry, PresenceKind, PresenceMode } from '@kanade/api-types';

/** A visible page checks in this often. */
export const CHECK_IN_MS = 15_000;
/** An entry fades this long after its last check-in. */
export const FADE_S = 20;
/** And is gone this long after it. */
export const GONE_S = 60;
/** Avatar fills, by admin id (`tone`). */
export const TONES = 4;

/**
 * The next call's `seq`: the wall clock in ms, but always above the last one.
 * The server orders a tab's check-ins and leave by it, and a session's tabs
 * share one entry, so a clock-based value keeps it growing across them.
 */
export function nextSeq(last: number, now: number): number {
  return Math.max(Math.floor(now), last + 1);
}

export interface Shown {
  id: string;
  name: string;
  mode: PresenceMode;
  /** Whole seconds since their last check-in. */
  age: number;
  /** Past FADE_S: drawn faded, with its age. */
  stale: boolean;
  tone: number;
}

/** The entries as they stand at `now` (ms), aged since the answer arrived at `received` (ms). */
export function shown(here: readonly PresenceEntry[], received: number, now: number): Shown[] {
  const elapsed = Math.max(0, Math.floor((now - received) / 1000));
  return here
    .map((entry) => {
      const age = entry.seen_s + elapsed;
      return { id: entry.admin.id, name: entry.admin.name, mode: entry.mode, age, stale: age >= FADE_S, tone: tone(entry.admin.id) };
    })
    .filter((entry) => entry.age < GONE_S);
}

/** A steady fill per admin, so the same person keeps their colour everywhere. */
export function tone(id: string): number {
  let sum = 0;
  for (const ch of id) sum += ch.codePointAt(0)!;
  return sum % TONES;
}

/** "now" while fresh, then the age in tens of seconds ("40 s ago"), so it does not tick every second. */
export function ageText(age: number): string {
  return age < FADE_S ? 'now' : `${Math.floor(age / 10) * 10} s ago`;
}

/** "Ren", "Ren and Kaito", "Ren, Kaito and Mika". */
export function names(list: readonly { name: string }[]): string {
  const all = list.map((entry) => entry.name);
  return all.length < 2 ? (all[0] ?? '') : `${all.slice(0, -1).join(', ')} and ${all.at(-1)}`;
}

/** Everyone editing, fresh or faded: their writes may still land first. */
export function editors(list: readonly Shown[]): Shown[] {
  return list.filter((entry) => entry.mode === 'editing');
}

/** The button's name: "Other admins here: Ren, editing; Kaito, viewing, last seen 40 seconds ago". */
export function buttonLabel(list: readonly Shown[]): string {
  const one = (entry: Shown) => `${entry.name}, ${entry.mode}${entry.stale ? `, last seen ${Math.floor(entry.age / 10) * 10} seconds ago` : ''}`;
  return `Other admins here: ${list.map(one).join('; ')}`;
}

/** What a popover row says the admin is doing. */
export function doing(entry: Shown, kind: PresenceKind): string {
  return entry.mode === 'editing' ? `editing this ${kind === 'run' ? 'run' : 'request'}` : 'viewing';
}

/**
 * The sentence near the action when someone else is editing: what the version
 * check will do, never a lock. `short` is the phone's wording (inside the
 * presence row).
 */
export function editingNote(kind: PresenceKind, list: readonly Shown[], short: boolean): { lead: string; rule: string } | null {
  const editing = editors(list);
  if (!editing.length) return null;
  const who = names(editing);
  const verb = editing.length === 1 ? 'is' : 'are';
  const end = short ? '' : '.';
  if (kind === 'request') {
    return {
      lead: `${who} ${verb} editing this too${end}`,
      rule: short ? 'The first approve wins; the other is refused.' : 'The first approve wins; the other is refused and starts over.',
    };
  }
  const whose = editing.length === 1 ? `${who}’s change lands` : `${editing.map((e) => `${e.name}’s`).join(' or ')} change lands`;
  return {
    lead: `${who} ${verb} editing this run too${end}`,
    rule: `Nothing is locked: if ${whose} first, yours is refused${short ? '' : ' as out of date'} and the sheet shows the new run.`,
  };
}

/**
 * The one polite announcement: someone started editing the thing you are
 * editing ("Ren started editing this request"). `before` is who was editing at
 * the last answer; empty when nobody new started or you are not editing.
 */
export function startedEditing(before: readonly string[], list: readonly Shown[], mine: PresenceMode, kind: PresenceKind): string {
  if (mine !== 'editing') return '';
  const fresh = editors(list).filter((entry) => !before.includes(entry.id));
  if (!fresh.length) return '';
  return `${names(fresh)} started editing this ${kind === 'run' ? 'run' : 'request'}`;
}
