import { describe, expect, it } from 'vitest';
import type { KnowledgeDoc } from '@kanade/api-types';
import { difficultyNotes, factTiles, figureParts, guideTabs, hpBreakdown, itemParts, missionPlace, multiplyHp, phaseIndex, phaseName, phaseRuns, runLabel, spellHp, stopLabel, tocSections } from '../src/bosses/guide';

describe('boss guide HP', () => {
  it('names a phase by its base and target, or keeps sequential phases as they are', () => {
    expect(phaseName('1', 'Perils')).toBe('Phase 1 (Perils)');
    expect(phaseName('3-1', 'Perils')).toBe('Phase 3 (Perils)');
    expect(phaseName('3-2', 'Carling')).toBe('Phase 3 (Carling)');
    expect(phaseName('2')).toBe('Phase 2');
    expect(phaseName('2-1')).toBe('Phase 2-1');
    expect(phaseName('Core')).toBe('Core');
  });

  it('multiplies in the stored unit and precision, promoting at 1000', () => {
    expect(multiplyHp('1.407q', 3)).toBe('4.221q');
    expect(multiplyHp('96.6t', 3)).toBe('289.8t');
    expect(multiplyHp('920t', 3)).toBe('2.76q');
    expect(multiplyHp('399t', 3)).toBe('1.197q');
    expect(multiplyHp('6.018q', 3)).toBe('18.054q');
    expect(multiplyHp('500b', 2)).toBe('1t');
    expect(multiplyHp('3,000b', 2)).toBe('6t');
    expect(multiplyHp('1.404q', 1)).toBe('1.404q');
    expect(multiplyHp('unknown', 3)).toBe('unknown');
  });

  it('spells a short value out without adding precision', () => {
    expect(spellHp('1.407q')).toBe('1.407 quadrillion');
    expect(spellHp('93.2t')).toBe('93.2 trillion');
    expect(spellHp('500b')).toBe('500 billion');
    expect(spellHp('12M')).toBe('12 million');
    expect(spellHp('about 5t')).toBe('about 5t');
  });

  it('lists phases in data order with one bar per target, a phase total only when split, and only a stored total', () => {
    const hp = hpBreakdown([
      { phase: '1', value: '920t', count: 3, target: 'Perils' },
      { phase: '2', value: '1.404q' },
      { phase: '3-1', value: '1.407q', count: 3, target: 'Perils' },
      { phase: '3-2', value: '1.883q', target: 'Carling' },
      { phase: 'total', value: '10.27q' },
    ])!;
    expect(hp.total).toBe('10.27q');
    expect(hp.phases).toEqual([
      { name: 'Phase 1 (Perils)', total: '2.76q', bars: ['920t', '920t', '920t'] },
      { name: 'Phase 2', total: null, bars: ['1.404q'] },
      { name: 'Phase 3 (Perils)', total: '4.221q', bars: ['1.407q', '1.407q', '1.407q'] },
      { name: 'Phase 3 (Carling)', total: null, bars: ['1.883q'] },
    ]);
    // No total row: no total line (rounded rows are never summed).
    expect(hpBreakdown([{ phase: '2-1', value: '972.5t' }, { phase: '2-2', value: '972.5t' }])).toEqual({
      total: null,
      phases: [
        { name: 'Phase 2-1', total: null, bars: ['972.5t'] },
        { name: 'Phase 2-2', total: null, bars: ['972.5t'] },
      ],
    });
    expect(hpBreakdown([{ phase: 'total', value: '4t' }])).toBeNull();
    expect(hpBreakdown(undefined)).toBeNull();
  });
});

describe('boss guide content', () => {
  const doc: KnowledgeDoc = {
    boss: 'Example',
    summary: 'Invented summary.',
    core: ['Old core line.'],
    danger: [{ title: 'Floor', text: 'It hurts.', detail: 'Long bot-only wording.' }],
    tips: [],
    difficulty_notes: { h: 'Hard only.' },
    sources: [{ url: 'https://example.invalid', title: 'A', author: 'B', kind: 'guide', fetched: '2026-10-05' }],
  };

  it('never surfaces the chatbot detail', () => {
    expect(itemParts(doc.danger[0]!)).toEqual({ title: 'Floor', text: 'It hurts.' });
    expect(itemParts('plain')).toEqual({ title: '', text: 'plain' });
  });

  it('hides empty tabs and counts the rest; Overview has no count', () => {
    expect(guideTabs(doc)).toEqual([
      { id: 'overview', label: 'Overview', count: null },
      { id: 'sources', label: 'Sources', count: 1 },
    ]);
  });

  it('builds tiles and notes from a difficulty', () => {
    const fact = { name: 'Hard' as const, entry_level: 275, boss_level: 285, pdr_percent: 380, party_max: 1, force: { kind: 'sacred' as const, value: 350 }, recommended_spec: { kind: 'HEXA stat', text: 'Long text.' } };
    expect(factTiles(fact).map((tile) => [tile.label, tile.value, tile.sub])).toEqual([
      ['Boss level', '285', 'entry 275'],
      ['Defence (PDR)', '380%', undefined],
      ['Authentic Force', '350', undefined],
      ['Party', 'Solo', undefined],
    ]);
    expect(difficultyNotes(doc, fact)).toEqual(['Hard only.', { title: 'Recommended · HEXA stat', text: 'Long text.' }]);
    const tiled = { ...fact, recommended_spec: { ...fact.recommended_spec, value: '≈ 86k', basis: 'KMS, Oct 2025' } };
    expect(factTiles(tiled).at(-1)).toEqual({ label: 'Recommended', value: '≈ 86k', sub: 'KMS, Oct 2025' });
    expect(difficultyNotes(doc, tiled)).toEqual(['Hard only.']);
    // A lone total is a tile; with phase rows it belongs to the HP bar.
    expect(factTiles({ name: 'Hard', hp: [{ phase: 'total', value: '3.288q' }] })).toEqual([{ label: 'HP (total)', value: '3.288q' }]);
    expect(factTiles({ name: 'Hard', hp: [{ phase: '1', value: '1q' }, { phase: 'total', value: '1q' }] })).toEqual([]);
  });
});

describe('mission track labels', () => {
  it('names Union Champion stops by rank letter and keeps Destiny numbers', () => {
    expect([1, 2, 3, 4, 5].map((order) => stopLabel('union-champion', order))).toEqual(['B', 'A', 'S', 'SS', 'SSS']);
    expect(stopLabel('union-champion', 9)).toBe('9');
    expect([1, 6].map((order) => stopLabel('destiny-weapon', order))).toEqual(['1', '6']);
  });

  it('places a champion by rank and a Destiny stop in its series', () => {
    expect(missionPlace('union-champion', 3, 4)).toBe('Rank S');
    expect(missionPlace('union-champion', 3, 0)).toBe('Rank S');
    expect(missionPlace('destiny-weapon', 3, 6)).toBe('3 of 6');
    expect(missionPlace('destiny-weapon', 3, 0)).toBe('Mission 3');
  });
});

describe('phase timeline', () => {
  const phase = (name: string, extra: object = {}) => ({ name, items: ['x'], ...extra });

  it('brackets adjacent phases of a group, cycling if any of them cycles', () => {
    const runs = phaseRuns([
      phase('Phase 1'),
      phase('Noon', { group: 'Phase 2', cycle: true }),
      phase('Sunset', { group: 'Phase 2', cycle: true }),
      phase('Midnight', { group: 'Phase 2', cycle: true }),
      phase('Dawn', { group: 'Phase 2', cycle: true }),
      phase('Phase 3'),
      phase('Phase 4'),
    ]);
    expect(runs.map((run) => [run.group, run.cycle, run.phases.map((entry) => entry.index)])).toEqual([
      [null, false, [0]],
      ['Phase 2', true, [1, 2, 3, 4]],
      [null, false, [5]],
      [null, false, [6]],
    ]);
  });

  it('starts a new run when the same group comes back after a gap', () => {
    const runs = phaseRuns([phase('A', { group: 'G' }), phase('B'), phase('C', { group: 'G' })]);
    expect(runs.map((run) => run.phases.length)).toEqual([1, 1, 1]);
  });

  it('says the loop cue in words', () => {
    expect(runLabel({ group: 'Phase 2', cycle: true })).toBe('Phase 2 · repeats');
    expect(runLabel({ group: 'Rooms', cycle: false })).toBe('Rooms');
    expect(runLabel({ group: null, cycle: true })).toBe('repeats');
    expect(runLabel({ group: null, cycle: false })).toBe('');
  });

  it('reads ?phase= as a 1-based index, defaulting to the first', () => {
    expect(phaseIndex('3', 5)).toBe(2);
    expect(phaseIndex('', 5)).toBe(0);
    expect(phaseIndex('9', 5)).toBe(0);
    expect(phaseIndex('0', 5)).toBe(0);
    expect(phaseIndex('two', 5)).toBe(0);
  });
});

describe('guide additions (2026-10-05 round 2)', () => {
  it('shows one recommendation row per party size, the basis underneath; value as the fallback', () => {
    const spec = { kind: 'HEXA stat', text: 'Long.', value: '≈ 99k', basis: 'KMS, 2026', parties: [{ party: 'Solo', value: '≈ 113k' }, { party: '6 players', value: '≈ 48k' }] };
    const tile = factTiles({ name: 'Hard', recommended_spec: spec }).at(-1)!;
    expect(tile).toEqual({ label: 'Recommended', value: '', sub: 'KMS, 2026', rows: [{ label: 'Solo', value: '≈ 113k' }, { label: '6 players', value: '≈ 48k' }] });
    expect(factTiles({ name: 'Hard', recommended_spec: { ...spec, parties: [] } }).at(-1)).toEqual({ label: 'Recommended', value: '≈ 99k', sub: 'KMS, 2026' });
    // Parties without a value still count as a figure: no long-text note.
    const noValue = { kind: spec.kind, text: spec.text, parties: spec.parties };
    expect(difficultyNotes({ boss: 'x', summary: '', danger: [], tips: [], sources: [] }, { name: 'Hard', recommended_spec: noValue })).toEqual([]);
  });

  it('keeps figures whole in band labels', () => {
    expect(figureParts('1000 locked in')).toEqual([{ text: '1000', figure: true }, { text: ' locked in', figure: false }]);
    expect(figureParts('250–749 more damage')).toEqual([{ text: '250–749', figure: true }, { text: ' more damage', figure: false }]);
    expect(figureParts('+30% off-band').map((part) => part.text)).toEqual(['+30%', ' off-band']);
    expect(figureParts('safe')).toEqual([{ text: 'safe', figure: false }]);
  });

  it('lists the page sections present, in page order', () => {
    expect(tocSections({ mission: true, facts: true, hp: true, notes: 'Hard notes' }).map((section) => section.label)).toEqual(['Mission', 'Facts', 'HP', 'Hard notes']);
    expect(tocSections({ mission: false, facts: true, hp: false, notes: null }).map((section) => section.key)).toEqual(['facts']);
  });
});
