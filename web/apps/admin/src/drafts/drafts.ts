/**
 * Drafts (boards B_Drafts*, docs/v5/api-schemas/drafts.json): words and
 * small rules the page, the run sheet's staging and their tests share.
 */
import type { DraftConflict, DraftRow, StagedOpRow } from '@kanade/api-types';
import { shortAt } from '../history/describe';

export const DRAFTS_PATH = '/api/admin/drafts';
export const draftPath = (id: string, rest = '') => `${DRAFTS_PATH}/${encodeURIComponent(id)}${rest}`;

/**
 * When a draft was last edited, guild-local: "11:48" today (by the server's
 * clock, `now`), else "Mon 22:15" (the boss week is on screen, so no date).
 */
export function editedAt(iso: string, timeZone: string, now: string): string {
  const at = shortAt(iso, timeZone);
  return now && shortAt(now, timeZone, false) === shortAt(iso, timeZone, false) ? at.slice(-5) : at.replace(/ \d{2} /, ' ');
}

/** The kind chip of a staged operation (its codec kind). */
const KIND: Record<string, string> = {
  amend_run: 'Move',
  swap_participants: 'Party',
  set_status: 'Status',
  create_run: 'Add run',
  reset_to_fixed: 'Reset',
  revive_run: 'Revive',
  fixed_participants: 'Weekly party',
  apply_fixed_edit: 'Weekly timing',
  add_fixed_run: 'New weekly',
};
export const kindLabel = (kind: string) => KIND[kind] ?? kind.replaceAll('_', ' ');

/** Staged changes count from 1 on screen; the API numbers them from 0. */
export const changeNo = (ord: number) => ord + 1;

/** "change 3", "changes 1, 2". */
export function changesLabel(ords: number[]): string {
  const shown = ords.map(changeNo);
  return `${shown.length === 1 ? 'change' : 'changes'} ${shown.join(', ')}`;
}

/** "1", "3 and 4", "1, 2 and 5". */
export function andList(items: (string | number)[]): string {
  const words = items.map(String);
  return words.length < 2 ? (words[0] ?? '') : `${words.slice(0, -1).join(', ')} and ${words.at(-1)}`;
}

export type Answer = 'keep' | 'take';

/**
 * A conflict's two answers in words, `keep` first for a both-changed field
 * (Keep mine / Take Ren's) and `take` first for an upstream removal (Drop 3
 * and 4 / Revive the run), as the board orders them. `short` (phones) leaves
 * out which changes "Take theirs" drops; the flag on each change says it.
 */
export function answers(conflict: DraftConflict, short = false): { value: Answer; label: string }[] {
  const who = conflict.record?.actor.name;
  const ords = andList(conflict.ops.map(changeNo));
  const keep = conflict.kind === 'upstream_removed' ? 'Revive the run' : 'Keep mine';
  const take =
    conflict.kind === 'upstream_removed'
      ? `Drop ${ords}`
      : `Take ${who ? `${who}’s` : 'theirs'}${conflict.ops.length && !short ? ` (drops ${ords})` : ''}`;
  const all = { keep: { value: 'keep' as const, label: keep }, take: { value: 'take' as const, label: take } };
  const order: Answer[] = conflict.kind === 'upstream_removed' ? ['take', 'keep'] : ['keep', 'take'];
  return order.filter((a) => conflict.settle.includes(a)).map((a) => all[a]);
}

/** "Cancelled in #39": what removed the row upstream, and where. */
export function removalLabel(conflict: DraftConflict): string {
  const what = { deleted: 'Deleted', retired: 'Retired', cancelled: 'Cancelled', done: 'Done', left_run: 'Left the run' }[conflict.removal ?? 'deleted'];
  return conflict.record ? `${what} in #${conflict.record.seq}` : what;
}

/** Every conflict answered: a rebase can name them all. */
export const settled = (conflicts: DraftConflict[], picks: Record<string, Answer>) => conflicts.every((c) => picks[c.key] !== undefined);

/** The first conflict naming each op, by ord, for the staged list's flag line. */
export function flagsByOp(conflicts: DraftConflict[]): Map<number, DraftConflict> {
  const out = new Map<number, DraftConflict>();
  for (const c of conflicts) for (const ord of c.ops) if (!out.has(ord)) out.set(ord, c);
  return out;
}

/** The flag line under a conflicting op: "Conflict · slot (Ren, #38)", "Can’t apply · run cancelled (#39)". */
export function opFlag(conflict: DraftConflict): string {
  const record = conflict.record;
  if (conflict.kind === 'both_changed') return `Conflict · ${conflict.field ?? 'changed'}${record ? ` (${record.actor.name}, #${record.seq})` : ''}`;
  if (conflict.kind === 'upstream_removed') {
    const how = conflict.removal === 'left_run' ? 'left' : (conflict.removal ?? 'removed');
    return `Can’t apply · run ${how}${record ? ` (#${record.seq})` : ''}`;
  }
  return conflict.message;
}

/**
 * Whether the run sheet can stage into a draft for this boss week (`week`
 * = `Week.starts`), and why not: weekly-only drafts hold no run change, and
 * a week draft holds only its own boss week. A new draft has no week yet.
 */
export function stageable(draft: DraftRow, week: string): { ok: true } | { ok: false; reason: string } {
  if (draft.status !== 'open') return { ok: false, reason: 'Closed' };
  if (draft.scope === 'weekly') return { ok: false, reason: 'Weekly timings only: can’t hold a run change' };
  if (draft.week && draft.week !== week) return { ok: false, reason: `Boss week of ${shortDate(draft.week)}: can’t hold this run` };
  return { ok: true };
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const DOW = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
/** "Thu 24 Sep" for a guild date `YYYY-MM-DD`. */
export function shortDate(date: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!m) return date;
  const day = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])));
  return `${DOW[day.getUTCDay()]} ${Number(m[3])} ${MONTHS[day.getUTCMonth()]}`;
}

/** The run a draft's first op acts on (its portrait leads the row). */
export const firstRun = (ops: StagedOpRow[]) => ops.find((op) => op.run_id)?.run_id ?? null;

/** "merged as #40", "discarded", "expired at the boss reset". */
export function closedLabel(draft: DraftRow): string {
  if (draft.status === 'merged') return draft.merged_seq === null ? 'merged' : `merged as #${draft.merged_seq}`;
  if (draft.status === 'expired') return 'expired at the boss reset';
  return 'discarded';
}
