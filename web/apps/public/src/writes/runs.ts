// The member's own-run writes (member-writes contract Phase A): who may
// answer or move which run, the optimistic answer, what changed when a write
// came back stale, the Discord deep link's `move_to` instant, what the Move
// page checks about a pick and says about a link gone stale, the request
// form's addresses, and the addresses a fresh sign-in returns to. Pure, over
// `MemberWeek`; "now" is the server's clock (`generated_at`) on the guild's
// wall clock.
import type { Answer, MemberRun, MemberRunLink, MemberWeek } from '@kanade/api-types';
import { clashes, dateMinutes, fromMinutes, liveRuns, namesIn, pickerRun, runTitle, wallMinutes, type Slot } from '@kanade/ui';
import { isPast, minutesUntil, yours } from '../member';

export type Choice = Exclude<Answer, 'waiting'>;

export const CHOICES: { value: Choice; label: string }[] = [
  { value: 'yes', label: 'In' },
  { value: 'maybe', label: 'Maybe' },
  { value: 'no', label: 'Out' },
];

export const choiceLabel = (answer: Answer): string => CHOICES.find((c) => c.value === answer)?.label ?? 'not answered yet';

/** The run takes the caller's answer: theirs, and not done or cancelled (this or next week). */
export const answerable = (run: MemberRun, memberId: string): boolean => run.can_edit && yours(run, memberId) !== null;

/**
 * The server would take a move of this run: the caller's, this boss week's,
 * not done or cancelled, and not started (an own-time run until its day ends).
 */
export function movable(run: MemberRun, week: Pick<MemberWeek, 'days' | 'generated_at' | 'timezone'>, which: 'this' | 'next', memberId: string): boolean {
  if (which !== 'this' || isPast(run) || !yours(run, memberId)) return false;
  if (run.time === null || run.status === 'otot') {
    const today = week.days.find((d) => d.is_today)?.index ?? 0;
    return run.day >= today;
  }
  const left = minutesUntil(run, week);
  return left !== null && left > 0;
}

/** The run as it reads with the caller's answer changed (the optimistic copy; a no puts a live run at risk). */
export function withAnswer(run: MemberRun, memberId: string, answer: Choice): MemberRun {
  const participants = run.participants.map((p) => (p.id === memberId ? { ...p, answer } : p));
  const status = answer === 'no' && (run.status === 'planned' || run.status === 'confirmed') ? 'at_risk' : run.status;
  return { ...run, participants, status, tally: { ...run.tally, on: participants.filter((p) => p.answer === 'yes').length } };
}

/** "Tue 29 22:00", a slot of a week as the boards write it. */
export function slotWords(week: Pick<MemberWeek, 'days'>, slot: Slot): string {
  const day = week.days[slot.day];
  return `${day ? `${day.dow} ${day.date.slice(8, 10)}` : `day ${slot.day + 1}`} ${slot.time ?? 'own time'}`;
}

/**
 * What changed on a run since the page read it (a `409 stale`), in words for
 * the toast; "changed" when the difference is not the slot, the caller's
 * answer or their place.
 */
export function changeWords(before: MemberRun, after: MemberRun | null, week: Pick<MemberWeek, 'days'>, memberId: string): string {
  const title = runTitle(before);
  if (!after) return `${title} is no longer on the week.`;
  if (!yours(after, memberId)) return `You're no longer in ${title}.`;
  const parts: string[] = [];
  if (after.day !== before.day || after.time !== before.time) parts.push(`it moved to ${slotWords(week, after)}`);
  const was = yours(before, memberId)?.answer;
  const now = yours(after, memberId)?.answer;
  if (was !== now && now) parts.push(`your answer is now ${choiceLabel(now)}`);
  if (after.status !== before.status) parts.push(`it is now ${after.status.replace('_', ' ')}`);
  return parts.length ? `${title} changed meanwhile: ${parts.join(', ')}.` : `${title} changed meanwhile.`;
}

/** Offset of the guild's wall clock from UTC in minutes, by the server's clock. */
function offset(week: Pick<MemberWeek, 'generated_at' | 'timezone'>): number | null {
  const wall = wallMinutes(week.generated_at, week.timezone);
  const utc = Date.parse(week.generated_at);
  return wall === null || Number.isNaN(utc) ? null : wall - Math.floor(utc / 60_000);
}

/** A Discord link's `move_to` (an RFC 3339 instant) as a slot of this week; null outside it or unreadable. */
export function slotOf(moveTo: string, week: Pick<MemberWeek, 'days' | 'generated_at' | 'timezone'>): Slot | null {
  const wall = wallMinutes(moveTo, week.timezone);
  const first = week.days[0] ? dateMinutes(week.days[0].date) : null;
  if (wall === null || first === null) return null;
  const day = Math.floor((wall - first) / 1440);
  if (day < 0 || day >= week.days.length) return null;
  return { day, time: fromMinutes(wall - first - day * 1440) };
}

/** A slot of the week as the UTC instant a deep link carries (`2026-09-30T13:30:00Z`); null for own time. */
export function instantOf(slot: Slot, week: Pick<MemberWeek, 'days' | 'generated_at' | 'timezone'>): string | null {
  const day = week.days[slot.day];
  const wall = day && slot.time ? dateMinutes(day.date, slot.time) : null;
  const shift = offset(week);
  if (wall === null || shift === null) return null;
  return new Date((wall - shift) * 60_000).toISOString().replace('.000Z', 'Z');
}

/** Back on the Week with the run open and the choice picked, after the fresh sign-in. */
export function answerReturn(run: Pick<MemberRun, 'id'>, which: 'this' | 'next', answer: Choice): string {
  const query = new URLSearchParams({ ...(which === 'next' ? { week: 'next' } : {}), run: run.id, answer });
  return `/?${query}`;
}

/** Back on the run's Move page with the picked slot, after the fresh sign-in. */
export function moveReturn(run: Pick<MemberRun, 'id'>, instant: string | null): string {
  return `/runs/${encodeURIComponent(run.id)}${instant ? `?move_to=${encodeURIComponent(instant)}` : ''}`;
}

/** The answer a fresh-sign-in return carries (`?answer=`), when it is one. */
export const returnedChoice = (value: string | null): Choice | null => CHOICES.find((c) => c.value === value)?.value ?? null;

/** "Ask for a change…" / "Ask to join…" about a run: the request form picks Leave for the caller's own run, Join for anyone else's (`requests/form.ts`). */
export const askPath = (run: Pick<MemberRun, 'id'>): string => `/requests/new?${new URLSearchParams({ run: run.id })}`;

/** A change to a weekly timing, as Discord's own links ask it (`?fixed=&change=edit`). */
export const weeklyAskPath = (fixedId: string): string => `/requests/new?${new URLSearchParams({ fixed: fixedId, change: 'edit' })}`;

/** What the Move page checks about a pick (board Move `move-checks`). */
export interface MoveChecks {
  /** Inside this boss week and still ahead. */
  inside: boolean;
  /** The caller's other runs it would overlap ("HStar 21:00"): any of them blocks the move. */
  yours: string[];
  /** Teammates it would double-book ("Asahi in XBM 22:00"): a warning, never a block. */
  mates: string[];
  /** Teammates who have not answered, who may miss the new time. */
  quiet: string[];
}

export function moveChecks(run: MemberRun, slot: Slot, week: Pick<MemberWeek, 'days' | 'generated_at' | 'timezone' | 'runs'>, memberId: string): MoveChecks {
  const today = week.days.find((d) => d.is_today)?.index ?? 0;
  const left = minutesUntil(slot, week);
  const inside = slot.day >= 0 && slot.day < week.days.length && (slot.time === null ? slot.day >= today : left !== null && left > 0);
  const field = liveRuns(week.runs);
  const titles = new Map(field.map((r) => [r.id, r.title]));
  const names = namesIn(week.runs);
  // Each clash with the run it hits in words: "HStar 21:00".
  const found = clashes({ ...pickerRun(run), day: slot.day, time: slot.time }, field).map((c) => ({
    members: c.members,
    at: `${titles.get(c.with.id) ?? 'another run'} ${c.with.time ?? ''}`.trim(),
  }));
  return {
    inside,
    yours: found.filter((c) => c.members.includes(memberId)).map((c) => c.at),
    mates: found.flatMap((c) => {
      const others = c.members.filter((m) => m !== memberId);
      return others.length ? [`${others.map(names).join(', ')} in ${c.at}`] : [];
    }),
    quiet: run.participants.filter((p) => p.id !== memberId && p.answer === 'waiting').map((p) => p.name),
  };
}

/** An instant on the guild's wall clock in words: "Wed 7 Oct", "23:59", "Wednesday"; null when unreadable. */
export function wallWords(instant: string | number, timeZone: string): { date: string; time: string; weekday: string } | null {
  const at = new Date(instant);
  if (Number.isNaN(at.getTime())) return null;
  try {
    // en-US: "Sep", where en-GB now writes "Sept"; the order comes from the parts, not the locale.
    const parts = new Intl.DateTimeFormat('en-US', { timeZone, weekday: 'long', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(at);
    const part = (type: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === type)?.value ?? '';
    const weekday = part('weekday');
    return { date: `${weekday.slice(0, 3)} ${part('day')} ${part('month')}`, time: `${part('hour')}:${part('minute')}`, weekday };
  } catch {
    return null;
  }
}

/** The last minute of a boss week that resets at `endsAt` ("Wed 14 Oct 23:59"). */
export function weekEnds(endsAt: string, timeZone: string): string {
  const words = wallWords(Date.parse(endsAt) - 60_000, timeZone);
  return words ? `${words.date} ${words.time}` : '';
}

/** What a Discord link asked for ("Sunday 21:30"), in the guild's time; empty when unreadable. */
export function askedWords(moveTo: string, timeZone: string): string {
  const words = wallWords(moveTo, timeZone);
  return words ? `${words.weekday} ${words.time}` : '';
}

/**
 * What the Move page shows for a run a link names: the picker (`move`), or
 * why it can't move any more (boards Move-NotIn, Move-WeekOver, and the same
 * shape for a run that started, closed, or sits in a later boss week).
 */
export type LinkState = 'move' | 'week_over' | 'not_in' | 'closed' | 'started' | 'not_yet';

export function linkState(link: Pick<MemberRunLink, 'run' | 'week' | 'started' | 'removed'>, memberId: string): LinkState {
  if (link.week === 'past') return 'week_over';
  if (link.removed || !yours(link.run, memberId)) return 'not_in';
  if (isPast(link.run)) return 'closed';
  if (link.week !== 'current') return 'not_yet';
  return link.started ? 'started' : 'move';
}
