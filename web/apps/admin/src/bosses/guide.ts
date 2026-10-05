// Pure rules for the boss guide (knowledge detail): HP breakdown, tabs, tiles.
import type { Difficulty, DifficultyFacts, GuideItem, GuidePhase, KnowledgeDoc, KnowledgeSource, MissionSeries, StrategyLevel } from '@kanade/api-types';

/** Catalog letters; Champion and Destiny are knowledge-only and have none. */
export const LETTER: Record<string, Difficulty> = { Easy: 'e', Normal: 'n', Hard: 'h', Chaos: 'c', Extreme: 'x' };

export const SERIES_NAME: Record<MissionSeries, string> = { 'destiny-weapon': 'Destiny Weapon', 'union-champion': 'Union Champion' };

/** Union Champion stops are ranks (order 1 = B … 5 = SSS); Destiny stops keep their number. */
const RANKS = ['B', 'A', 'S', 'SS', 'SSS'];
export function stopLabel(series: MissionSeries, order: number): string {
  return series === 'union-champion' ? (RANKS[order - 1] ?? String(order)) : String(order);
}

/** The card's place line: "Rank S" for a champion, "3 of 6" (or "Mission 3" with no track) for Destiny. */
export function missionPlace(series: MissionSeries, order: number, stops: number): string {
  if (series === 'union-champion') return `Rank ${stopLabel(series, order)}`;
  return stops ? `${order} of ${stops}` : `Mission ${order}`;
}

export const LEVEL_WORD: Record<StrategyLevel, string> = { low: 'Low', medium: 'Medium', high: 'High' };
export const LEVEL_PIPS: Record<StrategyLevel, number> = { low: 1, medium: 2, high: 3 };

/** A bullet as its bold title (may be empty) and its one line; `detail` is the chatbot's and never shown. */
export function itemParts(item: GuideItem): { title: string; text: string } {
  return typeof item === 'string' ? { title: '', text: item } : { title: item.title, text: item.text };
}

const UNITS = ['m', 'b', 't', 'q'] as const;
const UNIT_WORD: Record<(typeof UNITS)[number], string> = { m: 'million', b: 'billion', t: 'trillion', q: 'quadrillion' };
const HP_VALUE = /^\s*([\d,]*\.?\d+)\s*([mbtq])\s*$/i;

/** `1.407q` → `1.407 quadrillion` (title and screen-reader text); the stored precision, never more digits. */
export function spellHp(value: string): string {
  const found = HP_VALUE.exec(value);
  return found ? `${found[1]} ${UNIT_WORD[found[2]!.toLowerCase() as (typeof UNITS)[number]]}` : value;
}

/**
 * A phase's HP for `count` copies, in the stored short form: the source's
 * decimals kept, and the unit promoted at 1000 (3 × 920t = 2.76q). Count 1,
 * or a value that does not parse, comes back as stored.
 */
export function multiplyHp(value: string, count: number): string {
  const found = HP_VALUE.exec(value);
  if (!found || count <= 1) return value;
  const digits = found[1]!.replace(/,/g, '');
  let places = (digits.split('.')[1] ?? '').length;
  let unit = UNITS.indexOf(found[2]!.toLowerCase() as (typeof UNITS)[number]);
  let amount = Number(digits) * count;
  let promoted = false;
  while (amount >= 1000 && unit < UNITS.length - 1) {
    amount /= 1000;
    unit += 1;
    places += 3;
    promoted = true;
  }
  let text = amount.toFixed(places);
  // Promotion adds places the source never had: 2760t is 2.76q, not 2.760q.
  if (promoted && text.includes('.')) text = text.replace(/\.?0+$/, '');
  return `${text}${UNITS[unit]}`;
}

/** "Phase 1 (Perils)" when the row names its target (base phase before '-'), else "Phase 2-1". */
export function phaseName(phase: string, target?: string): string {
  if (target) return `Phase ${phase.split('-')[0]} (${target})`;
  return /^\d/.test(phase) ? `Phase ${phase}` : phase;
}

/** Adjacent phases that share a `group` (or one ungrouped phase); `cycle` if the run repeats. */
export interface PhaseRun {
  group: string | null;
  cycle: boolean;
  phases: { index: number; phase: GuidePhase }[];
}

export function phaseRuns(phases: GuidePhase[]): PhaseRun[] {
  const runs: PhaseRun[] = [];
  phases.forEach((phase, index) => {
    const last = runs.at(-1);
    if (phase.group && last?.group === phase.group) {
      last.phases.push({ index, phase });
      last.cycle ||= Boolean(phase.cycle);
    } else runs.push({ group: phase.group ?? null, cycle: Boolean(phase.cycle), phases: [{ index, phase }] });
  });
  return runs;
}

/** The run's label above its segments, and the cue's words: "Phase 2 · repeats". */
export function runLabel(run: Pick<PhaseRun, 'group' | 'cycle'>): string {
  return [run.group, run.cycle ? 'repeats' : null].filter(Boolean).join(' · ');
}

/** `?phase=` (1-based) as an index; anything else, or out of range, is the first phase. */
export function phaseIndex(param: string, count: number): number {
  const number = /^\d+$/.test(param) ? Number(param) : 0;
  return number >= 1 && number <= count ? number - 1 : 0;
}

export interface HpPhase {
  name: string;
  /** Value × count, short form, for a phase split between targets; null for one target (its bar shows the value once). */
  total: string | null;
  /** One bar per target, each with the stored value. */
  bars: string[];
}

export interface HpBreakdown {
  /** The `total` row only: rounded rows are never summed. */
  total: string | null;
  phases: HpPhase[];
}

/** HP per phase in data order; null when there are no phase rows (a lone total is a tile). */
export function hpBreakdown(hp: DifficultyFacts['hp']): HpBreakdown | null {
  const rows = (hp ?? []).filter((row) => row.phase !== 'total');
  if (!rows.length) return null;
  return {
    total: hp?.find((row) => row.phase === 'total')?.value ?? null,
    phases: rows.map((row) => {
      const count = Math.max(1, row.count ?? 1);
      return { name: phaseName(row.phase, row.target), total: count > 1 ? multiplyHp(row.value, count) : null, bars: Array.from({ length: count }, () => row.value) };
    }),
  };
}

export const forceLabel = (kind: 'arcane' | 'sacred') => (kind === 'sacred' ? 'Authentic Force' : 'Arcane Force');

export interface Tile {
  label: string;
  value: string;
  sub?: string;
}

export function factTiles(fact: DifficultyFacts): Tile[] {
  const out: Tile[] = [];
  const level = fact.boss_level ?? fact.entry_level;
  if (level) out.push({ label: 'Boss level', value: String(level), sub: fact.entry_level ? `entry ${fact.entry_level}` : undefined });
  if (fact.pdr_percent !== undefined) out.push({ label: 'Defence (PDR)', value: `${fact.pdr_percent}%` });
  if (fact.force) out.push({ label: forceLabel(fact.force.kind), value: String(fact.force.value) });
  if (fact.party_max) out.push({ label: 'Party', value: fact.party_max === 1 ? 'Solo' : `Up to ${fact.party_max}` });
  // A total with no phase rows has no bar to sit over: it is a tile.
  const total = fact.hp?.find((row) => row.phase === 'total');
  if (total && !fact.hp?.some((row) => row.phase !== 'total')) out.push({ label: 'HP (total)', value: total.value });
  const spec = fact.recommended_spec;
  // One tile per party size, each with the shared basis.
  if (spec?.parties?.length) out.push(...spec.parties.map((row) => ({ label: `Recommended · ${row.party}`, value: row.value, sub: spec.basis ?? spec.kind })));
  else if (spec?.value) out.push({ label: 'Recommended', value: spec.value, sub: spec.basis ?? spec.kind });
  return out;
}

/** The selected difficulty's short notes: its letter's difficulty note, its own notes, and a recommendation with no tile figure. */
export function difficultyNotes(doc: KnowledgeDoc, fact: DifficultyFacts): GuideItem[] {
  const letter = LETTER[fact.name];
  const note = letter ? doc.difficulty_notes?.[letter] : undefined;
  const spec = fact.recommended_spec;
  return [
    ...(note ? [note] : []),
    ...(fact.notes ?? []),
    ...(spec && !spec.value && !spec.parties?.length ? [{ title: `Recommended · ${spec.kind}`, text: spec.text }] : []),
  ];
}

export type GuideTab = 'overview' | 'phases' | 'strategies' | 'notes' | 'sources';

/** Pill tabs with counts; a tab with nothing in it is left out (Overview always stays). */
export function guideTabs(doc: KnowledgeDoc): { id: GuideTab; label: string; count: number | null }[] {
  const tabs: { id: GuideTab; label: string; count: number | null }[] = [
    { id: 'overview', label: 'Overview', count: null },
    { id: 'phases', label: 'Phases', count: doc.phases?.length ?? 0 },
    { id: 'strategies', label: 'Strategies', count: doc.strategies?.length ?? 0 },
    { id: 'notes', label: 'Notes', count: doc.notes?.length ?? 0 },
    { id: 'sources', label: 'Sources', count: doc.sources.length },
  ];
  return tabs.filter((tab) => tab.count === null || tab.count > 0);
}

const KINDS: KnowledgeSource['kind'][] = ['official', 'guide', 'wiki', 'tool'];
const KIND_WORD: Record<KnowledgeSource['kind'], string> = { official: 'Official', guide: 'Guide', wiki: 'Wiki', tool: 'Tool' };

export function sourceCounts(sources: KnowledgeSource[]): { kind: string; count: number }[] {
  return KINDS.map((kind) => ({ kind: KIND_WORD[kind], count: sources.filter((source) => source.kind === kind).length })).filter((row) => row.count > 0);
}

/**
 * A band or zone label split so figures stay whole: "250–749 more damage" →
 * [figure "250–749", text " more damage"]. A line may break between words,
 * never inside a figure or at its dash.
 */
export function figureParts(label: string): { text: string; figure: boolean }[] {
  const parts: { text: string; figure: boolean }[] = [];
  const pattern = /[+−-]?\d[\d,.]*(?:\s?[–—-]\s?\d[\d,.]*)?[%+a-zA-Z]*/g;
  let last = 0;
  for (const match of label.matchAll(pattern)) {
    if (match.index > last) parts.push({ text: label.slice(last, match.index), figure: false });
    parts.push({ text: match[0], figure: true });
    last = match.index + match[0].length;
  }
  if (last < label.length) parts.push({ text: label.slice(last), figure: false });
  return parts;
}

export interface TocSection {
  key: string;
  label: string;
  /** Selector of the section inside the guide column. */
  selector: string;
}

/** "On this page" entries for the guide's top sections that are present, in page order. */
export function tocSections(present: { mission: boolean; facts: boolean; hp: boolean; notes: string | null }): TocSection[] {
  return [
    ...(present.mission ? [{ key: 'mission', label: 'Mission', selector: '.guide-mission' }] : []),
    ...(present.facts ? [{ key: 'facts', label: 'Facts', selector: '.guide-tiles' }] : []),
    ...(present.hp ? [{ key: 'hp', label: 'HP', selector: '.guide-hp' }] : []),
    ...(present.notes ? [{ key: 'notes', label: present.notes, selector: '.guide-notes' }] : []),
  ];
}
