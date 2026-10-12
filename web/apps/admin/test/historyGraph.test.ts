import { describe, expect, it } from 'vitest';
import type { ChangeRecord, GraphDraft } from '@kanade/api-types';
import { FOLD_MIN, layout, laneCount, tipLine, tipState, type GraphInput, type Row } from '../src/history/graph';

// Records #1..#n, one minute apart, newest first like a History page.
function records(top: number, bottom: number): ChangeRecord[] {
  const out: ChangeRecord[] = [];
  for (let seq = top; seq >= bottom; seq--)
    out.push({
      format: 'kanade.change.v1',
      seq,
      id: `r${seq}`,
      revision: seq,
      at: new Date(Date.UTC(2026, 8, 27, 0, seq)).toISOString(),
      actor: { kind: 'member', id: '1005' },
      surface: seq === 33 ? 'draft_merge' : 'discord',
      request_id: null,
      weeks: [],
      rows: [],
      notices: [],
      refs: [],
      prev_hash: '',
      hash: `h${seq}`,
    });
  return out;
}
const minute = (seq: number) => new Date(Date.UTC(2026, 8, 27, 0, seq, 30)).toISOString();
function draft(id: string, over: Partial<GraphDraft>): GraphDraft {
  return {
    id,
    short_id: id,
    title: id,
    titled: true,
    status: 'open',
    version: 2,
    base: 1,
    up_to_date: false,
    scope: 'week',
    week: '2026-09-24',
    staged: 1,
    author: { kind: 'admin', id: 'discord:1001', name: 'Asahi' },
    created_at: minute(0),
    updated_at: minute(0),
    merged_seq: null,
    closed_by: null,
    close_reason: null,
    closed_at: null,
    ...over,
  };
}
// The boards' drafts on #20..#39 (the first page): three open, one merged, one discarded, one from below.
const BOARD = [
  draft('Weekend', { base: 36 }),
  draft('Bellona', { base: 38 }),
  draft('Kalos', { base: 34, scope: 'weekly', week: null }),
  draft('Tuesday', { status: 'merged', base: 31, merged_seq: 33, closed_at: minute(33) }),
  draft('Scratch', { status: 'discarded', base: 32, closed_at: minute(35) }),
  draft('Untitled', { status: 'expired', base: 12, closed_at: minute(24) }),
];
const input = (over: Partial<GraphInput> = {}): GraphInput => ({
  records: records(39, 20),
  head: 39,
  drafts: BOARD,
  more: true,
  conflicts: {},
  expanded: new Set(),
  tipsCollapsed: false,
  keep: new Set(),
  ...over,
});
const keys = (rows: Row[]) => rows.map((r) => r.key);
const text = (row: Row) => row.chips.map((c) => c.text).join(' | ');
const at = (rows: Row[], key: string) => rows.find((r) => r.key === key)!;

describe('history graph layout', () => {
  it('lays tips over the head, closes by time and every lane event in words', () => {
    const rows = layout(input());
    expect(keys(rows).slice(0, 6)).toEqual(['sec-open', 't-Weekend', 't-Bellona', 't-Kalos', 'sec-history', 'r39']);
    // Scratch closed after #35 and before #36.
    expect(keys(rows).indexOf('e-Scratch')).toBe(keys(rows).indexOf('r35') - 1);
    for (const row of rows)
      for (const lane of row.lanes) {
        if (lane.seg === 'pass') continue;
        // The tip names its draft in the row; every other event in the Drafts column.
        const words = row.kind === 'tip' ? lane.draft.title : text(row);
        expect(words, `${row.key} ${lane.seg}`).toContain(lane.draft.title);
      }
    expect(text(at(rows, 'r38'))).toBe('Bellona starts here');
    expect(text(at(rows, 'r33'))).toBe('Tuesday merged · from #31');
    expect(text(at(rows, 'e-Scratch'))).toBe('Scratch discarded · from #32');
    expect(text(at(rows, 'more'))).toBe('Untitled starts at #12, further down');
    expect(at(rows, 'r33').node).toBe('merge');
    expect(at(rows, 'r39')).toMatchObject({ node: 'head', line: 'bottom' });
    expect(at(rows, 'more').line).toBe('dash');
  });

  it('reuses a lane once its draft has ended, a row between', () => {
    const rows = layout(input());
    const lane = (row: string, id: string) => at(rows, row).lanes.find((l) => l.draft.id === id)?.lane;
    expect([lane('t-Weekend', 'Weekend'), lane('t-Bellona', 'Bellona'), lane('t-Kalos', 'Kalos')]).toEqual([1, 2, 3]);
    // Weekend ends at #36; Tuesday (from #33 down) takes lane 1 again.
    expect(lane('r33', 'Tuesday')).toBe(1);
    expect(laneCount(rows)).toBe(3);
  });

  it('folds quiet stretches of four or more, never the head, and opens a fold on request', () => {
    const rows = layout(input());
    const folds = rows.filter((r) => r.kind === 'fold');
    // #25–#30 (no draft starts, merges or closes in them) and #20–#24 under the expiry.
    expect(folds.map((r) => r.key)).toEqual(['f30-25', 'f24-20']);
    expect(folds.every((r) => r.kind === 'fold' && r.records.length >= FOLD_MIN)).toBe(true);
    // The expired draft's lane runs through the fold below its close.
    expect(at(rows, 'f24-20').lanes.map((l) => [l.draft.id, l.seg])).toEqual([['Untitled', 'pass']]);
    const open = layout(input({ expanded: new Set(['f30-25']) }));
    expect(keys(open)).toContain('r27');
    // A run of three stays rows.
    const short = layout(input({ records: records(39, 37), more: false, drafts: [] }));
    expect(short.filter((r) => r.kind === 'fold')).toEqual([]);
  });

  it('Go to: the newer changes fold above, the tips collapse and the starts are said', () => {
    const rows = layout(input({ records: records(28, 18), tipsCollapsed: true, keep: new Set([18]) }));
    expect(keys(rows).slice(0, 2)).toEqual(['tips', 'newer']);
    const newer = at(rows, 'newer');
    expect(newer.kind === 'newer' && [newer.from, newer.to]).toEqual([29, 39]);
    expect(text(newer)).toBe('3 drafts start in here');
    expect(newer.kind === 'newer' && newer.inside.map((d) => d.id)).toEqual(['Tuesday', 'Scratch']);
    expect(at(rows, 'tips').lanes.map((l) => l.seg)).toEqual(['tip', 'tip', 'tip']);
    expect(newer.lanes.map((l) => l.seg)).toEqual(['base', 'base', 'base']);
    expect(keys(rows)).toContain('r18');
  });

  it('says how a tip stands: base moved until its preview counted conflicts', () => {
    const weekend = BOARD[0]!;
    expect(tipState(weekend, {})).toEqual({ text: 'Base moved', tone: 'warn' });
    expect(tipState(weekend, { Weekend: 2 })).toEqual({ text: '2 conflicts', tone: 'risk' });
    expect(tipState(weekend, { Weekend: 0 }).text).toBe('Base moved · no conflicts');
    expect(tipState({ ...weekend, up_to_date: true }, {}).text).toBe('Up to date');
    expect(tipLine(BOARD[2]!, 39)).toBe('1 staged · weekly timings · on #34 · 5 changes behind the head');
  });
});
