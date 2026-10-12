// Character data (module `character_data`; boards in
// docs/research/2026-10-12-chardata-mockups): the strip's states, NEXON
// values and MapleScouter estimates in the boards' words. Admin-only: NEXON
// data never reaches the member portal, exports or a model path.
import type {
  BossReadiness,
  BossRow,
  CharacterDataHealth,
  CharacterLink,
  CharacterRow,
  Difficulty,
  EstimateError,
  EstimateJob,
  EstimateMissing,
  ReadinessPower,
  ReadinessSlot,
} from '@kanade/api-types';
import type { IconName } from '@kanade/ui';

/** The API sends these with every job; the panel needs them before the first one. */
export const ESTIMATE_LABEL = 'Estimate from MapleScouter';
export const ESTIMATE_NOTE = 'Estimate for planning only, not eligibility';

/** NEXON data is kept 30 days after the fetch; a member's pending request 14. */
export const PENDING_DAYS = 14;

type Tone = 'neutral' | 'ok' | 'risk' | 'warn';

/** The strip's seven states, plus a linked character that is retired (refresh paused). */
export type StripState = CharacterLink['state'] | 'paused';

export const STRIP_TAGS: Record<StripState, { label: string; tone: Tone; icon: IconName }> = {
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
 * How each power value reads (B_CharReady). The run sheet renders `power`
 * by its kind only: MapleScouter values (`scouter`) replace NEXON's combat
 * power with no other change. Power is shown, never judged.
 */
export const POWER_KINDS: Record<ReadinessPower['kind'], { short: string; label: string; source: string }> = {
  nexon_cp: { short: 'CP', label: 'Combat Power', source: 'NEXON' },
  scouter: { short: 'CP', label: 'Combat Power (MapleScouter)', source: 'MapleScouter' },
};

/** "Lv. 283 · CP 48.2M" and its source: "NEXON · as of 29 Sep", "manual · not linked". */
export function readinessLine(slot: ReadinessSlot): { values: string; source: string } {
  const kind = POWER_KINDS[slot.power?.kind ?? 'nexon_cp'];
  const values = `Lv. ${slot.level?.value ?? '—'} · ${kind.short} ${slot.power ? combatPower(slot.power.value) : '—'}`;
  const asOf = slot.level?.as_of ?? slot.power?.as_of ?? null;
  if (slot.level?.source === 'nexon' || slot.power) return { values, source: `${slot.power ? POWER_KINDS[slot.power.kind].source : 'NEXON'}${asOf ? ` · as of ${asOfDate(asOf).replace(/ \d{4}$/, '')}` : ''}` };
  const why: Partial<Record<NonNullable<ReadinessSlot['link']>, string>> = {
    not_linked: 'not linked',
    not_connected: 'not linked',
    pending: 'link pending',
    stale: 'NEXON link stale',
    expired: 'NEXON data expired',
  };
  const reason = slot.link ? why[slot.link] : undefined;
  return { values, source: reason ? `manual · ${reason}` : 'manual' };
}

/** One boss's level verdict as a chip; never blocking, so nothing is red. */
export function levelChip(boss: BossReadiness): { text: string; tone: Tone } {
  return boss.level === 'ok'
    ? { text: 'Lv ok', tone: 'ok' }
    : boss.level === 'under'
      ? { text: `under Lv. ${boss.entry_level}`, tone: 'warn' }
      : { text: 'Lv unknown', tone: 'neutral' };
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** A NEXON `YYYY-MM-DD` (already Singapore time) as "29 Sep 2026". */
export function asOfDate(date: string): string {
  const [y, m, d] = date.split('-').map(Number);
  return y && m && d ? `${d} ${MONTHS[m - 1]} ${y}` : date;
}

function parts(iso: string, timeZone: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const p of new Intl.DateTimeFormat('en-US', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZone }).formatToParts(new Date(iso)))
    out[p.type] = p.value;
  return out;
}

/** "29 Sep" in `timeZone`. */
export function dayMonth(iso: string, timeZone: string): string {
  const p = parts(iso, timeZone);
  return `${p.day} ${p.month}`;
}

/** "29 Sep 04:01" in `timeZone`. */
export function dayTime(iso: string, timeZone: string): string {
  const p = parts(iso, timeZone);
  return `${p.day} ${p.month} ${p.hour}:${p.minute}`;
}

/** "04:01" in `timeZone`. */
export function hourMinute(iso: string, timeZone: string): string {
  const p = parts(iso, timeZone);
  return `${p.hour}:${p.minute}`;
}

/** "72.4%"; null (MapleScouter can't tell) is a dash. */
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
export function estimateErrorText(error: EstimateError, ign: string | null, party: boolean): { lead: string; text: string } {
  const partial = party ? ' No party total is made from a partial lookup.' : '';
  switch (error.code) {
    case 'busy':
      return { lead: 'MapleScouter is busy.', text: `It is serving another lookup.${error.retry_after_seconds ? ` Try again in ${error.retry_after_seconds} s.` : ''}` };
    case 'critical_rate':
      return {
        lead: 'Set up critical rate on MapleScouter first.',
        text: `${party ? 'MapleScouter shows' : 'Every preset MapleScouter tried shows'} ${ign ?? 'a character'}’s critical rate under 100%${party ? ' in every preset it tried.' : '. Fix it on MapleScouter, then look up again.'}${partial}`,
      };
    case 'manual_check':
      return { lead: 'Scouter needs a manual check.', text: 'MapleScouter asked for a verification. Lookups stay off until an operator runs the scouter setup.' };
    case 'failed': {
      const why =
        error.status === 504 || error.detail === 'lookup_timeout' || error.detail === 'timeout'
          ? `MapleScouter didn’t answer within 2 minutes${error.status ? ` (${error.status})` : ''}.`
          : error.status
            ? `MapleScouter ${party && ign ? `stopped at ${ign}` : 'couldn’t finish the lookup'} (${error.status}).`
            : 'Kanade couldn’t reach MapleScouter.';
      return { lead: 'Lookup failed.', text: `${why}${party ? partial : ' Nothing was kept.'}` };
    }
  }
}

/** Why a party member is not looked up (listed as missing, never 0%). */
export const MISSING_REASONS: Record<EstimateMissing['reason'], string> = {
  no_characters: 'no characters on the roster',
  no_character_picked: 'no character picked',
  other_region: 'not a MapleSEA character',
  unsupported_ign: 'an IGN MapleScouter can’t look up',
};

/** "A", "A and B", "A, B and C". */
export function joinNames(names: readonly string[]): string {
  if (names.length < 2) return names[0] ?? '';
  return `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`;
}

/** Who started a lookup and of what, for the 409 line. */
export function lookupTarget(job: EstimateJob): string {
  return job.run_id ? 'a run’s party' : joinNames(job.characters.map((c) => c.ign));
}

export type HealthTone = Tone;

/** Each health card's tag; risk cards are the ones the Config list counts. */
export function nexonTag(h: CharacterDataHealth, timeZone: string): { label: string; tone: Tone; icon: IconName } {
  switch (h.nexon) {
    case 'ok':
      return { label: 'ok', tone: 'ok', icon: 'check' };
    case 'no_key':
      return { label: 'no_key', tone: 'warn', icon: 'plug' };
    case 'backoff':
      return { label: h.backoff_until ? `backoff until ${hourMinute(h.backoff_until, timeZone)}` : 'backoff', tone: 'warn', icon: 'clock' };
    case 'key_refused':
      return { label: 'key_refused', tone: 'risk', icon: 'alert-triangle' };
  }
}

export function scouterTag(h: CharacterDataHealth): { label: string; tone: Tone; icon: IconName } {
  const state = h.scouter.state;
  if (state === 'ok') return { label: 'ok', tone: 'ok', icon: 'check' };
  if (state === 'off') return { label: 'off', tone: 'neutral', icon: 'minus' };
  return { label: state, tone: 'risk', icon: 'alert-triangle' };
}

/** Shared cards that need someone: unknown, stuck and overdue. */
export const postIssues = (h: CharacterDataHealth) => (h.posts ? h.posts.unknown + h.posts.stuck + h.posts.overdue : 0);

export function sweeperTag(h: CharacterDataHealth): { label: string; tone: Tone; icon: IconName } {
  const n = postIssues(h);
  return n ? { label: `${n} need${n === 1 ? 's' : ''} a hand`, tone: 'risk', icon: 'alert-triangle' } : { label: 'ok', tone: 'ok', icon: 'check' };
}

/** The Config list's note: "ok", "no key", "backoff", or the cards that need attention. */
export function healthNote(h: CharacterDataHealth): { text: string; flagged: number } {
  const flagged = [nexonTag(h, 'UTC'), scouterTag(h), sweeperTag(h)].filter((tag) => tag.tone === 'risk').length;
  if (flagged) return { text: '', flagged };
  if (h.nexon === 'no_key') return { text: 'no key', flagged: 0 };
  return { text: h.nexon === 'backoff' ? 'backoff' : 'ok', flagged: 0 };
}
