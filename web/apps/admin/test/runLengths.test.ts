import { describe, expect, it } from 'vitest';
import { check, draftOf } from '../src/config/runLengths';

describe('run lengths draft', () => {
  const saved = { default_minutes: 30, overrides: [{ boss: 'BM', difficulty: 'h' as const, minutes: 60 }] };

  it('round-trips the saved settings', () => {
    expect(check(draftOf(saved))).toEqual({ value: saved });
  });

  it('holds the default to 5–240 whole minutes', () => {
    for (const bad of [4, 241, 12.5, null]) {
      expect(check({ ...draftOf(saved), default_minutes: bad })).toMatchObject({ field: 'default', error: expect.stringContaining('5–240') });
    }
  });

  it('needs a boss, a difficulty and 5–480 minutes per override, once per pair', () => {
    const draft = draftOf(saved);
    expect(check({ ...draft, overrides: [{ boss: '', difficulty: '', minutes: 60 }] })).toMatchObject({ field: 0 });
    expect(check({ ...draft, overrides: [{ boss: 'BM', difficulty: 'h', minutes: 481 }] })).toMatchObject({ field: 0, error: expect.stringContaining('5–480') });
    expect(check({ ...draft, overrides: [...draft.overrides, { boss: 'BM', difficulty: 'h', minutes: 90 }] })).toMatchObject({ field: 1 });
  });
});

describe('run lengths against the boss list', () => {
  const catalog = [
    { key: 'BM', name: 'Black Mage', level: 275, hue: 0, portrait: null, difficulties: [{ letter: 'h' as const, name: 'Hard', token: 'HBM', in_use: false }, { letter: 'x' as const, name: 'Extreme', token: 'XBM', in_use: true }] },
  ];

  it('accepts a boss and difficulty from the list', () => {
    expect(check({ default_minutes: 30, overrides: [{ boss: 'BM', difficulty: 'x', minutes: 90 }] }, catalog)).toEqual({
      value: { default_minutes: 30, overrides: [{ boss: 'BM', difficulty: 'x', minutes: 90 }] },
    });
  });

  it('refuses a boss key the list does not have', () => {
    expect(check({ default_minutes: 30, overrides: [{ boss: 'Retired', difficulty: 'h', minutes: 60 }] }, catalog)).toEqual({
      error: 'Override 1: “Retired” is not in the boss list; pick a boss.',
      field: 0,
    });
  });

  it('refuses a difficulty that boss does not have', () => {
    expect(check({ default_minutes: 30, overrides: [{ boss: 'BM', difficulty: 'n', minutes: 60 }] }, catalog)).toEqual({
      error: 'Override 1: Black Mage has no “n” difficulty; pick one it has.',
      field: 0,
    });
  });

  it('leaves the list check to the server until the list has loaded', () => {
    expect(check({ default_minutes: 30, overrides: [{ boss: 'Retired', difficulty: 'h', minutes: 60 }] }, null)).toHaveProperty('value');
  });
});
