import type { BackendGroup } from '@kanade/api-types';

/** At most this many permit bars wave at once; the rest stay flat. */
export const MAX_WAVES = 2;

/** Groups whose bar waves: requests in flight, the fullest first, at most two. */
export function wavingGroups(groups: readonly Pick<BackendGroup, 'name' | 'permits'>[]): Set<string> {
  const busy = groups.filter((g) => g.permits.in_use > 0 && g.permits.total > 0);
  busy.sort((a, b) => b.permits.in_use / b.permits.total - a.permits.in_use / a.permits.total);
  return new Set(busy.slice(0, MAX_WAVES).map((g) => g.name));
}
