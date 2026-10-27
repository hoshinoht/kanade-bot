/**
 * Plain-language lines for a change record's rows (docs/notes/history.md).
 * Row values are full domain rows (`kanade.change.v1`: run instants in UTC,
 * weekly timings with Monday = 0), so a line never needs another lookup;
 * member names come from the roster when it is loaded.
 */
import type { ChangeRecord, Run, RowChange } from '@kanade/api-types';
import { runTitle as liveTitle } from '@kanade/ui';

type Row = Record<string, unknown> | null;
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
const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
export const statusWord = (v: unknown) => STATUS[str(v)] ?? str(v);
export const answerWord = (v: unknown) => ANSWER[str(v)] ?? str(v);

function parts(iso: string, timeZone: string): Record<string, string> | null {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  const out: Record<string, string> = {};
  for (const p of new Intl.DateTimeFormat('en-US', {
    timeZone,
    weekday: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(date))
    out[p.type] = p.value;
  return out;
}

/** Guild-local "Fri 25 21:30" for an instant. */
export function shortAt(iso: string, timeZone: string, time = true): string {
  const p = parts(iso, timeZone);
  if (!p) return iso;
  return `${p.weekday} ${p.day}${time ? ` ${p.hour}:${p.minute}` : ''}`;
}

/**
 * When something was last edited, guild-local: "11:48" today (by the server's
 * clock, `now`), else "Mon 22:15" (the boss week is on screen, so no date).
 */
export function editedAt(iso: string, timeZone: string, now: string): string {
  const at = shortAt(iso, timeZone);
  return now && shortAt(now, timeZone, false) === shortAt(iso, timeZone, false) ? at.slice(-5) : at.replace(/ \d{2} /, ' ');
}

/**
 * A record's boss week as its guild-local start date (`YYYY-MM-DD`), which is
 * what `weekStartLabel` and `Week.starts` use; records name it by instant.
 */
export function weekDate(week: string, timeZone: string): string {
  if (/^\d{4}-\d{2}-\d{2}$/.test(week)) return week;
  const date = new Date(week);
  if (Number.isNaN(date.getTime())) return week;
  // en-CA formats dates as YYYY-MM-DD.
  return new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(date);
}

/** "morning card", "T-1h card": the reminder kinds as the Reminders page names cards. */
export function reminderKind(kind: string): string {
  if (kind === 'day_of') return 'morning card';
  const minutes = Number(/^countdown_(\d+)$/.exec(kind)?.[1]);
  if (!Number.isFinite(minutes)) return `${kind} reminder`;
  return `T-${minutes % 60 === 0 ? `${minutes / 60}h` : `${minutes}m`} card`;
}

const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

function title(row: Row): string {
  return list(row?.bosses).join(' + ') || 'a run';
}

interface Ctx {
  names: Names;
  timeZone: string;
  runTitle: (id: string) => string;
}

function slot(row: Row, ctx: Ctx): string {
  if (!row) return '';
  const own = row.status === 'otot';
  return `${shortAt(str(row.datetime), ctx.timeZone, !own)}${own ? ' own time' : ''}`;
}

function runLines(change: RowChange, ctx: Ctx, reminders: number): string[] {
  const { before, after } = change;
  if (!before) return [`${title(after)} added on ${slot(after, ctx)}`];
  if (!after) return [`${title(before)} removed`];
  const lines: string[] = [];
  const name = title(after);
  if (!same(before.datetime, after.datetime) || (before.status === 'otot') !== (after.status === 'otot')) {
    lines.push(`${name}: ${slot(before, ctx)} → ${slot(after, ctx)}`);
  }
  if (before.status !== after.status) lines.push(`${name}: ${statusWord(before.status)} → ${statusWord(after.status)}`);
  if (!same(before.bosses, after.bosses)) lines.push(`${title(before)} → ${name}`);
  const was = list(before.participants);
  const now = list(after.participants);
  const added = now.filter((id) => !was.includes(id)).map(ctx.names);
  const removed = was.filter((id) => !now.includes(id)).map(ctx.names);
  if (added.length || removed.length) {
    lines.push(`${name} roster: ${[...added.map((n) => `+${n}`), ...removed.map((n) => `−${n}`)].join(' ')}`);
  }
  if (!same(before.channel_id, after.channel_id)) lines.push(`${name}: home channel changed`);
  if (!same(before.status_pin, after.status_pin)) {
    const pin = after.status_pin as Row;
    lines.push(pin ? `${name}: status held at ${statusWord(pin.status)}` : `${name}: status no longer held`);
  }
  const marks = (row: Row) => (Array.isArray(row?.attendance) ? (row.attendance as Row[]) : []);
  for (const entry of marks(after)) {
    const earlier = marks(before).find((e) => e?.user_id === entry?.user_id);
    if (!same(earlier, entry)) lines.push(`${ctx.names(str(entry?.user_id))} ${entry?.attended ? 'attended' : 'missed'} ${name}`);
  }
  if (reminders) lines.push(`${name}: ${reminders} reminder${reminders === 1 ? '' : 's'} re-placed`);
  return lines.length ? lines : [`${name} updated`];
}

// "Ren → on for XKalos (was out)": "for" as the reminder lines and the answer toasts say it,
// since the answer word itself can be "on".
function rsvpLine(change: RowChange, ctx: Ctx): string {
  if (!('user_id' in change.key)) return '';
  const who = ctx.names(change.key.user_id);
  const answer = change.after ? answerWord(change.after.state) : 'no answer';
  const was = change.before ? ` (was ${answerWord(change.before.state)})` : '';
  return `${who} → ${answer} for ${ctx.runTitle(change.key.run_id)}${was}`;
}

function reminderLine(change: RowChange, ctx: Ctx): string {
  const { before, after } = change;
  const row = after ?? before;
  const what = `${capital(reminderKind(str(row?.kind)))} for ${ctx.runTitle(str(row?.run_id))}`;
  const at = (v: unknown) => shortAt(str(v), ctx.timeZone);
  if (!before) return `${what} set for ${at(after?.fire_at)}`;
  if (!after) return `${what} withdrawn`;
  if (!before.sent_at && after.sent_at) return `${what} sent ${at(after.sent_at)}`;
  if (!same(before.fire_at, after.fire_at)) return `${what}: ${at(before.fire_at)} → ${at(after.fire_at)}`;
  return `${what} updated`;
}

const wall = (v: unknown) => str(v).slice(0, 5);

function fixedLines(change: RowChange): string[] {
  const { before, after } = change;
  const name = `Weekly timing ${title(after ?? before)}`;
  if (!before) return [`${name} added: ${WEEKDAYS[Number(after?.weekday)] ?? ''} ${wall(after?.time)}`];
  if (!after) return [`${name} retired`];
  const lines: string[] = [];
  if (before.weekday !== after.weekday || before.time !== after.time) {
    lines.push(`${name}: ${WEEKDAYS[Number(before.weekday)]} ${wall(before.time)} → ${WEEKDAYS[Number(after.weekday)]} ${wall(after.time)}`);
  }
  if (!same(before.bosses, after.bosses)) lines.push(`Weekly timing ${title(before)} → ${title(after)}`);
  if (list(before.participants).join() !== list(after.participants).join()) lines.push(`${name}: party changed`);
  if (!same(before.channel_id, after.channel_id)) lines.push(`${name}: home channel changed`);
  if (!same(before.note, after.note)) lines.push(`${name}: note ${after.note ? 'changed' : 'removed'}`);
  return lines.length ? lines : [`${name} updated`];
}

/**
 * Run titles by id from what a page already holds, for records that name a run only by id: the
 * newest run row among the loaded records (a past week's run), overridden by the loaded weeks' runs.
 */
export function runTitles(runs: readonly Pick<Run, 'id' | 'bosses'>[], records: readonly ChangeRecord[]): Map<string, string> {
  const titles = new Map<string, string>();
  // Records come newest first: the first row seen for a run is its latest title.
  for (const record of records) {
    for (const row of record.rows) {
      if (row.key.table === 'runs' && !titles.has(row.key.id)) titles.set(row.key.id, title(row.after ?? row.before));
    }
  }
  for (const run of runs) titles.set(run.id, liveTitle(run));
  return titles;
}

/** `runTitle` names a run the record carries no row for (the page's runs, or a run's own log); unknown runs read "a run". */
export function describe(record: ChangeRecord, names: Names, timeZone: string, runTitle?: (id: string) => string | undefined): string[] {
  // Answers and reminders name their run by id; the same record usually carries that run's row.
  const titles = new Map<string, string>();
  for (const row of record.rows) {
    if (row.key.table === 'runs') titles.set(row.key.id, title(row.after ?? row.before));
  }
  const ctx: Ctx = { names, timeZone, runTitle: (id) => titles.get(id) ?? runTitle?.(id) ?? 'a run' };
  // A move re-places its reminders: one line on the run, not one per card. The scheduler
  // withdraws the old rows and writes new ones (a rebuild), so a withdrawal, a new row and an
  // in-place change all fold; counted per card kind. A delivery's send is its own line, and a
  // run that did not move (cancelled, say) keeps a line per card.
  const moved = new Set<string>();
  for (const { key, before, after } of record.rows) {
    if (key.table === 'runs' && before && after && (!same(before.datetime, after.datetime) || (before.status === 'otot') !== (after.status === 'otot'))) moved.add(key.id);
  }
  const folds = (row: RowChange) => {
    const run = str((row.after ?? row.before)?.run_id);
    return moved.has(run) && !(row.before && row.after && !row.before.sent_at && row.after.sent_at);
  };
  const followers = new Map<string, Set<string>>();
  for (const row of record.rows) {
    if (row.key.table !== 'reminders' || !folds(row)) continue;
    const card = row.after ?? row.before;
    const run = str(card?.run_id);
    followers.set(run, (followers.get(run) ?? new Set()).add(str(card?.kind)));
  }
  return collapse(record.rows.flatMap((row) => {
    switch (row.key.table) {
      case 'runs':
        return runLines(row, ctx, followers.get(row.key.id)?.size ?? 0);
      case 'rsvps':
        return [rsvpLine(row, ctx)];
      case 'reminders':
        return folds(row) ? [] : [reminderLine(row, ctx)];
      default:
        return fixedLines(row);
    }
  }));
}

/** Identical lines (one per week a weekly timing touched) read once, with a count. */
function collapse(lines: string[]): string[] {
  const counts = new Map<string, number>();
  for (const line of lines) counts.set(line, (counts.get(line) ?? 0) + 1);
  return [...counts].map(([line, n]) => (n > 1 ? `${line} ×${n}` : line));
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
  draft_merge: 'draft merge',
  request_merge: 'request approval',
  cherry_pick: 'week-to-week copy',
};

/**
 * Who made a change. Admin ids are the server's sign-in attribution
 * (`token`, `discord:<user id>`, `tailscale:<login>`); a Discord admin reads
 * as their member name when the roster has them.
 */
export function actorName(actor: ChangeRecord['actor'], names: Names, known: (id: string) => boolean = () => false): string {
  if (actor.kind === 'member') return names(actor.id);
  if (actor.kind === 'admin') {
    if (actor.id === 'token') return 'Admin (token)';
    const [method, subject = ''] = actor.id.split(/:(.*)/s);
    if (method === 'discord' && subject) return known(subject) ? names(subject) : `Admin (Discord ${subject})`;
    if (method === 'tailscale' && subject) return `Admin (${subject})`;
    return `Admin (${actor.id})`;
  }
  return `system (${actor.id})`;
}

/**
 * "12 min ago", "3 h ago", "2 d ago" from `now` (the server's clock, e.g. the
 * week's `generated_at`, so a skewed device clock cannot move it).
 */
export function relativeAt(iso: string, now: string): string {
  const ms = Date.parse(now) - Date.parse(iso);
  if (Number.isNaN(ms)) return '';
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days < 14) return `${days} d ago`;
  return `${Math.floor(days / 7)} wk ago`;
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
