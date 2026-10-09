// What a member's week means for them (public-portal-plan § Week): which runs
// are theirs, what they still owe, and their next run. Pure functions over a
// `MemberWeek`; "now" is the server's clock (`generated_at`) on the guild's
// wall clock, never the browser's.
import type { MemberRun, MemberWeek, Participant } from '@kanade/api-types';
import { dateMinutes, sortRuns, spanWords, wallMinutes } from '@kanade/ui';

/** Done and cancelled runs: hidden on the board until asked for, as on the admin Week. */
export const isPast = (run: Pick<MemberRun, 'status'>): boolean => run.status === 'done' || run.status === 'cancelled';

/** The caller's own place on a run, when they have one. */
export function yours(run: Pick<MemberRun, 'participants'>, memberId: string): Participant | null {
  return run.participants.find((p) => p.id === memberId) ?? null;
}

/** The party as a card's one line: "You" first, then everyone else in party order. */
export function partyLine(run: Pick<MemberRun, 'participants' | 'mine'>, memberId: string): string {
  const others = run.participants.filter((p) => p.id !== memberId).map((p) => p.name);
  return (run.mine ? ['You', ...others] : others).join(' · ');
}

/** The party with "You" first (the pane's and the sheet's party lists). */
export function youFirst(run: Pick<MemberRun, 'participants'>, memberId: string): Participant[] {
  return [...run.participants].sort((a, b) => Number(b.id === memberId) - Number(a.id === memberId));
}

type Clock = Pick<MemberWeek, 'days' | 'generated_at' | 'timezone'>;

/** Minutes from now (server clock) until a timed run starts; null for own-time or unreadable runs. */
export function minutesUntil(run: Pick<MemberRun, 'day' | 'time'>, week: Clock): number | null {
  const day = week.days[run.day];
  const start = day && run.time ? dateMinutes(day.date, run.time) : null;
  const now = wallMinutes(week.generated_at, week.timezone);
  return start === null || now === null ? null : start - now;
}

/**
 * The caller's next run on this week: the soonest own timed run still ahead,
 * else an own-time run from today on; null when nothing of theirs is left.
 */
export function nextOwn(week: MemberWeek): MemberRun | null {
  const today = week.days.find((d) => d.is_today)?.index ?? 0;
  const live = sortRuns(week.runs.filter((r) => r.mine && !isPast(r)));
  return (
    live.find((r) => {
      const left = minutesUntil(r, week);
      return left !== null && left > 0;
    }) ??
    live.find((r) => r.time === null && r.day >= today) ??
    null
  );
}

/** "in 1 day", "in 9 h"; empty for own-time and started runs. */
export function countdownWords(run: Pick<MemberRun, 'day' | 'time'>, week: Clock): string {
  const left = minutesUntil(run, week);
  return left !== null && left > 0 ? `in ${spanWords(left)}` : '';
}

/** Needs you: the caller's own runs that still take answers and have none from them yet, board order. */
export function answersOwed(runs: MemberRun[], memberId: string): MemberRun[] {
  return sortRuns(runs.filter((r) => r.can_edit && yours(r, memberId)?.answer === 'waiting'));
}

/** The runs the board and the list show: only the caller's when asked, the past only when asked. */
export function shownRuns(runs: MemberRun[], options: { onlyMine: boolean; showPast: boolean }): MemberRun[] {
  return runs.filter((r) => (!options.onlyMine || r.mine) && (options.showPast || !isPast(r)));
}
