/**
 * The History graph (boards B_HistoryGraph*, docs/research/2026-10-12-graph-presence-mockups):
 * the loaded History records, newest first, as table rows with the admin's
 * own drafts as lanes beside them. Pure: the page loads, this lays out.
 *
 * Every lane event drawn in a row (a tip, a start, a merge, a close, a lane
 * running off the loaded pages) is also said in that row's words, so the
 * `aria-hidden` lanes add nothing a reader of the table misses.
 */
import type { ChangeRecord, GraphDraft } from '@kanade/api-types';

/** What a lane draws in one row. */
export type Seg =
  /** The lane passes through. */
  | 'pass'
  /** An open draft's tip: the glyph, the lane going down. */
  | 'tip'
  /** The change the draft started on: the lane leaves the line upwards. */
  | 'base'
  /** The merge record: the lane joins the line from below. */
  | 'merge'
  /** Discarded or expired here: the glyph, the lane going down. */
  | 'end'
  /** Merged or closed inside a folded stretch: the lane stops at it. */
  | 'endIn'
  /** Started further down than the loaded pages: an arrow off the bottom. */
  | 'stub';

/** The History line (lane 0) through a row. */
export type Line = 'none' | 'full' | 'top' | 'bottom' | 'dash';
export type Node = 'rec' | 'head' | 'merge' | 'fold' | null;

export interface Lane {
  draft: GraphDraft;
  /** 1 and up; 0 is the History line. */
  lane: number;
  seg: Seg;
}

export type ChipKind = 'start' | 'merge' | 'discard' | 'expire' | 'stub' | 'starts-in' | 'state';
export interface Chip {
  kind: ChipKind;
  text: string;
  /** The draft it names (opens it), when it names one. */
  draft?: GraphDraft;
  /** `state` chips: how the tip stands. */
  tone?: 'ok' | 'warn' | 'risk';
}

interface Base {
  key: string;
  lanes: Lane[];
  line: Line;
  node: Node;
  chips: Chip[];
}
export type Row =
  | (Base & { kind: 'sec'; label: string })
  | (Base & { kind: 'tip'; draft: GraphDraft })
  | (Base & { kind: 'tips'; drafts: GraphDraft[] })
  | (Base & { kind: 'newer'; from: number; to: number; starting: GraphDraft[]; inside: GraphDraft[] })
  | (Base & { kind: 'rec'; record: ChangeRecord; head: boolean })
  | (Base & { kind: 'ev'; draft: GraphDraft })
  | (Base & { kind: 'fold'; records: ChangeRecord[] })
  | (Base & { kind: 'more'; older: number; canLoad: boolean });

export interface GraphInput {
  /** Contiguous History records, newest first. */
  records: ChangeRecord[];
  head: number;
  drafts: GraphDraft[];
  /** Older pages exist (Older changes). */
  more: boolean;
  /** Conflict counts of the drafts opened so far (their previews), by id. */
  conflicts: Record<string, number>;
  /** Fold keys the admin opened. */
  expanded: ReadonlySet<string>;
  /** Open drafts as one row ("3 open drafts"), when the view is away from the head. */
  tipsCollapsed: boolean;
  /** Records never folded (the selected change, a Go to target). */
  keep: ReadonlySet<number>;
}

/** Runs of this many quiet changes or more fold into one row. */
export const FOLD_MIN = 4;

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "Base moved", "2 conflicts", "Up to date": a tip's state, conflicts only once its preview was read. */
export function tipState(draft: GraphDraft, conflicts: Record<string, number>): { text: string; tone: 'ok' | 'warn' | 'risk' } {
  const known = conflicts[draft.id];
  if (known) return { text: plural(known, 'conflict'), tone: 'risk' };
  if (draft.up_to_date) return { text: 'Up to date', tone: 'ok' };
  return { text: known === 0 ? 'Base moved · no conflicts' : 'Base moved', tone: 'warn' };
}

/** "merged", "discarded", "expired". */
export const closeWord = (draft: { status: string }) => (draft.status === 'merged' ? 'merged' : draft.status === 'expired' ? 'expired' : 'discarded');

/** A tip's second line: "6 staged · on #36 · 3 changes behind the head". */
export function tipLine(draft: GraphDraft, head: number): string {
  const behind = head - draft.base;
  const where = draft.scope === 'weekly' ? 'weekly timings · ' : '';
  return `${draft.staged} staged · ${where}on #${draft.base} · ${behind === 0 ? 'on the head' : `${plural(behind, 'change')} behind the head`}`;
}

type Chain = { kind: 'rec'; record: ChangeRecord } | { kind: 'ev'; draft: GraphDraft };
/** A row before layout fills its lanes and line (distributed over the row kinds). */
type Init<R> = R extends unknown ? Omit<R, 'lanes' | 'line' | 'node' | 'chips'> & Partial<Base> : never;
type RowInit = Init<Row>;

/** The rows, their lanes and their words. */
export function layout(input: GraphInput): Row[] {
  const { records, head, drafts } = input;
  const top = records[0]?.seq ?? head;
  const bottom = records.at(-1)?.seq ?? head;
  const newer = records.length && top < head ? { from: top + 1, to: head } : null;
  const inNewer = (seq: number) => newer !== null && seq >= newer.from && seq <= newer.to;
  const open = drafts.filter((d) => d.status === 'open');
  const closedAt = (d: GraphDraft) => Date.parse(d.closed_at ?? d.updated_at);

  // The chain: records with each discard/expiry placed by time, above the newest record at or before it.
  const events = drafts.filter((d) => d.status === 'discarded' || d.status === 'expired');
  const chain: Chain[] = [];
  const placed = new Set<string>();
  for (const record of records) {
    const at = Date.parse(record.at);
    for (const d of events) {
      if (placed.has(d.id) || closedAt(d) < at || closedInNewer(d)) continue;
      placed.add(d.id);
      chain.push({ kind: 'ev', draft: d });
    }
    chain.push({ kind: 'rec', record });
  }
  function closedInNewer(d: GraphDraft) {
    // Closed after the top loaded record: inside the newer fold (when there is one).
    return newer !== null && records.length > 0 && closedAt(d) > Date.parse(records[0]!.at);
  }

  const starts = new Map<number, GraphDraft[]>();
  const merges = new Map<number, GraphDraft[]>();
  for (const d of drafts) {
    starts.set(d.base, [...(starts.get(d.base) ?? []), d]);
    if (d.status === 'merged' && d.merged_seq !== null) merges.set(d.merged_seq, [...(merges.get(d.merged_seq) ?? []), d]);
  }
  // A change something starts at or merges into, the head and kept changes never fold.
  const loud = new Set<number>([head, ...starts.keys(), ...merges.keys(), ...input.keep]);

  const rows: Row[] = [];
  const row = (r: RowInit): Row => ({ lanes: [], line: 'none', node: null, chips: [], ...r }) as Row;

  if (open.length && input.tipsCollapsed) rows.push(row({ kind: 'tips', key: 'tips', drafts: open }));
  else if (open.length) {
    rows.push(row({ kind: 'sec', key: 'sec-open', label: `Open drafts · ${open.length} · not live` }));
    for (const d of open) {
      const state = tipState(d, input.conflicts);
      rows.push(row({ kind: 'tip', key: `t-${d.id}`, draft: d, chips: [{ kind: 'state', text: state.text, tone: state.tone, draft: d }] }));
    }
    rows.push(row({ kind: 'sec', key: 'sec-history', label: 'History · newest first' }));
  }

  const chainStart = rows.length;
  if (newer) {
    const starting = drafts.filter((d) => inNewer(d.base) && !endsInNewer(d));
    const inside = drafts.filter((d) => inNewer(d.base) && endsInNewer(d));
    const chips: Chip[] = starting.length ? [{ kind: 'starts-in', text: `${plural(starting.length, 'draft')} start${starting.length === 1 ? 's' : ''} in here` }] : [];
    for (const d of drafts.filter((d) => !inNewer(d.base) && endsInNewer(d))) chips.push({ kind: d.status === 'merged' ? 'merge' : d.status === 'expired' ? 'expire' : 'discard', text: `${d.title} ${closeWord(d)} in here`, draft: d });
    rows.push(row({ kind: 'newer', key: 'newer', from: newer.from, to: newer.to, starting, inside, node: 'fold', chips }));
  }
  function endsInNewer(d: GraphDraft) {
    if (d.status === 'open') return false;
    if (d.status === 'merged') return d.merged_seq !== null && inNewer(d.merged_seq);
    return closedInNewer(d);
  }

  // Quiet stretches fold: FOLD_MIN or more records in a row with nothing starting, merging or closing.
  let run: ChangeRecord[] = [];
  const flush = () => {
    const key = run.length ? `f${run[0]!.seq}-${run.at(-1)!.seq}` : '';
    if (run.length >= FOLD_MIN && !input.expanded.has(key)) rows.push(row({ kind: 'fold', key, records: run, node: 'fold' }));
    else for (const record of run) rows.push(recRow(record));
    run = [];
  };
  function recRow(record: ChangeRecord): Row {
    const chips: Chip[] = [];
    for (const d of merges.get(record.seq) ?? []) chips.push({ kind: 'merge', text: `${d.title} merged · from #${d.base}`, draft: d });
    for (const d of starts.get(record.seq) ?? []) if (!inNewer(d.base)) chips.push({ kind: 'start', text: `${d.title} starts here`, draft: d });
    const merge = merges.has(record.seq) || record.surface === 'draft_merge' || record.surface === 'request_merge';
    return row({ kind: 'rec', key: `r${record.seq}`, record, head: record.seq === head, node: record.seq === head ? 'head' : merge ? 'merge' : 'rec', chips });
  }
  for (const item of chain) {
    if (item.kind === 'ev') {
      flush();
      const d = item.draft;
      rows.push(row({ kind: 'ev', key: `e-${d.id}`, draft: d, chips: [{ kind: d.status === 'expired' ? 'expire' : 'discard', text: `${d.title} ${closeWord(d)} · from #${d.base}`, draft: d }] }));
    } else if (loud.has(item.record.seq)) {
      flush();
      rows.push(recRow(item.record));
    } else run.push(item.record);
  }
  flush();

  // Lanes whose start is older than the loaded pages run off the bottom.
  const below = drafts.filter((d) => records.length > 0 && d.base < bottom);
  if (input.more || below.length) {
    rows.push(
      row({
        kind: 'more',
        key: 'more',
        older: Math.max(0, bottom - 1),
        canLoad: input.more,
        chips: below.map((d) => ({ kind: 'stub' as const, text: `${d.title} starts at #${d.base}, further down`, draft: d })),
      }),
    );
  }

  // Each draft's first and last row, then a lane for it.
  const indexOf = (key: string) => rows.findIndex((r) => r.key === key);
  const spans: { draft: GraphDraft; top: number; topSeg: Seg; bottom: number; bottomSeg: Seg }[] = [];
  for (const d of drafts) {
    let topAt = -1;
    let topSeg: Seg = 'pass';
    if (d.status === 'open') [topAt, topSeg] = [input.tipsCollapsed ? indexOf('tips') : indexOf(`t-${d.id}`), 'tip'];
    else if (endsInNewer(d)) [topAt, topSeg] = [indexOf('newer'), 'endIn'];
    else if (d.status === 'merged') [topAt, topSeg] = [indexOf(`r${d.merged_seq}`), 'merge'];
    else [topAt, topSeg] = [indexOf(`e-${d.id}`), 'end'];
    let bottomAt = -1;
    let bottomSeg: Seg = 'base';
    if (inNewer(d.base)) bottomAt = indexOf('newer');
    else if (d.base < bottom && records.length) [bottomAt, bottomSeg] = [indexOf('more'), 'stub'];
    else bottomAt = indexOf(`r${d.base}`);
    // Begun and ended inside the newer fold: said in its words, not drawn.
    if (topAt < 0 || bottomAt < 0 || topAt >= bottomAt) continue;
    spans.push({ draft: d, top: topAt, topSeg, bottom: bottomAt, bottomSeg });
  }
  spans.sort((a, b) => a.top - b.top);
  // A lane is reused once its draft has ended, with at least one row between.
  const lastBottom: number[] = [];
  for (const span of spans) {
    let lane = 1;
    while (lastBottom[lane] !== undefined && lastBottom[lane]! >= span.top - 1) lane++;
    lastBottom[lane] = span.bottom;
    for (let i = span.top; i <= span.bottom; i++) {
      const seg = i === span.top ? span.topSeg : i === span.bottom ? span.bottomSeg : 'pass';
      rows[i]!.lanes.push({ draft: span.draft, lane, seg });
    }
  }

  // The History line from the first chain row down to the last (dashed into Older changes).
  const last = rows.length - 1;
  const chainEnd = rows[last]?.kind === 'more' ? last - 1 : last;
  for (let i = chainStart; i <= last; i++) {
    const r = rows[i]!;
    if (r.kind === 'more') r.line = 'dash';
    else if (i === chainStart && i === chainEnd) r.line = 'none';
    else if (i === chainStart) r.line = 'bottom';
    else if (i === chainEnd && rows[last]?.kind !== 'more') r.line = 'top';
    else r.line = 'full';
  }
  return rows;
}

/** The number of lanes the rows use (for the lane column's width). */
export const laneCount = (rows: Row[]) => Math.max(0, ...rows.flatMap((r) => r.lanes.map((l) => l.lane)));

/** Every row the keyboard can reach (sections are labels only). */
export const focusable = (rows: Row[]) => rows.filter((r) => r.kind !== 'sec');

/** Drafts drawn as starting at, merging into or conflicting with a change (the pane's "Drafts at this change"). */
export function draftsAt(seq: number, drafts: GraphDraft[]): { draft: GraphDraft; what: 'starts' | 'merged' }[] {
  return [
    ...drafts.filter((d) => d.base === seq).map((draft) => ({ draft, what: 'starts' as const })),
    ...drafts.filter((d) => d.status === 'merged' && d.merged_seq === seq).map((draft) => ({ draft, what: 'merged' as const })),
  ];
}

/** Where a draft opens in Drafts. */
export const draftHref = (draft: { id: string; status: string }) =>
  `/drafts?${draft.status === 'open' ? '' : 'tab=closed&'}item=${encodeURIComponent(draft.id)}`;
