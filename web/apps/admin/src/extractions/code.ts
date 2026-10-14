/**
 * Pure helpers for the Extractions list-detail: the `?call=` selection in the
 * page's query string, the code viewer's lines and find matches, and the
 * outcome words on list rows.
 */
import type { ExtractionRow } from '@kanade/api-types';
import { OUTCOME_LABEL, outcomeTone } from '../logs/filters';

/** `search` with `call` set (or dropped when empty); other keys keep their order. */
export function withCall(search: string, call: string): string {
  const query = new URLSearchParams(search);
  if (call) query.set('call', call);
  else query.delete('call');
  const text = query.toString();
  return text ? `?${text}` : '';
}

export const callOf = (search: string): string => new URLSearchParams(search).get('call') ?? '';

/** A list row's outcome pill: "n changes" (success), "no change" (neutral), anything else in words. */
export function rowOutcome(row: Pick<ExtractionRow, 'outcome' | 'changes'>): { text: string; tone: ReturnType<typeof outcomeTone> } {
  if (row.outcome === 'proposed' && row.changes > 0) return { text: `${row.changes} change${row.changes === 1 ? '' : 's'}`, tone: 'success' };
  if (row.outcome === 'no_change' || (row.outcome === 'proposed' && row.changes === 0)) return { text: 'no change', tone: 'neutral' };
  return { text: OUTCOME_LABEL[row.outcome] ?? row.outcome, tone: outcomeTone(row.outcome) };
}

/** A stored JSON response pretty-printed; anything that is not JSON as it came. */
export function pretty(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

export interface Segment {
  text: string;
  hit: boolean;
}

/** One line split around case-insensitive matches of `find` (an empty find: the line whole). */
export function segments(line: string, find: string): Segment[] {
  const needle = find.toLowerCase();
  if (!needle) return [{ text: line, hit: false }];
  const hay = line.toLowerCase();
  const out: Segment[] = [];
  let at = 0;
  for (let i = hay.indexOf(needle); i >= 0; i = hay.indexOf(needle, i + needle.length)) {
    if (i > at) out.push({ text: line.slice(at, i), hit: false });
    out.push({ text: line.slice(i, i + needle.length), hit: true });
    at = i + needle.length;
  }
  if (at < line.length || !out.length) out.push({ text: line.slice(at), hit: false });
  return out;
}

/** How many times `find` occurs in `text`, case-insensitively. */
export function matches(text: string, find: string): number {
  const needle = find.toLowerCase();
  if (!needle) return 0;
  const hay = text.toLowerCase();
  let n = 0;
  for (let i = hay.indexOf(needle); i >= 0; i = hay.indexOf(needle, i + needle.length)) n++;
  return n;
}

/** "21:40" in the guild's zone; text that is not an instant as it came. */
export function clock(at: string, timeZone: string): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return at;
  return new Intl.DateTimeFormat('en-GB', { timeZone, hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(date);
}

/** "21:40–22:00" in the guild's zone over the messages' times; one time alone when they share it. */
export function timeRange(times: string[], timeZone: string): string {
  const sorted = times.filter((t) => !Number.isNaN(new Date(t).getTime())).sort((a, b) => new Date(a).getTime() - new Date(b).getTime());
  if (!sorted.length) return '';
  const first = clock(sorted[0]!, timeZone);
  const last = clock(sorted[sorted.length - 1]!, timeZone);
  return first === last ? first : `${first}–${last}`;
}
