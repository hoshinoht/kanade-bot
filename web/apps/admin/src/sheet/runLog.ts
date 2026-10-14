/**
 * One run's change log (`GET /api/admin/history?run=<id>`) in people's words.
 * A record carries full domain rows (docs/notes/history.md); the run's own
 * before → after is its `runs` row and its `rsvps` rows. Fields are named as
 * the server's per-field index names them: `slot`, `bosses`, `participants`,
 * `channel`, `status`, `status_pin`, `rsvp:<member id>`, `attended:<member id>`.
 */
import type { ChangeRecord } from '@kanade/api-types';
import { answerWord, shortAt, statusWord, type Names } from '../history/describe';

const FIELD: Record<string, string> = {
  created: 'Created',
  slot: 'Day and time',
  bosses: 'Bosses',
  participants: 'Roster',
  channel: 'Home channel',
  status: 'Status',
  status_pin: 'Held status',
};

type Row = Record<string, unknown>;
const obj = (v: unknown): Row | null => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Row) : null);
const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

export interface FieldChange {
  field: string;
  before: unknown;
  after: unknown;
}

/** The rows of a record that belong to this run (a swap's record also holds the other run). */
export function runRows(record: ChangeRecord, runId: string): ChangeRecord['rows'] {
  return record.rows.filter((row) => {
    if (row.key.table === 'rsvps') return row.key.run_id === runId;
    if (row.key.table === 'runs') return row.key.id === runId;
    return row.key.table === 'reminders' && (row.after ?? row.before)?.run_id === runId;
  });
}

const slotOf = (row: Row) => ({ datetime: row.datetime ?? null, own: row.status === 'otot' });

/** What this record changed on the run, field by field. */
export function fieldChanges(record: ChangeRecord, runId: string): FieldChange[] {
  const out: FieldChange[] = [];
  for (const row of runRows(record, runId)) {
    if (row.key.table === 'rsvps') {
      if (!same(row.before?.state, row.after?.state)) out.push({ field: `rsvp:${row.key.user_id}`, before: row.before, after: row.after });
      continue;
    }
    if (row.key.table !== 'runs') continue;
    const { before, after } = row;
    if (!after) continue;
    if (!before) {
      out.push({ field: 'created', before: null, after });
      continue;
    }
    if (!same(slotOf(before), slotOf(after))) out.push({ field: 'slot', before: slotOf(before), after: slotOf(after) });
    for (const [field, key] of [
      ['bosses', 'bosses'],
      ['participants', 'participants'],
      ['channel', 'channel_id'],
      ['status', 'status'],
      ['status_pin', 'status_pin'],
    ] as const) {
      if (!same(before[key], after[key])) out.push({ field, before: before[key] ?? null, after: after[key] ?? null });
    }
    const marks = (r: Row) => (Array.isArray(r.attendance) ? (r.attendance as Row[]) : []);
    const ids = new Set([...marks(before), ...marks(after)].map((m) => String(m?.user_id)));
    for (const id of ids) {
      const was = marks(before).find((m) => String(m?.user_id) === id) ?? null;
      const now = marks(after).find((m) => String(m?.user_id) === id) ?? null;
      if (!same(was, now)) out.push({ field: `attended:${id}`, before: was, after: now });
    }
  }
  return out;
}

/** A history line inside the run's own log: "XKalos: 21:30 → 22:00" drops the title it repeats. */
export function inRun(line: string, title: string): string {
  // Not a bare "XKalos " lead: "XKalos → HStar" (bosses changed) needs its subject.
  for (const lead of [`${title}: `, `${title} `]) {
    if (line.startsWith(lead) && (lead.endsWith(': ') || line.startsWith(`${lead}roster:`))) {
      const rest = line.slice(lead.length);
      return rest.charAt(0).toUpperCase() + rest.slice(1);
    }
  }
  return line;
}

export function fieldLabel(field: string, names: Names): string {
  const known = FIELD[field];
  if (known) return known;
  const [kind, id] = field.split(/:(.*)/s);
  if (kind === 'rsvp' && id) return `${names(id)}'s answer`;
  if (kind === 'attended' && id) return `${names(id)}'s attendance`;
  // A field this app does not know yet still shows, under its own name.
  return field;
}

export interface ValueCtx {
  names: Names;
  timeZone: string;
  /** The run's home channel, so a matching id reads as its name. */
  channel?: { id: string; name: string };
}

export function fieldValue(field: string, value: unknown, ctx: ValueCtx): string {
  if (value === null || value === undefined) return field.startsWith('rsvp:') ? 'no answer' : field === 'status_pin' ? 'not held' : '—';
  const kind = field.split(':')[0];
  const row = obj(value);
  if (field === 'created' && row) return `${fieldValue('slot', slotOf(row), ctx)} · ${(Array.isArray(row.bosses) ? row.bosses : []).join(' + ')}`;
  if (field === 'slot' && row) {
    if (!row.datetime) return 'no time';
    return row.own ? `${shortAt(String(row.datetime), ctx.timeZone, false)} own time` : shortAt(String(row.datetime), ctx.timeZone);
  }
  if (field === 'status') return statusWord(value);
  if (field === 'status_pin' && row) return `held at ${statusWord(row.status)}`;
  if (field === 'channel') return ctx.channel && value === ctx.channel.id ? ctx.channel.name : `#${String(value)}`;
  if (kind === 'rsvp' && row) return answerWord(row.state);
  if (kind === 'attended' && row) return row.attended ? 'attended' : 'missed';
  if (field === 'bosses' && Array.isArray(value)) return value.join(' + ');
  if (field === 'participants' && Array.isArray(value)) return value.length ? value.map((id) => ctx.names(String(id))).join(', ') : 'nobody';
  return row ? JSON.stringify(row) : String(value);
}
