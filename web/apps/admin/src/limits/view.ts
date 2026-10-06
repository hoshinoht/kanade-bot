import type { BackendGroup, Refusal } from '@kanade/api-types';
import { wallMinutes } from '../shared/wall';

/** The breaker states in words, and the chip tone that goes with each (B_LimitsLive). */
export const BREAKER = {
  closed: { word: 'closed — calls flow', tone: 'success', icon: 'check' },
  half_open: { word: 'half-open — probing', tone: 'warning', icon: 'clock' },
  open: { word: 'open — calls refused', tone: 'danger', icon: 'x' },
} as const;

/** Refusal kinds as people say them; unknown kinds print as sent. */
export const REFUSAL: Record<string, string> = {
  rate: 'rate limit',
  concurrency: 'too many in flight',
  quota: 'key quota spent',
  key_rate: 'key rate limit',
  key_quota: 'key quota spent',
};

const DOW = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const ISO_INSTANT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})$/;

/** The guild zone the other pages fall back to before the week (and its zone) has loaded. */
export const GUILD_ZONE = 'Asia/Kuala_Lumpur';

/**
 * A server instant as the API's other labels print it, "Tue 29 Sep 11:56",
 * on the guild's wall clock (never the browser's zone). Anything that is not
 * an ISO instant (an older server's ready-made label) prints as sent.
 */
export function serverTime(value: string, timeZone: string | undefined): string {
  if (!ISO_INSTANT.test(value)) return value;
  // Fixed English names: ICU's en-GB short month for September is "Sept".
  const minutes = wallMinutes(value, timeZone || GUILD_ZONE);
  if (minutes === null) return value;
  const d = new Date(minutes * 60_000);
  const two = (n: number) => String(n).padStart(2, '0');
  return `${DOW[d.getUTCDay()]} ${two(d.getUTCDate())} ${MONTHS[d.getUTCMonth()]} ${two(d.getUTCHours())}:${two(d.getUTCMinutes())}`;
}

const RANK = { open: 0, half_open: 1, closed: 2 } as const;

/** Phones list open, then half-open breakers first; otherwise the configured order (a stable sort). */
export function phoneOrder<T extends Pick<BackendGroup, 'breaker'>>(groups: readonly T[]): T[] {
  return [...groups].sort((a, b) => RANK[a.breaker.state] - RANK[b.breaker.state]);
}

/** The first group whose permits are all held, for the page line. */
export function atCapacity<T extends Pick<BackendGroup, 'permits'>>(groups: readonly T[]): T | undefined {
  return groups.find((g) => g.permits.total > 0 && g.permits.in_use >= g.permits.total);
}

/** Refusals split by scope, backend groups first, each with its total. */
export function refusalGroups(refusals: readonly Refusal[]): { scope: 'group' | 'key'; label: string; total: number; rows: Refusal[] }[] {
  return (
    [
      ['group', 'Backend groups'],
      ['key', 'Gateway key'],
    ] as const
  )
    .map(([scope, label]) => {
      const rows = refusals.filter((r) => (scope === 'key') === (r.scope === 'key'));
      return { scope, label, total: rows.reduce((n, r) => n + r.count, 0), rows };
    })
    .filter((g) => g.rows.length > 0);
}

/** An allowance window in words, as Limits and Account print it: "5 min", "6 h", "1 d", "90 s". */
export function windowWords(perS: number): string {
  if (perS % 86_400 === 0) return `${perS / 86_400} d`;
  if (perS % 3600 === 0) return `${perS / 3600} h`;
  if (perS % 60 === 0) return `${perS / 60} min`;
  return `${perS} s`;
}

export const plural = (n: number, one: string, many = `${one}s`) => (n === 1 ? one : many);

/**
 * How long until an allowance's oldest counted answer leaves its window, as
 * "resets in …" reads it: "1 d 3 h", "5 h 12 m", "2 m 12 s" or "45 s"
 * (hours and days round up to the minute and hour).
 * `now` is the server's clock in epoch ms (`serverNow`), never the browser's.
 * Null when nothing will reset (staff, an empty window, unreadable times);
 * '' once it is due, until the next read brings the new window.
 */
export function resetSpan(resetsAt: string | null | undefined, now: number | null): string | null {
  const at = resetsAt ? Date.parse(resetsAt) : NaN;
  if (Number.isNaN(at) || now === null) return null;
  const s = Math.ceil((at - now) / 1000);
  if (s <= 0) return '';
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} m ${s % 60} s`;
  // Coarser spans round up, so the last unit never reads as already gone.
  const m = Math.ceil(s / 60);
  if (m < 1440) return `${Math.floor(m / 60)} h ${m % 60} m`;
  const h = Math.ceil(s / 3600);
  return `${Math.floor(h / 24)} d ${h % 24} h`;
}
