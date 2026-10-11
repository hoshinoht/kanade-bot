/**
 * "Edit before approving" a member request (boards B_InboxEdit*): an admin
 * may give a weekly day/time a new value and leave changes out (user
 * decision 2026-10-11: day/time plus dropping only). Days are 0 = Monday,
 * as `RequestedChange.day` and the approve body's `edit.retime[].day`.
 */
import type { Proposal, RequestedChange, RequestEditBody } from '@kanade/api-types';

export const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

export interface RequestSlot {
  day: number;
  time: string;
}

/** What the admin has done to the request so far. */
export interface RequestEdit {
  retimed: Record<string, RequestSlot>;
  dropped: Record<string, true>;
}

/** Whether there is anything to edit: a day/time to change, or one of several changes to leave out. */
export function requestEditable(p: Proposal): boolean {
  const asked = p.requested ?? [];
  return p.source === 'self_service' && asked.length > 0 && (asked.some((c) => c.retimable) || asked.length > 1);
}

/** "Sat 22:00". */
export const slotText = (slot: RequestSlot) => `${WEEKDAYS[slot.day]?.slice(0, 3) ?? ''} ${slot.time}`;

/** The change's day/time as the admin set it, else as the member asked. */
export function slotOf(change: RequestedChange, edit: RequestEdit): RequestSlot | null {
  return edit.retimed[change.id] ?? (change.day !== null && change.time ? { day: change.day, time: change.time } : null);
}

/** A new day/time that differs from what the member asked. */
export function retimed(change: RequestedChange, edit: RequestEdit): boolean {
  const slot = edit.retimed[change.id];
  return Boolean(slot && (slot.day !== change.day || slot.time !== change.time));
}

/** The approve body's `edit`: only real changes (a retime back to the asked time sends nothing). */
export function editBody(asked: RequestedChange[], edit: RequestEdit): RequestEditBody {
  return {
    retime: asked.filter((c) => !edit.dropped[c.id] && retimed(c, edit)).map((c) => ({ change: c.id, ...edit.retimed[c.id]! })),
    drop: asked.filter((c) => edit.dropped[c.id]).map((c) => c.id),
  };
}

/** The approval approves something: leaving every change out is a rejection. */
export const approvesSomething = (asked: RequestedChange[], edit: RequestEdit) => asked.some((c) => !edit.dropped[c.id]);

const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

/**
 * What the member will be told (drafts README question 9, the server's
 * "approved with changes by an admin" notice): each retimed change's new
 * value, and the changes left out in their own words.
 */
export function memberNote(member: string, subject: string, asked: RequestedChange[], edit: RequestEdit): string {
  const moved = asked.filter((c) => !edit.dropped[c.id] && retimed(c, edit)).map((c) => `${subject} weekly moves to ${slotText(edit.retimed[c.id]!)}.`);
  const left = asked.filter((c) => edit.dropped[c.id]).map((c) => `${capital(c.label)} was not approved.`);
  return [`${member} sees the version you approve, marked “edited by an admin”:`, ...moved, ...left].join(' ');
}
