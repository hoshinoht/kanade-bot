// The Members sheet's Characters tab (module `roster`, boards in
// docs/research/2026-10-11-roster-mockups): the list's grouping, the form's
// choices and checks, and the API's refusals in the boards' words. UI only:
// characters never reach a model path.
import type { CharacterRow } from '@kanade/api-types';
import type { SelectOption } from '@kanade/ui';

/** The server's default world key: the guild plays MapleSEA. */
export const DEFAULT_REGION = 'msea';

export const REGIONS: SelectOption[] = [
  { value: 'msea', label: 'MapleSEA' },
  { value: 'gms', label: 'GMS' },
  { value: 'kms', label: 'KMS' },
  { value: 'jms', label: 'JMS' },
  { value: 'tms', label: 'TMS' },
];

/** A stored world key in words; an unlisted key shows as stored. */
export const regionLabel = (world: string) => REGIONS.find((r) => r.value === world)?.label ?? world;

const BRANCHES: [string, string[]][] = [
  ['Explorers', ['Hero', 'Paladin', 'Dark Knight', 'Arch Mage (F/P)', 'Arch Mage (I/L)', 'Bishop', 'Bowmaster', 'Marksman', 'Pathfinder', 'Night Lord', 'Shadower', 'Dual Blade', 'Buccaneer', 'Corsair', 'Cannoneer']],
  ['Cygnus Knights', ['Mihile', 'Dawn Warrior', 'Blaze Wizard', 'Wind Archer', 'Night Walker', 'Thunder Breaker']],
  ['Heroes', ['Aran', 'Evan', 'Mercedes', 'Phantom', 'Luminous', 'Shade']],
  ['Resistance', ['Blaster', 'Battle Mage', 'Wild Hunter', 'Mechanic', 'Xenon', 'Demon Slayer', 'Demon Avenger']],
  ['Nova', ['Kaiser', 'Kain', 'Cadena', 'Angelic Buster']],
  ['Flora', ['Adele', 'Illium', 'Khali', 'Ark']],
  ['Anima', ['Lara', 'Hoyoung']],
  ['Others', ['Zero', 'Kinesis']],
];

/** The class dropdown; a stored class that is not listed stays choosable. */
export function classOptions(current: string | null): SelectOption[] {
  const listed = BRANCHES.flatMap(([group, names]) => names.map((name) => ({ value: name, label: name, group })));
  return current && !listed.some((o) => o.value === current) ? [{ value: current, label: current }, ...listed] : listed;
}

/** Active characters (the main first, then oldest first, as listed) and the retired ones. */
export function splitCharacters(rows: readonly CharacterRow[]): { active: CharacterRow[]; retired: CharacterRow[] } {
  const active = rows.filter((c) => c.retired_at === null);
  return {
    active: [...active.filter((c) => c.role === 'main'), ...active.filter((c) => c.role !== 'main')],
    retired: rows.filter((c) => c.retired_at !== null),
  };
}

/** The member's active main, if any. */
export const mainOf = (rows: readonly CharacterRow[]) => rows.find((c) => c.role === 'main' && c.retired_at === null) ?? null;

/** "27 Sep" in the guild's time zone (en-GB would write "Sept"). */
export function dayMonth(iso: string, timeZone: string): string {
  const parts = new Intl.DateTimeFormat('en-US', { day: 'numeric', month: 'short', timeZone }).formatToParts(new Date(iso));
  return `${parts.find((p) => p.type === 'day')?.value} ${parts.find((p) => p.type === 'month')?.value}`;
}

/** The main stays while other characters are active: retiring or deleting it waits. */
export function mainLocked(row: CharacterRow, rows: readonly CharacterRow[]): boolean {
  return row.role === 'main' && row.retired_at === null && rows.some((c) => c.id !== row.id && c.retired_at === null);
}

export type Field = 'ign' | 'world' | 'class' | 'level' | 'role';
export type FieldErrors = Partial<Record<Field, string>>;

export const FIELD_LABELS: Record<Field, string> = { ign: 'IGN', world: 'Region', class: 'Class', level: 'Level', role: 'Role' };

export const LEVEL_ERROR = 'Level is 1 to 300.';
export const IGN_ERROR = 'An IGN is 4 to 12 letters or digits.';

export const takenText = (ign: string, world: string) =>
  `“${ign}” is already on the roster in ${regionLabel(world)}. An IGN belongs to one member per region, whatever the letter case.`;

/** The level as typed: empty is none, else a whole number (range checked by `checkForm`). */
export function parseLevel(text: string): number | null | 'bad' {
  const t = text.trim();
  if (!t) return null;
  return /^\d{1,4}$/.test(t) ? Number(t) : 'bad';
}

export interface FormValues {
  ign: string;
  world: string;
  class: string;
  level: string;
}

/**
 * What can be told before asking the server: the IGN's shape, a class picked,
 * the level's range, and an IGN the server already refused in this region
 * (`taken`, lower-cased with its region) while it is unchanged.
 */
export function checkForm(values: FormValues, taken: ReadonlySet<string>): FieldErrors {
  const errors: FieldErrors = {};
  const ign = values.ign.trim();
  if (!/^[A-Za-z0-9]{4,12}$/.test(ign)) errors.ign = IGN_ERROR;
  else if (taken.has(takenKey(ign, values.world))) errors.ign = takenText(ign, values.world);
  if (!values.class) errors.class = 'Pick a class.';
  const level = parseLevel(values.level);
  if (level === 'bad' || (level !== null && (level < 1 || level > 300))) errors.level = LEVEL_ERROR;
  return errors;
}

export const takenKey = (ign: string, world: string) => `${world}\u001f${ign.trim().toLowerCase()}`;

/** A 422 `invalid` names its field in its first words ("A level is 1-300."). */
export function invalidField(message: string): { field: Field; text: string } | null {
  if (message.startsWith('An IGN')) return { field: 'ign', text: IGN_ERROR };
  if (message.startsWith('A level')) return { field: 'level', text: LEVEL_ERROR };
  if (message.startsWith('A class')) return { field: 'class', text: message };
  if (message.startsWith('A world')) return { field: 'world', text: message };
  if (message.startsWith('A role')) return { field: 'role', text: message };
  return null;
}
