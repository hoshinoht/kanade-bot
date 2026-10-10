import { describe, expect, it } from 'vitest';
import {
  allOf,
  countWords,
  edge,
  filterOptions,
  groupsOf,
  hasSearch,
  marked,
  navigate,
  startRow,
  step,
  toggle,
  typeAhead,
  type SelectOption,
} from '../../../packages/ui/src/components/select';

const o = (value: string, label = value, extra: Partial<SelectOption> = {}): SelectOption => ({ value, label, ...extra });
const kinds = [o('', 'every kind'), o('morning'), o('T-1h'), o('T-15m')];
const people = [
  o('', 'everyone'),
  o('admin:1', 'Yuzu', { group: 'Admins', sub: 'as admin' }),
  o('system:delivery', 'system', { group: 'System', sub: 'delivery' }),
  o('member:1', 'Yuzu', { group: 'Members', keywords: 'yuzu_ms' }),
  o('member:2', 'Rin', { group: 'Members', keywords: 'yurin' }),
  o('member:3', 'Yuna', { group: 'Members', disabled: true }),
  o('member:4', 'Ásahi', { group: 'Members' }),
];

describe('keyboard model', () => {
  it('opens on the current value, or the first enabled row', () => {
    expect(startRow(kinds, 'T-1h')).toBe('T-1h');
    expect(startRow(kinds, 'gone')).toBe('');
    expect(startRow(people, 'member:3')).toBe('');
  });

  it('↑/↓ skip disabled rows and stop at the ends', () => {
    expect(step(people, 'member:2', 1)).toBe('member:4');
    expect(step(people, 'member:4', -1)).toBe('member:2');
    expect(step(people, 'member:4', 1)).toBe('member:4');
    expect(step(people, '', -1)).toBe('');
    // From nowhere, ↓ lands on the first row and ↑ on the last.
    expect(step(people, null, 1)).toBe('');
    expect(step(people, null, -1)).toBe('member:4');
    expect(step([o('a', 'a', { disabled: true })], null, 1)).toBeNull();
  });

  it('Home and End reach the first and last enabled rows', () => {
    const list = [o('a', 'a', { disabled: true }), o('b'), o('c'), o('d', 'd', { disabled: true })];
    expect(edge(list, 'first')).toBe('b');
    expect(edge(list, 'last')).toBe('c');
    expect(navigate('Home', list, 'c')).toBe('b');
    expect(navigate('End', list, 'b')).toBe('c');
    expect(navigate('ArrowDown', list, 'b')).toBe('c');
    expect(navigate('ArrowUp', list, 'c')).toBe('b');
    expect(navigate('Enter', list, 'b')).toBeUndefined();
  });
});

describe('type-ahead', () => {
  const runs = [o('', 'every run'), o('bel', 'NBellona'), o('fa', 'HFA'), o('jup', 'HJupiter'), o('car', 'HCarling'), o('lim', 'HLimbo', { disabled: true })];

  it('one letter cycles through the rows it starts', () => {
    expect(typeAhead(runs, '', 'h')).toBe('fa');
    expect(typeAhead(runs, 'fa', 'h')).toBe('jup');
    expect(typeAhead(runs, 'car', 'h')).toBe('fa');
    // A repeated letter keeps cycling.
    expect(typeAhead(runs, 'fa', 'hh')).toBe('jup');
  });

  it('a longer buffer narrows from the active row and skips disabled rows', () => {
    expect(typeAhead(runs, 'fa', 'hc')).toBe('car');
    expect(typeAhead(runs, 'car', 'hca')).toBe('car');
    expect(typeAhead(runs, '', 'hl')).toBeNull();
    expect(typeAhead(runs, '', 'nb')).toBe('bel');
  });

  it('ignores case and accents', () => {
    expect(typeAhead(people, '', 'a')).toBe('member:4');
    expect(typeAhead(people, '', 'R')).toBe('member:2');
  });
});

describe('search filtering', () => {
  it('appears above ten options', () => {
    expect(hasSearch(Array.from({ length: 10 }, (_, i) => o(String(i))))).toBe(false);
    expect(hasSearch(Array.from({ length: 11 }, (_, i) => o(String(i))))).toBe(true);
  });

  it('matches labels, secondary text and aliases, and groups with no match drop out', () => {
    const hits = filterOptions(people, 'yu');
    expect(hits.map((h) => h.value)).toEqual(['admin:1', 'member:1', 'member:2', 'member:3']);
    expect(groupsOf(hits).map((g) => g.label)).toEqual(['Admins', 'Members']);
    expect(filterOptions(people, 'delivery').map((h) => h.value)).toEqual(['system:delivery']);
    expect(filterOptions(people, 'asahi').map((h) => h.value)).toEqual(['member:4']);
    expect(filterOptions(people, 'zzk')).toEqual([]);
    expect(filterOptions(people, '  ')).toHaveLength(people.length);
  });

  it('groups keep their order and ungrouped rows stay outside', () => {
    expect(groupsOf(people).map((g) => [g.label, g.options.length])).toEqual([
      [undefined, 1],
      ['Admins', 1],
      ['System', 1],
      ['Members', 4],
    ]);
  });

  it('marks the first match in a label', () => {
    expect(marked('Yuzu', 'yu')).toEqual(['', 'Yu', 'zu']);
    expect(marked('Rin', 'yu')).toEqual(['Rin', '', '']);
    expect(marked('Rin', '')).toEqual(['Rin', '', '']);
  });
});

describe('multi-select', () => {
  const channels = ['#hcarling-party', '#kalos-crew', '#limbo-trio', '#fa-trio', '#jupiter-crew', '#bm-trio', '#bell-crew', '#announcements'].map((c, i) =>
    o(c, c, { disabled: i === 7 }),
  );

  it('counts "3 of 8", names a single pick, and says none or all', () => {
    expect(countWords(channels, [])).toBe('none');
    expect(countWords(channels, ['#kalos-crew'])).toBe('#kalos-crew');
    expect(countWords(channels, ['#hcarling-party', '#kalos-crew', '#fa-trio'])).toBe('3 of 8');
    expect(countWords(channels, channels.map((c) => c.value))).toBe('all 8');
    // Values no longer offered are not counted.
    expect(countWords(channels, ['#gone', '#fa-trio'])).toBe('#fa-trio');
  });

  it('toggles keep the options order; All picks every enabled row', () => {
    expect(toggle(channels, ['#fa-trio'], '#hcarling-party')).toEqual(['#hcarling-party', '#fa-trio']);
    expect(toggle(channels, ['#hcarling-party', '#fa-trio'], '#hcarling-party')).toEqual(['#fa-trio']);
    expect(allOf(channels)).toHaveLength(7);
    expect(allOf(channels)).not.toContain('#announcements');
  });
});
