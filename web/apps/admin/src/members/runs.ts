import type { Answer, Week } from '@kanade/api-types';

/** One run of the week as a member's detail lists it. */
export interface MemberRun {
  id: string;
  /** `Fri 22:00`, or `Fri own time`. */
  when: string;
  /** `HCarling + HStar`. */
  title: string;
  answer: Answer;
  /** Not done or cancelled. */
  upcoming: boolean;
}

export interface MemberWeek {
  runs: MemberRun[];
  /** The first upcoming run from today on, if any. */
  next: MemberRun | null;
}

export const ANSWER_WORDS: Record<Answer, string> = { yes: 'in', maybe: 'maybe', waiting: 'waiting', no: 'out' };

/** The week's runs that list the member, in week order (own-time runs last on their day). */
export function memberRuns(week: Week, memberId: string): MemberWeek {
  const today = week.days.find((d) => d.is_today)?.index ?? 0;
  const mine = week.runs
    .flatMap((run) => {
      const who = run.participants.find((p) => p.id === memberId);
      return who ? [{ run, answer: who.answer }] : [];
    })
    .toSorted((a, b) => a.run.day - b.run.day || (a.run.time ?? '99:99').localeCompare(b.run.time ?? '99:99'));
  const runs = mine.map(({ run, answer }) => ({
    id: run.id,
    when: `${week.days[run.day]?.dow ?? `Day ${run.day + 1}`} ${run.time ?? 'own time'}`,
    title: run.bosses.map((b) => b.token).join(' + '),
    answer,
    upcoming: run.status !== 'done' && run.status !== 'cancelled',
  }));
  const next = mine.find(({ run }) => run.day >= today && run.status !== 'done' && run.status !== 'cancelled');
  return { runs, next: next ? (runs.find((r) => r.id === next.run.id) ?? null) : null };
}

export type MemberOrder = 'runs' | 'name';

/** Runs this week per member id: every run of the week that lists them (as `runs_this_week` counts). */
export function runCounts(week: Week): Map<string, number> {
  const counts = new Map<string, number>();
  for (const run of week.runs) for (const p of run.participants) counts.set(p.id, (counts.get(p.id) ?? 0) + 1);
  return counts;
}

/** Roster order: most runs first with A–Z between equals, or A–Z alone. */
export function orderMembers<T extends { id: string }>(rows: readonly T[], order: MemberOrder, label: (row: T) => string, runs: (row: T) => number): T[] {
  const byName = (a: T, b: T) => label(a).localeCompare(label(b));
  return rows.toSorted(order === 'runs' ? (a, b) => runs(b) - runs(a) || byName(a, b) : byName);
}
