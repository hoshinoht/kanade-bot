// The Party total (boards in docs/research/2026-10-16-party-polish, v2 in
// 2026-10-16-per-boss-picks): per boss the saved estimates' summed effective
// % and, from a saved total of that boss's whole party, the minutes and
// "technically possible" (`RunPartyTotal`, the reminder image's figures).
// The admin run sheet draws it as the Party grid's first row (and the phone
// list's first item), the portal run pane as one block. Words shared here.
import type { RunPartyTotal } from '@kanade/api-types';

/** "Tue 29 Sep 12:00" for an instant in `timeZone`. */
export function asOfTime(iso: string, timeZone: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone,
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(date);
  const part = (type: string) => parts.find((p) => p.type === type)?.value ?? '';
  return `${part('weekday')} ${part('day')} ${part('month')} ${part('hour')}:${part('minute')}`;
}

/** Some boss has a figure: else every boss says "no estimate yet". */
export const hasTotal = (total: RunPartyTotal) => total.bosses.some((b) => b.combined_percentage !== null);

/** One member's reading of one boss, as the app has it, for the notes under the total. */
export interface TotalReading {
  /** As the screen names the member ("Ren (2)"). */
  name: string;
  /** The boss token (`HCarling`). */
  token: string;
  /** The character brought to that boss; `null` with none. */
  ign: string | null;
  /** The saved reading of that character for that boss; `null` without one. */
  reading: { effective_percentage: number | null; critical_rate_assumed: boolean; critical_rate_actual: number | null } | null;
}

export interface TotalNotes {
  /** Characters counted at an assumed 100% critical rate, once each. */
  assumed: { ign: string; actual: number | null }[];
  /** Members with no estimate for some boss that has a total, in party order. */
  none: string[];
}

/** The notes under the Party total: who is counted at an assumed critical rate, who has no estimate. */
export function totalNotes(total: RunPartyTotal, readings: readonly TotalReading[]): TotalNotes {
  const assumed: TotalNotes['assumed'] = [];
  const none: string[] = [];
  for (const boss of total.bosses) {
    if (boss.combined_percentage === null) continue;
    for (const r of readings.filter((x) => x.token === boss.boss)) {
      if (boss.critical_rate_assumed && r.ign && r.reading?.critical_rate_assumed && !assumed.some((a) => a.ign === r.ign))
        assumed.push({ ign: r.ign, actual: r.reading.critical_rate_actual });
      if (boss.no_estimate > 0 && (r.reading?.effective_percentage ?? null) === null && !none.includes(r.name)) none.push(r.name);
    }
  }
  return { assumed, none };
}
