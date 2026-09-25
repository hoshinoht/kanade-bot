/**
 * Plain-language lines for a change record's rows (docs/v5/history.md).
 * Row values are full domain rows, so a line never needs another lookup;
 * member names come from the roster when it is loaded.
 */
import type { ChangeRecord, RowChange } from '@kanade/api-types';

type Row = Record<string, unknown> | null;
const DOW = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'];
const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
const STATUS: Record<string, string> = {
  planned: 'unconfirmed',
  confirmed: 'confirmed',
  at_risk: 'at risk',
  otot: 'own time',
  done: 'done',
  cancelled: 'cancelled',
};
const ANSWER: Record<string, string> = { yes: 'on', no: 'out', maybe: 'maybe' };

export type Names = (id: string) => string;

const str = (v: unknown) => (typeof v === 'string' ? v : v === null || v === undefined ? '' : String(v));
const list = (v: unknown) => (Array.isArray(v) ? v.map(str) : []);

/** "Tue 29" from a run row's boss week start and day index. */
export function dayOf(row: Row): string {
  if (!row) return '';
  const day = Number(row.day ?? 0);
  const start = str(row.week);
  const date = /^\d{4}-\d{2}-\d{2}$/.test(start) ? new Date(`${start}T00:00:00Z`) : null;
  if (date) date.setUTCDate(date.getUTCDate() + day);
  return `${DOW[day] ?? '?'}${date ? ` ${String(date.getUTCDate()).padStart(2, '0')}` : ''}`;
}

function title(row: Row): string {
  return list(row?.bosses).join(' + ') || 'a run';
}

function slot(row: Row): string {
  if (!row) return '';
  return `${dayOf(row)} ${row.status === 'otot' || !row.time ? 'own time' : str(row.time)}`;
}

function runLines(change: RowChange, names: Names): string[] {
  const { before, after } = change;
  if (!before) return [`${title(after)} added on ${slot(after)}`];
  if (!after) return [`${title(before)} removed`];
  const lines: string[] = [];
  const name = title(after);
  if (before.day !== after.day || before.time !== after.time) lines.push(`${name}: ${slot(before)} → ${slot(after)}`);
  if (before.status !== after.status) lines.push(`${name}: ${STATUS[str(before.status)] ?? before.status} → ${STATUS[str(after.status)] ?? after.status}`);
  const was = list(before.participants);
  const now = list(after.participants);
  const added = now.filter((id) => !was.includes(id)).map(names);
  const removed = was.filter((id) => !now.includes(id)).map(names);
  if (added.length || removed.length) {
    lines.push(`${name} roster: ${[...added.map((n) => `+${n}`), ...removed.map((n) => `−${n}`)].join(' ')}`);
  }
  return lines.length ? lines : [`${name} updated`];
}

function rsvpLine(change: RowChange, names: Names, runTitle: (id: string) => string): string {
  if (!('user_id' in change.key)) return '';
  const who = names(change.key.user_id);
  const answer = change.after ? (ANSWER[str(change.after.answer)] ?? str(change.after.answer)) : 'no answer';
  const was = change.before ? ` (was ${ANSWER[str(change.before.answer)] ?? str(change.before.answer)})` : '';
  return `${who} → ${answer} on ${runTitle(change.key.run_id)}${was}`;
}

function fixedLines(change: RowChange): string[] {
  const { before, after } = change;
  const name = `Weekly timing ${title(after ?? before)}`;
  if (!before) return [`${name} added: ${WEEKDAYS[Number(after?.weekday)] ?? ''} ${str(after?.time)}`];
  if (!after || after.retired) return [`${name} retired`];
  const lines: string[] = [];
  if (before.weekday !== after.weekday || before.time !== after.time) {
    lines.push(`${name}: ${WEEKDAYS[Number(before.weekday)]} ${str(before.time)} → ${WEEKDAYS[Number(after.weekday)]} ${str(after.time)}`);
  }
  if (list(before.participants).join() !== list(after.participants).join()) lines.push(`${name}: party changed`);
  return lines.length ? lines : [`${name} updated`];
}

export function describe(record: ChangeRecord, names: Names): string[] {
  // An RSVP row names its run by id; the same record usually carries that run's row.
  const titles = new Map<string, string>();
  for (const row of record.rows) {
    if (row.key.table === 'runs') titles.set(row.key.id, title(row.after ?? row.before));
  }
  const runTitle = (id: string) => titles.get(id) ?? `run ${id}`;
  return record.rows.flatMap((row) => {
    switch (row.key.table) {
      case 'runs':
        return runLines(row, names);
      case 'rsvps':
        return [rsvpLine(row, names, runTitle)];
      default:
        return fixedLines(row);
    }
  });
}

export const SURFACE_LABELS: Record<string, string> = {
  discord: 'Discord',
  admin_portal: 'admin portal',
  public_portal: 'public portal',
  cli: 'CLI',
  chat_approval: 'chat approval',
  extraction_approval: 'extraction approval',
  delivery_tick: 'reminder delivery',
  rollback: 'rollback',
  import: 'import',
};

export function actorName(actor: ChangeRecord['actor'], names: Names): string {
  if (actor.kind === 'member') return names(actor.id);
  if (actor.kind === 'admin') return actor.id === 'admin-token' ? 'admin token' : `admin ${actor.id}`;
  return `system (${actor.id})`;
}

/** Guild-local "Tue 29 Sep 12:00" for an ISO instant. */
export function localAt(iso: string, timeZone: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone,
    weekday: 'short',
    day: '2-digit',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(date);
  const part = (type: string) => parts.find((p) => p.type === type)?.value ?? '';
  return `${part('weekday')} ${part('day')} ${part('month')} ${part('hour')}:${part('minute')}`;
}
