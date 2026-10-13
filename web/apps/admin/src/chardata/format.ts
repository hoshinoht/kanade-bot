// Character data (module `character_data`; boards in
// docs/research/2026-10-12-chardata-mockups): the admin-only words — run
// readiness, the party estimate's missing members, Config's health cards and
// the Add toast's link request. The strip's and estimates' shared words live
// in `@kanade/ui` (characters/chardata.ts). Never a model path.
import type { BossReadiness, CharacterDataHealth, EstimateMissing, ReadinessSlot } from '@kanade/api-types';
import { asOfDate, combatPower, type ChipTone, type IconName } from '@kanade/ui';

/**
 * The slot's MapleScouter reading time (B_CharReady), when any boss has one:
 * its readings replace the NEXON CP for that slot (the newest, as they come
 * from one lookup). Shown, never judged.
 */
export function scouterAt(slot: ReadinessSlot): string | null {
  let at: string | null = null;
  for (const boss of slot.bosses) if (boss.scouter && (at === null || boss.scouter.fetched_at > at)) at = boss.scouter.fetched_at;
  return at;
}

/**
 * "Lv. 283 · CP 48.2M" and its source: "NEXON · as of 29 Sep", "manual · not linked".
 * With a MapleScouter reading the CP is left out ("Lv. 283"): the per-boss chips replace it.
 */
export function readinessLine(slot: ReadinessSlot): { values: string; source: string } {
  const scouter = scouterAt(slot) !== null;
  const power = scouter ? null : slot.power;
  const level = `Lv. ${slot.level?.value ?? '—'}`;
  const values = scouter ? level : `${level} · CP ${power ? combatPower(power.value) : '—'}`;
  const asOf = slot.level?.as_of ?? power?.as_of ?? null;
  if (slot.level?.source === 'nexon' || power) return { values, source: `NEXON${asOf ? ` · as of ${asOfDate(asOf).replace(/ \d{4}$/, '')}` : ''}` };
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
export function levelChip(boss: BossReadiness): { text: string; tone: ChipTone } {
  return boss.level === 'ok'
    ? { text: 'Lv ok', tone: 'ok' }
    : boss.level === 'under'
      ? { text: `under Lv. ${boss.entry_level}`, tone: 'warn' }
      : { text: 'Lv unknown', tone: 'neutral' };
}

/** "04:01" in `timeZone`. */
export function hourMinute(iso: string, timeZone: string): string {
  const parts = new Intl.DateTimeFormat('en-US', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZone }).formatToParts(new Date(iso));
  return `${parts.find((p) => p.type === 'hour')?.value}:${parts.find((p) => p.type === 'minute')?.value}`;
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

/** Each health card's tag; risk cards are the ones the Config list counts. */
export function nexonTag(h: CharacterDataHealth, timeZone: string): { label: string; tone: ChipTone; icon: IconName } {
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

export function scouterTag(h: CharacterDataHealth): { label: string; tone: ChipTone; icon: IconName } {
  const state = h.scouter.state;
  if (state === 'ok') return { label: 'ok', tone: 'ok', icon: 'check' };
  if (state === 'off') return { label: 'off', tone: 'neutral', icon: 'minus' };
  return { label: state, tone: 'risk', icon: 'alert-triangle' };
}

/** Shared cards that need someone: unknown, stuck and overdue. */
export const postIssues = (h: CharacterDataHealth) => (h.posts ? h.posts.unknown + h.posts.stuck + h.posts.overdue : 0);

export function sweeperTag(h: CharacterDataHealth): { label: string; tone: ChipTone; icon: IconName } {
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

/** The Add toast's link part from the create's `link`. */
export function createdLinkNote(link: 'pending' | 'failed' | null, member: string): string {
  if (link === 'pending') return ` Asked ${member} to link it: they confirm with /character link in Discord.`;
  if (link === 'failed') return ' The link request didn’t go through: use Link… in Edit to ask again.';
  return '';
}
