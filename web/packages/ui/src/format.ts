import type { Answer, Difficulty, PublicRun, RunStatus, Tally, WeekShape } from '@kanade/api-types';
import type { IconName } from './components/Icon.svelte';

/** v4 STATUS_LABEL without the emoji (templating.STATUS_WORDS). */
export const STATUS_WORDS: Record<RunStatus, string> = {
  planned: 'unconfirmed',
  confirmed: 'confirmed',
  at_risk: 'at risk',
  otot: 'own time',
  done: 'done',
  cancelled: 'cancelled',
};

/** v4 templating.STATUS_ICONS. */
export const STATUS_ICONS: Record<RunStatus, IconName> = {
  planned: 'alert-triangle',
  confirmed: 'check',
  at_risk: 'alert-circle',
  otot: 'clock',
  done: 'check',
  cancelled: 'x',
};

export const ANSWER_MARKS: Record<Answer, { mark: string; word: string }> = {
  yes: { mark: '●', word: 'on' },
  no: { mark: '✕', word: 'out' },
  maybe: { mark: '?', word: 'maybe' },
  waiting: { mark: '○', word: 'no answer yet' },
};

export const DIFFICULTY_WORDS: Record<Difficulty, string> = {
  e: 'Easy',
  n: 'Normal',
  h: 'Hard',
  c: 'Chaos',
  x: 'Extreme',
};

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

export function dayNumber(isoDate: string): string {
  return isoDate.slice(8, 10);
}

export function dayLabel(week: WeekShape, day: number): string {
  const d = week.days[day];
  if (!d) return `day ${day + 1}`;
  return `${d.dow} ${dayNumber(d.date)}`;
}

export function longDate(isoDate: string): string {
  const month = MONTHS[Number(isoDate.slice(5, 7)) - 1] ?? '';
  return `${Number(isoDate.slice(8, 10))} ${month}`;
}

const DOWS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

/** "Thu 24 Sep" for an ISO date (a boss week's start), weekday from the calendar date itself. */
export function weekStartLabel(isoDate: string): string {
  const [y, m, d] = isoDate.slice(0, 10).split('-').map(Number);
  const dow = DOWS[new Date(Date.UTC(y!, m! - 1, d!)).getUTCDay()] ?? '';
  return `${dow} ${longDate(isoDate)}`;
}

export function runTitle(run: Pick<PublicRun, 'bosses'>): string {
  return run.bosses.map((b) => b.token).join(' + ');
}

export function runFullTitle(run: Pick<PublicRun, 'bosses'>): string {
  return run.bosses.map((b) => `${DIFFICULTY_WORDS[b.difficulty]} ${b.name}`).join(' + ');
}

export function whenLabel(week: WeekShape, day: number, time: string | null): string {
  return `${dayLabel(week, day)} ${time ?? 'own time'}`;
}

export function tally(run: Pick<PublicRun, 'tally'>): Tally {
  return run.tally;
}

/** "1 open" or "full": the places still open beside the "3/4" tally. */
export function openPlaces(run: Pick<PublicRun, 'tally'>): string {
  const open = Math.max(0, run.tally.total - run.tally.on);
  return open ? `${open} open` : 'full';
}

/** Screen-reader name for a compact card: everything the card shows as marks. */
export function runAccessibleName(week: WeekShape, run: PublicRun): string {
  const t = tally(run);
  return `${runFullTitle(run)}, ${whenLabel(week, run.day, run.time)}, ${STATUS_WORDS[run.status]}, ${t.on} of ${t.total} on`;
}

export function sortRuns<R extends PublicRun>(runs: R[]): R[] {
  return [...runs].sort((a, b) => a.day - b.day || (a.time ?? '99').localeCompare(b.time ?? '99') || a.id.localeCompare(b.id));
}

/** HH:MM:SS in `timeZone`; HH:MM with `seconds: false` (the admin Live chip). */
export function clockTime(iso: string, timeZone: string, seconds = true): string {
  try {
    return new Intl.DateTimeFormat('en-GB', { timeZone, hour: '2-digit', minute: '2-digit', ...(seconds ? { second: '2-digit' } : {}) }).format(new Date(iso));
  } catch {
    return iso.slice(11, seconds ? 19 : 16);
  }
}
