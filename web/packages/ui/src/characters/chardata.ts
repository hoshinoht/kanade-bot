// Character data (module `character_data`; boards in
// docs/research/2026-10-12-chardata-mockups and 2026-10-13-member-characters):
// the strip's states, NEXON values and scouter estimates in the boards'
// words, shared by the admin app and the member portal (which shows a member
// only their own characters' NEXON data). Never a model path.
import type { BossRow, CharacterLink, CharacterLookup, CharacterRow, Difficulty, EstimateError, ScouterReading } from '@kanade/api-types';
import type { IconName } from '../components/Icon.svelte';

/** The API sends these with every job; the rows need them before the first one. */
export const ESTIMATE_LABEL = 'Clear-time estimate';
export const ESTIMATE_NOTE = 'For planning only, not eligibility';

/** NEXON data is kept 30 days after the fetch; a member's pending request 14. */
export const PENDING_DAYS = 14;

export type ChipTone = 'neutral' | 'ok' | 'risk' | 'warn';

/** The strip's seven states, plus a linked character that is retired (refresh paused). */
export type StripState = CharacterLink['state'] | 'paused';

export const STRIP_TAGS: Record<StripState, { label: string; tone: ChipTone; icon: IconName }> = {
  not_connected: { label: 'not connected', tone: 'neutral', icon: 'plug' },
  not_linked: { label: 'not linked', tone: 'neutral', icon: 'link' },
  pending: { label: 'pending', tone: 'warn', icon: 'clock' },
  linked: { label: 'linked', tone: 'ok', icon: 'check' },
  paused: { label: 'refresh paused', tone: 'neutral', icon: 'clock' },
  stale: { label: 'stale', tone: 'risk', icon: 'alert-triangle' },
  expired: { label: 'expired', tone: 'risk', icon: 'clock' },
};

/** A retired character's link keeps its data until it lapses, but never refreshes. */
export function stripState(link: CharacterLink, character: CharacterRow): StripState {
  return link.state === 'linked' && character.retired_at !== null ? 'paused' : link.state;
}

/** "48.2M": combat power in millions, one decimal (the guild's unit). */
export function combatPower(value: number): string {
  return `${new Intl.NumberFormat('en-US', { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(value / 1_000_000)}M`;
}

/**
 * "Assumes 100% critical rate (actual 89%)" for a result the scouter worked out
 * as if the character's critical rate were 100% (theirs is under it); null otherwise.
 */
export function assumedCrit(found: { critical_rate_assumed: boolean; critical_rate_actual: number | null }): string | null {
  if (!found.critical_rate_assumed) return null;
  // The scouter's rate to one decimal: "89", "89.5".
  return `Assumes 100% critical rate${found.critical_rate_actual === null ? '' : ` (actual ${Math.round(found.critical_rate_actual * 10) / 10}%)`}`;
}

/** A queued lookup's place: "Queued · 2 ahead". */
export const queuedText = (position: number | null) => `Queued · ${position ?? 1} ahead`;

/**
 * The marker a result at least the saved `estimate_days` old (`stale` in the
 * API) adds to its time line: " · older than 7 days". Still shown, never hidden.
 */
export const olderThan = (days: number) => ` · older than ${days} day${days === 1 ? '' : 's'}`;

/**
 * One boss's scouter reading as a chip: "Estimate 164% · ~18 min"; the rating
 * goes in its title. An assumed critical rate adds a short "· crit assumed" marker and
 * its full label (`assumed`) for the title.
 */
export function scouterChip(reading: Pick<ScouterReading, 'percentage' | 'rating' | 'estimated_minutes' | 'critical_rate_assumed' | 'critical_rate_actual'>): {
  text: string;
  rating: string;
  assumed: string | null;
} {
  const time = reading.estimated_minutes === null ? '' : ` · ~${Math.max(1, Math.round(reading.estimated_minutes))} min`;
  const assumed = assumedCrit(reading);
  return { text: `Estimate ${Math.round(reading.percentage)}%${time}${assumed ? ' · crit assumed' : ''}`, rating: reading.rating, assumed };
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** A NEXON `YYYY-MM-DD` (already Singapore time) as "29 Sep 2026". */
export function asOfDate(date: string): string {
  const [y, m, d] = date.split('-').map(Number);
  return y && m && d ? `${d} ${MONTHS[m - 1]} ${y}` : date;
}

/** "72.4%"; null (the scouter can't tell) is a dash. */
export const percent = (value: number | null) => (value === null ? '—' : `${value.toFixed(1)}%`);

/** Estimated minutes: a decimal under half an hour, whole minutes above. */
export function minutes(value: number | null): string {
  if (value === null) return '—';
  return `${value < 30 ? value.toFixed(1) : Math.round(value)} min`;
}

/** "1:12" from seconds. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** A catalog token (`HLimbo`) as its boss and difficulty; null outside the catalog. */
export function catalogBoss(catalog: readonly BossRow[], token: string): { name: string; difficulty: Difficulty } | null {
  for (const boss of catalog) {
    const d = boss.difficulties.find((option) => option.token === token);
    if (d) return { name: boss.name, difficulty: d.letter };
  }
  return null;
}

/** An estimate error in the boards' words: the lead, and what to do. */
export function estimateErrorText(error: EstimateError, ign: string | null): { lead: string; text: string } {
  switch (error.code) {
    case 'busy':
      return { lead: 'The scouter is busy.', text: `It stayed busy with other lookups.${error.retry_after_seconds ? ` Try again in ${error.retry_after_seconds} s.` : ''}` };
    case 'critical_rate':
      return {
        lead: 'Set up critical rate on the scouter first.',
        text: `The scouter shows ${ign ?? 'a character'}’s critical rate under 100% and couldn’t assume 100% for it. Fix it on the scouter, then look up again.`,
      };
    case 'manual_check':
      return { lead: 'Scouter needs a manual check.', text: 'The scouter asked for a verification. Lookups stay off until an operator runs the scouter setup.' };
    case 'failed': {
      const why =
        error.status === 504 || error.detail === 'lookup_timeout'
          ? `The scouter didn’t answer within 2 minutes${error.status ? ` (${error.status})` : ''}.`
          : error.detail === 'timeout'
            ? 'The scouter didn’t answer in time.'
            : error.status
              ? `The scouter couldn’t finish the lookup (${error.status}).`
              : 'Kanade couldn’t reach the scouter.';
      return { lead: 'Lookup failed.', text: `${why} Nothing was kept.` };
    }
  }
}

/** The Add character lookup's values in one line: "Lv. 283 · Bishop · Aquila". */
export function lookupLine(found: CharacterLookup): string {
  return [found.level !== null ? `Lv. ${found.level}` : '', found.class ?? '', found.world ?? ''].filter(Boolean).join(' · ');
}

/** Why a lookup has no answer, in the dialog's words. */
export type LookupMiss = 'not_found' | 'held' | 'not_connected' | 'wait' | 'failed';

/**
 * A refused lookup (its API code) as the dialog's state and line. `own`: the
 * member adds their own character (their link fills it in), not an admin.
 */
export function lookupMiss(code: string | null | undefined, ign: string, message: string, own = false): { miss: LookupMiss; lead: string; text: string } {
  switch (code) {
    case 'not_found':
      return { miss: 'not_found', lead: 'Not found.', text: `NEXON has no MapleSEA character named “${ign}”. Check the spelling; you can still add it.` };
    case 'nexon_unavailable':
      return { miss: 'held', lead: 'NEXON can’t be asked right now.', text: `${message} You can still add ${ign}; ${own ? 'linking it' : 'the member’s link'} fills it in later.` };
    case 'not_connected':
      return { miss: 'not_connected', lead: 'Character data isn’t connected.', text: 'Without a NEXON key nothing can be looked up: enter the class and level by hand.' };
    case 'lookup_cooldown':
      return { miss: 'wait', lead: 'Wait a moment.', text: 'Lookups are a few seconds apart. Try again shortly.' };
    default:
      return { miss: 'failed', lead: 'Lookup failed.', text: message };
  }
}
