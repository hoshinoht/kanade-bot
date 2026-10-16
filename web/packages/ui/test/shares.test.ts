import { describe, expect, it } from 'vitest';
import { shareBar, shareLabel, whoColour, wholeShares, type ShareMember } from '../src/characters/shares';

const member = (n: number | null, name: string, effective: Record<string, number | null>, assumed = false): ShareMember => ({
  id: name.toLowerCase(),
  name,
  igns: {},
  n,
  effective,
  assumed: Object.fromEntries(Object.keys(effective).map((token) => [token, assumed])),
});

// The approved Kalos board's numbers (the user's screenshot, members renamed).
const KALOS = [
  member(1, 'Asahi', { XKalos: 38.8 }),
  member(2, 'Ren', { XKalos: 35.8 }, true),
  member(3, 'Mika', { XKalos: 25.3 }),
  member(4, 'Tsubame', { XKalos: 23.7 }, true),
  member(5, 'Sora', { XKalos: 16.8 }),
];

describe('wholeShares', () => {
  it('adds up to exactly 100 by largest remainder', () => {
    expect(wholeShares([38.8, 35.8, 25.3, 23.7, 16.8])).toEqual([28, 25, 18, 17, 12]);
    expect(wholeShares([1, 1, 1])).toEqual([34, 33, 33]);
    for (const values of [[1, 2, 3, 4, 5, 6], [0.1, 99.9], [33.3, 33.3, 33.4], [7]]) expect(wholeShares(values).reduce((a, b) => a + b, 0)).toBe(100);
  });

  it('has no shares of nothing', () => {
    expect(wholeShares([])).toEqual([]);
    expect(wholeShares([0, 0])).toEqual([0, 0]);
  });
});

describe('shareBar', () => {
  it('splits the party total by effective %, in party order', () => {
    const bar = shareBar('XKalos', KALOS);
    expect(bar.parts.map((p) => [p.n, p.share])).toEqual([
      [1, 28],
      [2, 25],
      [3, 18],
      [4, 17],
      [5, 12],
    ]);
    expect(bar.total).toBeCloseTo(140.4);
    expect(bar.complete).toBe(true);
    expect(bar.missing).toEqual([]);
  });

  it('flags assumed critical rates on their parts', () => {
    const bar = shareBar('XKalos', KALOS);
    expect(bar.parts.filter((p) => p.assumed).map((p) => p.name)).toEqual(['Ren', 'Tsubame']);
    expect(shareLabel('XKalos', bar)).toBe('XKalos shares: Asahi 28%, Ren 25% (assumes 100% critical rate), Mika 18%, Tsubame 17% (assumes 100% critical rate), Sora 12%');
  });

  it('leaves members without an estimate out of the total, never as 0%', () => {
    const party = [
      member(1, 'Asahi', { HCarling: 34, HMaleficStar: 41 }),
      // A failed lookup keeps the number; no estimate for either boss.
      member(2, 'Ren', {}),
      // No card for one boss, an unknown share (null) for the other.
      member(3, 'Mika', { HMaleficStar: null }),
      member(4, 'Yuzu', { HCarling: 20.5, HMaleficStar: 24 }),
      // No character: never numbered, even with a stray value.
      member(null, 'Hotaru', { HCarling: 50 }),
    ];
    const carling = shareBar('HCarling', party);
    expect(carling.parts.map((p) => [p.name, p.n, p.colour])).toEqual([
      ['Asahi', 1, 1],
      ['Yuzu', 4, 4],
    ]);
    expect(carling.parts.reduce((sum, p) => sum + p.share, 0)).toBe(100);
    expect(carling.total).toBe(54.5);
    expect(carling.complete).toBe(false);
    expect(carling.missing.map((m) => m.name)).toEqual(['Ren', 'Mika', 'Hotaru']);
    expect(shareLabel('HCarling', carling)).toBe('HCarling shares: Asahi 62%, Yuzu 38%; 3 with no estimate');
    expect(carling.parts.every((p) => p.share > 0)).toBe(true);
  });

  it('draws no parts for a boss nobody has an estimate for', () => {
    const bar = shareBar('XKalos', [member(1, 'Asahi', {}), member(null, 'Ren', {})]);
    expect(bar.parts).toEqual([]);
    expect(bar.total).toBeNull();
    expect(bar.missing).toHaveLength(2);
  });

  it('never counts a non-number', () => {
    const bar = shareBar('XKalos', [member(1, 'Asahi', { XKalos: Number.NaN }), member(2, 'Ren', { XKalos: 10 })]);
    expect(bar.parts.map((p) => [p.name, p.share])).toEqual([['Ren', 100]]);
  });
});

describe('per boss', () => {
  it('keeps a member’s number on every bar and names the character each boss brings', () => {
    const asahi: ShareMember = {
      id: 'asahi',
      name: 'Asahi',
      igns: { HCarling: 'AsahiDK', HMaleficStar: 'AsahiMech' },
      n: 1,
      effective: { HCarling: 30, HMaleficStar: 10 },
      assumed: { HMaleficStar: true },
    };
    const party = [asahi, member(2, 'Ren', { HCarling: 30, HMaleficStar: 30 })];
    const carling = shareBar('HCarling', party);
    const star = shareBar('HMaleficStar', party);
    expect(carling.parts.map((p) => [p.n, p.ign, p.assumed])).toEqual([
      [1, 'AsahiDK', false],
      [2, null, false],
    ]);
    expect(star.parts.map((p) => [p.n, p.ign, p.assumed])).toEqual([
      [1, 'AsahiMech', true],
      [2, null, false],
    ]);
    expect(shareLabel('HMaleficStar', star)).toBe('HMaleficStar shares: Asahi (AsahiMech) 25% (assumes 100% critical rate), Ren 75%');
  });
});

describe('whoColour', () => {
  it('follows the number, six colours', () => {
    expect([1, 2, 6, 7, 12].map(whoColour)).toEqual([1, 2, 6, 1, 6]);
  });
});
