/**
 * Guides (module `guides`, boards G_* in docs/research/2026-10-16-guide-mockups,
 * API src/api/admin/guides): the words and small rules the page, the editor,
 * the plan and the run pane share.
 */
import type { GuideBlock, GuidePlan, GuidePlanOp, GuideRow, GuideRun, GuideStopReason, PreviewSlot } from '@kanade/api-types';
import type { Change } from '../config/dirty';
import { editedAt } from '../history/describe';
import { cardLines, type Run } from '../reminders/discord';

export const GUIDES_PATH = '/api/admin/guides';
export const guidePath = (id: string, rest = '') => `${GUIDES_PATH}/${encodeURIComponent(id)}${rest}`;

/** Discord's limit for one message's text, in UTF-16 units (`split::units`). */
export const CONTENT_LIMIT = 2000;
export const NOTE_LIMIT = 1000;

/** Length as Discord counts it: UTF-16 units, so an emoji counts 2. */
export const units = (text: string) => text.length;

/** "1,599": the counters' and plan's numbers. */
export const count = (n: number) => n.toLocaleString('en-US');

export const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The list chip (approved default 2): tone and words. */
export const STATUS: Record<GuideRow['status'], { tone: 'ok' | 'warn' | 'risk' | 'neutral'; word: string }> = {
  published: { tone: 'ok', word: 'Published' },
  out_of_date: { tone: 'warn', word: 'Out of date' },
  publishing: { tone: 'neutral', word: 'Publishing' },
  needs_attention: { tone: 'risk', word: 'Needs attention' },
  blocked: { tone: 'risk', word: 'Blocked' },
  not_published: { tone: 'neutral', word: 'Not published' },
};

export function statusWord(row: GuideRow): string {
  const { word } = STATUS[row.status];
  return row.status === 'publishing' && row.progress?.total ? `${word} ${row.progress.done} of ${row.progress.total}` : word;
}

/** Why the guide needs a look, as the row's second line says it; `null` when nothing to say. */
export function reasonText(row: GuideRow): string | null {
  switch (row.reason) {
    case 'draft_changed':
      return 'Draft changed since publish';
    case 'channel_changed':
      return `Channel changed${row.channel_name ? `: ${row.channel_name}` : ''}`;
    case 'art_changed':
      return 'Art changed since publish';
    case 'boss_facts_changed':
      return 'Boss facts changed since publish';
    case 'message_deleted':
      return 'A message was deleted in Discord';
    case 'unconfirmed_message':
      return `Message ${(row.position ?? 0) + 1} unconfirmed`;
    case 'channel_missing':
      return 'The channel is gone or hidden from Kanade';
    case 'discord_refused':
      return `Discord refused the last publish${row.channel_name ? ` in ${row.channel_name}` : ''}`;
    case 'draft_invalid':
      return 'The saved draft no longer renders';
    default:
      return null;
  }
}

/** The row's time: what last happened, guild-local ("published 12:02", "stopped 11:41", "edited Mon 21:30"). */
export function rowWhen(row: GuideRow, timeZone: string, now: string): string {
  const run = row.last_run;
  if (row.status === 'publishing' && run) return `since ${editedAt(run.requested_at, timeZone, now)}`;
  if (run?.finished_at && run.finished_at >= row.updated_at) {
    const verb = run.state === 'done' ? (run.kind === 'unpublish' ? 'unpublished' : run.kind === 'search' ? 'searched' : 'published') : run.state === 'cancelled' ? 'cancelled' : 'stopped';
    return `${verb} ${editedAt(run.finished_at, timeZone, now)}`;
  }
  return `edited ${editedAt(row.updated_at, timeZone, now)}`;
}

/** A new block of `kind` with a fresh id, as Add makes it. */
export function newBlock(kind: GuideBlock['kind']): GuideBlock {
  const id = crypto.randomUUID();
  if (kind === 'text') return { kind, id, content: '' };
  if (kind === 'bosses') return { kind, id, entries: [] };
  return { kind, id };
}

export const KIND_LABEL: Record<GuideBlock['kind'], string> = { text: 'Text', bosses: 'Bosses', spacer: 'Spacer' };

/** Markdown markers off: a summary line or an op's name. */
export const plain = (text: string) => text.replace(/^(#+|-#|-) /gm, '').replace(/\*\*/g, '');

/** A closed text block's first two lines. */
export const textSummary = (content: string) =>
  plain(content)
    .split('\n')
    .filter((line) => line.trim())
    .slice(0, 2)
    .join(' · ');

/** "message 2", "messages 3–4": which messages a block renders to. */
export function messagesLabel(positions: number[]): string {
  if (!positions.length) return '';
  const first = positions[0]! + 1;
  const last = positions[positions.length - 1]! + 1;
  return first === last ? `message ${first}` : `messages ${first}–${last}`;
}

/** "1, 4 and 5". */
export function listWords(items: (string | number)[]): string {
  const words = items.map(String);
  return words.length < 2 ? (words[0] ?? '') : `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;
}

/** One preview line: a heading, subtext, a list item or plain text, in bold/plain runs. */
export type SlotLine = { kind: 'h1' | 'h2' | 'sub' | 'li' | 'text'; runs: Run[] };

const MARKERS: [string, SlotLine['kind']][] = [
  ['## ', 'h2'],
  ['# ', 'h1'],
  ['-# ', 'sub'],
  ['- ', 'li'],
];

/** Discord Markdown as the preview draws it: Reminders' bold runs plus headings, `-#` subtext and list items. */
export function slotLines(content: string): SlotLine[] {
  return content.split('\n').map((line) => {
    const [marker, kind] = MARKERS.find(([m]) => line.startsWith(m)) ?? ['', 'text'];
    return { kind, runs: cardLines(line.slice(marker.length))[0] ?? [] };
  });
}

/** The spacer's zero-width space. */
export const SPACER = '\u200b';

/**
 * An embed description's knowledge lead: the part after the facts and
 * before the admin's note (render.rs: facts, "\n\n" lead, "\n\n" note).
 */
export function leadOf(description: string, note: string | undefined): string | null {
  const parts = description.split('\n\n');
  if (note && parts[parts.length - 1] === note) parts.pop();
  return parts.length > 1 ? parts.slice(1).join('\n\n') : null;
}

/** "…4821": a message id's tail, to match against Discord (desktop plan only). */
export const tail = (messageId: string | null) => (messageId ? `…${messageId.slice(-4)}` : '');

/** An op's name: "Text · Before you join", or the message it deletes. */
export function opLabel(op: GuidePlanOp): string {
  if (op.block_kind) return `${KIND_LABEL[op.block_kind]}${op.summary ? ` · ${plain(op.summary)}` : ''}`;
  return `Message ${op.position + 1}`;
}

/** The plan's op words, as the boards name them. */
export const OP_WORD: Record<GuidePlanOp['kind'] | 'keep', string> = { edit: 'Edit', create: 'Add', delete: 'Delete', forget: 'Forget', keep: 'Keep' };

/** The desired messages no op touches (already as posted), 0-based, in the target channel. */
export function keptPositions(plan: GuidePlan): number[] {
  const touched = new Set(plan.ops.filter((op) => op.channel_id === plan.channel_id && op.kind !== 'delete' && op.kind !== 'forget').map((op) => op.position));
  return plan.slots.map((slot) => slot.position).filter((position) => !touched.has(position));
}

/** Why a stopped run stopped, in words for the run pane's flash. */
export function stopWords(reason: GuideStopReason | null, run: GuideRun, channel: string): string {
  const stuck = run.ops.find((op) => op.outcome === 'unknown' || op.outcome === 'rejected' || op.outcome === 'gone');
  const n = stuck ? stuck.position + 1 : '?';
  switch (reason) {
    case 'plan_changed':
      return stuck
        ? `Message ${n} is gone from ${channel}. Someone deleted it while Kanade was editing. Nothing else was sent. Review the plan again.`
        : 'The guide or its channel changed after you confirmed the plan. Nothing was sent. Review the plan again.';
    case 'unknown_slot':
      return `Kanade couldn’t confirm message ${n} was posted, so nothing after it was sent. Settle it before publishing again.`;
    case 'edit_unconfirmed':
      return `Discord didn’t confirm editing message ${n}. Publish again to retry; repeating an edit is safe.`;
    case 'delete_unconfirmed':
      return `Discord didn’t confirm deleting message ${n}, so nothing new was posted. Publish again to retry; it checks first.`;
    case 'rejected':
      return `Discord refused message ${n}: Kanade is missing a permission in ${channel}. Fix the channel’s permissions, then publish again.`;
    case 'discord_unavailable':
      return 'Discord was unavailable, so the rest wasn’t sent. Publish again later.';
    case 'interrupted':
      return 'Kanade restarted during publishing. Publish again to finish.';
    default:
      return 'Publishing stopped.';
  }
}

/** "7 added · 2 edited": what a run's settled ops did, for the summary chips and the toast. */
export function runSummary(run: GuideRun): string {
  const done = run.ops.filter((op) => op.outcome === 'done' || op.outcome === 'recovered' || op.outcome === 'gone');
  const tally = (kind: GuideRun['ops'][number]['kind']) => done.filter((op) => op.kind === kind).length;
  const parts = [
    [tally('create'), 'added'],
    [tally('edit'), 'edited'],
    [tally('delete'), 'deleted'],
  ]
    .filter(([n]) => n)
    .map(([n, word]) => `${n} ${word}`);
  return parts.length ? parts.join(' · ') : 'nothing changed';
}

/**
 * What the save bar lists: each unsaved change, in block order ("block 2
 * 1,553 → 1,599" for text, "new" for an added block), then the channel and
 * title.
 */
export function blockChanges(saved: GuideBlock[], draft: GuideBlock[], names: { channel: [string, string]; title: [string, string] }): Change[] {
  const out: Change[] = [];
  const before = new Map(saved.map((b, i) => [b.id, { block: b, i }]));
  draft.forEach((block, i) => {
    const was = before.get(block.id);
    const label = `block ${i + 1}`;
    if (!was) out.push({ label, from: '—', to: `new ${KIND_LABEL[block.kind].toLowerCase()}` });
    else if (JSON.stringify(was.block) !== JSON.stringify(block))
      out.push(block.kind === 'text' && was.block.kind === 'text' ? { label, from: count(units(was.block.content)), to: count(units(block.content)) } : { label, from: 'saved', to: 'edited' });
    else if (was.i !== i) out.push({ label, from: `block ${was.i + 1}`, to: `block ${i + 1}` });
  });
  const kept = new Set(draft.map((b) => b.id));
  saved.forEach((block, i) => {
    if (!kept.has(block.id)) out.push({ label: `block ${i + 1}`, from: KIND_LABEL[block.kind].toLowerCase(), to: 'removed' });
  });
  if (names.channel[0] !== names.channel[1]) out.push({ label: 'channel', from: names.channel[0], to: names.channel[1] });
  if (names.title[0] !== names.title[1]) out.push({ label: 'title', from: names.title[0], to: names.title[1] });
  return out;
}

/** Which preview messages each block renders to, by block id. */
export function positionsByBlock(slots: PreviewSlot[]): Map<string, number[]> {
  const out = new Map<string, number[]>();
  for (const slot of slots) out.set(slot.block_id, [...(out.get(slot.block_id) ?? []), slot.position]);
  return out;
}
