import { describe, expect, it } from 'vitest';
import type { PartyTotalBoss, RunPartyTotal } from '@kanade/api-types';
import { asOfTime, hasTotal, totalNotes, type TotalReading } from '../src/characters/partyTotal';

const boss = (token: string, over: Partial<PartyTotalBoss> = {}): PartyTotalBoss => ({
  boss: token,
  combined_percentage: 150.2,
  estimated_minutes: 19.6,
  technically_possible: true,
  complete: true,
  critical_rate_assumed: false,
  no_estimate: 0,
  ...over,
});
const total = (...bosses: PartyTotalBoss[]): RunPartyTotal => ({ as_of: '2026-09-29T03:42:00Z', stale: false, fresh_days: 7, bosses });
const reading = (effective: number | null, assumed = false, actual: number | null = null) => ({
  effective_percentage: effective,
  critical_rate_assumed: assumed,
  critical_rate_actual: actual,
});
const row = (name: string, token: string, ign: string | null, r: TotalReading['reading']): TotalReading => ({ name, token, ign, reading: r });

describe('asOfTime', () => {
  it('names the weekday, day, month and guild time', () => {
    expect(asOfTime('2026-09-29T03:42:00Z', 'Asia/Kuala_Lumpur')).toBe('Tue 29 Sep 11:42');
  });
});

describe('hasTotal', () => {
  it('is false only while no boss has a figure', () => {
    expect(hasTotal(total(boss('HCarling', { combined_percentage: null })))).toBe(false);
    expect(hasTotal(total(boss('HCarling', { combined_percentage: null }), boss('HMaleficStar')))).toBe(true);
  });
});

describe('totalNotes', () => {
  it('names the assumed characters once and the members with no estimate in party order', () => {
    const t = total(boss('HCarling', { critical_rate_assumed: true, no_estimate: 1, complete: false }), boss('HMaleficStar', { no_estimate: 1, complete: false }));
    const rows = [
      row('Asahi', 'HCarling', 'AsahiDK', reading(30)),
      row('Asahi', 'HMaleficStar', 'AsahiMech', reading(10)),
      row('Mika', 'HCarling', 'MikaBish', reading(25, true, 89)),
      row('Mika', 'HMaleficStar', 'MikaBish', reading(20, true, 89)),
      row('Ren (2)', 'HCarling', null, null),
      row('Ren (2)', 'HMaleficStar', null, null),
    ];
    expect(totalNotes(t, rows)).toEqual({ assumed: [{ ign: 'MikaBish', actual: 89 }], none: ['Ren (2)'] });
  });

  it('says nothing for a boss with no figure yet', () => {
    const t = total(boss('HCarling', { combined_percentage: null, no_estimate: 2, critical_rate_assumed: true }));
    expect(totalNotes(t, [row('Asahi', 'HCarling', 'AsahiDK', null)])).toEqual({ assumed: [], none: [] });
  });
});
