import type { Run } from '@kanade/api-types';

export interface WeekFilter {
  channel: string;
  member: string;
  boss: string;
}

export const NO_FILTER: WeekFilter = { channel: '', member: '', boss: '' };

export function filtering(filter: WeekFilter): boolean {
  return Boolean(filter.channel || filter.member || filter.boss.trim());
}

/** v4's week filters: party channel, member, and a boss search over token, key and name. */
export function applyFilter(runs: Run[], filter: WeekFilter): Run[] {
  const boss = filter.boss.trim().toLowerCase();
  return runs.filter(
    (run) =>
      (!filter.channel || run.party === filter.channel) &&
      (!filter.member || run.participants.some((p) => p.id === filter.member)) &&
      (!boss || run.bosses.some((b) => [b.token, b.key, b.name].some((t) => t.toLowerCase().includes(boss)))),
  );
}
