/**
 * "Edit, then approve" for Kanade's proposals (admin-api "Inbox (A6)"): the
 * typed day and time replace the proposed instant inside ITS boss week, which
 * may not be the week on screen. `day` counts from that week's reset day;
 * `time` is always `HH:MM`.
 */
import type { Proposal, WeekDay } from '@kanade/api-types';
import { parseWhen } from '../sheet/parseWhen';

const DOWS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
/** `Fri 25 Sep 22:00`, the server's `when`. */
const WHEN = /^([A-Z][a-z]{2}) \d{2} [A-Z][a-z]{2} (\d{2}:\d{2})$/;

/** Only these carry a time the server lets an edit replace. */
export const editable = (p: Proposal) => p.tab === 'extractor' && ['move', 'add', 'split'].includes(p.kind) && !p.flags.includes('expired');

export type Edit = { ok: true; day: number; time: string } | { ok: false; message: string };

/** `resetDow` is the boss week's first day (`Thu` here), from the week on screen. */
export function parseEdit(text: string, p: Proposal, resetDow: string): Edit {
  const proposed = WHEN.exec(p.when);
  const reset = DOWS.indexOf(resetDow.slice(0, 3));
  if (!proposed || reset < 0) return { ok: false, message: 'The proposed time cannot be read; approve it as it is or reject it.' };
  // The proposal's week by weekday names: the calendar dates do not matter.
  const days = Array.from({ length: 7 }, (_, index) => ({ index, dow: DOWS[(reset + index) % 7]!, date: '', is_reset: index === 0, is_today: false }) satisfies WeekDay);
  const day = (DOWS.indexOf(proposed[1]!) - reset + 7) % 7;
  const parsed = parseWhen(text, days, { day, time: proposed[2]! });
  if (!parsed.ok) return parsed;
  if (!parsed.slot.time) return { ok: false, message: 'Give a time, for example "21:30".' };
  return { ok: true, day: parsed.slot.day, time: parsed.slot.time };
}
