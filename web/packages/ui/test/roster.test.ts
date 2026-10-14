import { describe, expect, it } from 'vitest';
import type { CharacterRow } from '@kanade/api-types';
import { checkForm, classOptions, dayMonth, IGN_ERROR, invalidField, LEVEL_ERROR, mainLocked, regionLabel, splitCharacters, takenKey } from '../src/characters/roster';

// Invented characters, as the mock seeds them.
const row = (id: string, ign: string, role: 'main' | 'alt', retired = false): CharacterRow => ({
  id,
  member_id: '1003',
  ign,
  world: 'msea',
  class: 'Bishop',
  level: 281,
  role,
  retired_at: retired ? '2026-08-14T04:00:00Z' : null,
  created_at: '2026-06-01T04:00:00Z',
  updated_at: '2026-09-27T04:00:00Z',
  updated_by: { kind: 'admin', id: 'discord:1001' },
  version: 1,
});

const hero = row('h', 'MikaHero', 'alt', true);
const nl = row('n', 'MikaNL', 'main');
const bish = row('b', 'MikaBish', 'alt');
const form = { ign: 'MikaKain', world: 'msea', class: 'Kain', level: '250' };

describe('the Characters list', () => {
  it('puts the main first and the retired ones in their own group', () => {
    const { active, retired } = splitCharacters([hero, bish, nl]);
    expect(active.map((c) => c.ign)).toEqual(['MikaNL', 'MikaBish']);
    expect(retired.map((c) => c.ign)).toEqual(['MikaHero']);
  });

  it('keeps the main while other characters are active', () => {
    expect(mainLocked(nl, [hero, nl, bish])).toBe(true);
    expect(mainLocked(nl, [hero, nl]), 'only the retired one besides it').toBe(false);
    expect(mainLocked(bish, [nl, bish])).toBe(false);
  });

  it('dates in the guild time zone and names regions', () => {
    expect(dayMonth('2026-09-26T17:00:00Z', 'Asia/Kuala_Lumpur')).toBe('27 Sep');
    expect(regionLabel('msea')).toBe('MapleSEA');
    expect(regionLabel('elysium')).toBe('elysium');
  });

  it('keeps a stored class the list does not name', () => {
    expect(classOptions('Mystery')[0]).toEqual({ value: 'Mystery', label: 'Mystery' });
    expect(classOptions('Bishop').filter((o) => o.value === 'Bishop')).toHaveLength(1);
  });
});

describe('the Add / Edit form', () => {
  it('accepts a whole form and an empty level', () => {
    expect(checkForm(form, new Set())).toEqual({});
    expect(checkForm({ ...form, level: '' }, new Set())).toEqual({});
  });

  it('checks the IGN, the class and the level range before asking', () => {
    expect(checkForm({ ign: 'ab', world: 'msea', class: '', level: '312' }, new Set())).toEqual({ ign: IGN_ERROR, class: 'Pick a class.', level: LEVEL_ERROR });
    expect(checkForm({ ...form, level: '0' }, new Set()).level).toBe(LEVEL_ERROR);
    expect(checkForm({ ...form, level: '2x' }, new Set()).level).toBe(LEVEL_ERROR);
  });

  it('asks no class or level when NEXON is connected for a MapleSEA add', () => {
    expect(checkForm({ ign: 'Wisteria', world: 'msea', class: '', level: '' }, new Set(), false)).toEqual({});
    expect(checkForm({ ign: 'ab', world: 'msea', class: '', level: '999' }, new Set(), false)).toEqual({ ign: IGN_ERROR });
  });

  it('remembers an IGN the server refused in that region, whatever the case', () => {
    const taken = new Set([takenKey('hotaru7', 'msea')]);
    expect(checkForm({ ...form, ign: 'Hotaru7' }, taken).ign).toContain('already on the roster in MapleSEA');
    expect(checkForm({ ...form, ign: 'Hotaru7', world: 'gms' }, taken)).toEqual({});
  });

  it('maps the server’s field refusals to their field', () => {
    expect(invalidField('A level is 1-300.')).toEqual({ field: 'level', text: LEVEL_ERROR });
    expect(invalidField('An IGN is 4-12 letters or digits.')?.field).toBe('ign');
    expect(invalidField('Something else.')).toBeNull();
  });
});
