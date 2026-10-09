import type { BackendGroup, Refusal } from '@kanade/api-types';
import { wallMinutes } from '@kanade/ui';

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

export const plural = (n: number, one: string, many = `${one}s`) => (n === 1 ? one : many);
