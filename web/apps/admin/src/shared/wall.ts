// Wall-clock minutes in the guild's zone: the progress bars compare the
// server's own clock (`Week.generated_at`) with the API's local dates, times
// and "Tue 29 Sep 13:00" labels, never the browser's clock.

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const DAY = 24 * 60;

/** Minutes since 1970-01-01 00:00 on the wall clock of `timeZone`; null when unreadable. */
export function wallMinutes(iso: string, timeZone: string): number | null {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  try {
    const p: Record<string, string> = {};
    for (const part of new Intl.DateTimeFormat('en-US', {
      timeZone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hourCycle: 'h23',
    }).formatToParts(date))
      p[part.type] = part.value;
    return Date.UTC(Number(p.year), Number(p.month) - 1, Number(p.day), Number(p.hour), Number(p.minute)) / 60_000;
  } catch {
    return null;
  }
}

/** A local "2026-09-29" date at "21:00" as wall minutes; null when unreadable. */
export function dateMinutes(isoDate: string, time = '00:00'): number | null {
  const d = /^(\d{4})-(\d{2})-(\d{2})$/.exec(isoDate);
  const t = /^(\d{1,2}):(\d{2})$/.exec(time);
  if (!d || !t) return null;
  return Date.UTC(Number(d[1]), Number(d[2]) - 1, Number(d[3]), Number(t[1]), Number(t[2])) / 60_000;
}

/**
 * An API "Tue 29 Sep 13:00" label as wall minutes. It carries no year, so the
 * year that puts it nearest `near` (wall minutes) wins; null when unreadable.
 */
export function whenMinutes(when: string, near: number): number | null {
  const m = /^\w{3} (\d{1,2}) (\w{3}) (\d{1,2}):(\d{2})$/.exec(when.trim());
  const month = m ? MONTHS.indexOf(m[2]!) : -1;
  if (!m || month < 0) return null;
  const year = new Date(near * 60_000).getUTCFullYear();
  let best: number | null = null;
  for (const y of [year - 1, year, year + 1]) {
    const at = Date.UTC(y, month, Number(m[1]), Number(m[3]), Number(m[4])) / 60_000;
    if (best === null || Math.abs(at - near) < Math.abs(best - near)) best = at;
  }
  return best;
}

/** "47 min", "2 h 5 min", "3 days": the API's countdown wording without "in". */
export function spanWords(minutes: number): string {
  const m = Math.max(0, Math.round(minutes));
  if (m < 60) return `${m} min`;
  if (m < DAY) return m % 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m / 60} h`;
  const days = Math.floor(m / DAY);
  return days === 1 ? '1 day' : `${days} days`;
}
