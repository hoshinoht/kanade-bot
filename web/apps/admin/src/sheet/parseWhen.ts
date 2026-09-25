/**
 * The run sheet's Move field, as v4's `/amend` read it: "wed 21:30",
 * "fri 9:45pm", "22:00" (same day), "tue" (same time). Day names resolve
 * inside the boss week on screen, never across weeks.
 */
import type { WeekDay } from '@kanade/api-types';
import type { Slot } from '../planner/keyboardMove';

export type Parsed = { ok: true; slot: Slot } | { ok: false; message: string };

const PATTERN = /^(?:(mon|tue|wed|thu|fri|sat|sun)[a-z]*\.?)?\s*(?:(\d{1,2})(?::(\d{2}))?\s*(am|pm)?)?$/;

export function parseWhen(text: string, days: WeekDay[], current: Slot): Parsed {
  const input = text.trim().toLowerCase();
  const m = PATTERN.exec(input);
  if (!input || !m || (!m[1] && !m[2])) {
    return { ok: false, message: 'Write a day and/or a time, for example "wed 21:30" or "9:45pm".' };
  }
  const [, dow, hourText, minuteText, meridiem] = m;
  let day = current.day;
  if (dow) {
    const found = days.find((d) => d.dow.toLowerCase().startsWith(dow));
    if (!found) return { ok: false, message: `There is no ${dow} in this boss week.` };
    day = found.index;
  }
  let time = current.time;
  if (hourText) {
    if (!minuteText && !meridiem) return { ok: false, message: 'Give minutes or am/pm, for example "21:00" or "9pm".' };
    let hour = Number(hourText);
    const minute = Number(minuteText ?? '0');
    if (meridiem) {
      if (hour < 1 || hour > 12) return { ok: false, message: 'With am/pm the hour is 1 to 12.' };
      hour = (hour % 12) + (meridiem === 'pm' ? 12 : 0);
    }
    if (hour > 23 || minute > 59) return { ok: false, message: 'Times run from 00:00 to 23:59.' };
    time = `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`;
  }
  return { ok: true, slot: { day, time } };
}
