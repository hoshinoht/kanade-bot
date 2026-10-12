import { describe, expect, it } from 'vitest';
import type { CharacterDataHealth, CharacterLink, CharacterRow, EstimateError } from '@kanade/api-types';
import { asOfDate, catalogBoss, clock, combatPower, dayTime, estimateErrorText, healthNote, joinNames, minutes, percent, stripState } from '../src/chardata/format';

// Wholly invented data, as the mock seeds it.
const row = (retired: boolean): CharacterRow => ({
  id: 'ch-s3',
  member_id: '1003',
  ign: 'MikaBish',
  world: 'msea',
  class: 'Bishop',
  level: 281,
  role: 'alt',
  retired_at: retired ? '2026-08-14T04:00:00Z' : null,
  created_at: '2026-06-01T04:00:00Z',
  updated_at: '2026-09-27T04:00:00Z',
  updated_by: { kind: 'admin', id: 'discord:1001' },
  version: 1,
});

const link = (state: CharacterLink['state']): CharacterLink => ({
  character_id: 'ch-s3',
  state,
  version: 2,
  connected: true,
  requested_by: null,
  requested_at: null,
  consent_version: 1,
  consented_at: null,
  linked_at: null,
  notice_version: 1,
  data: null,
  refresh: { next_attempt_at: null, last_error: null },
  attribution: 'Data based on NEXON Open API',
  privacy_url: null,
});

const health = (over: Partial<CharacterDataHealth>): CharacterDataHealth => ({
  nexon: 'ok',
  backoff_until: null,
  calls_24h: 162,
  budget: 800,
  last_batch: null,
  links: { pending: 1, active: 4, stale: 1 },
  last_sweep: null,
  posts: { due: 0, unknown: 0, stuck: 0, overdue: 0, items: [] },
  scouter: { state: 'ok', url: 'http://scouter:8091', last_lookup_at: null, last_error: null, last_error_at: null },
  ...over,
});

describe('the character data strip', () => {
  it('reads a retired linked character as paused, every other state as the API says', () => {
    expect(stripState(link('linked'), row(true))).toBe('paused');
    expect(stripState(link('linked'), row(false))).toBe('linked');
    expect(stripState(link('expired'), row(true))).toBe('expired');
  });

  it('writes combat power in millions with one decimal, and NEXON dates as given', () => {
    expect(combatPower(48_204_551)).toBe('48.2M');
    expect(combatPower(1_234_560_000)).toBe('1,234.6M');
    expect(asOfDate('2026-09-29')).toBe('29 Sep 2026');
    expect(dayTime('2026-09-28T20:01:00Z', 'Asia/Singapore')).toBe('29 Sep 04:01');
  });
});

describe('MapleScouter estimates', () => {
  it('formats shares, minutes and the elapsed clock', () => {
    expect(percent(72.4)).toBe('72.4%');
    expect(percent(null)).toBe('—');
    expect(minutes(41.44)).toBe('41 min');
    expect(minutes(26.79)).toBe('26.8 min');
    expect(clock(72)).toBe('1:12');
    expect(joinNames(['Hotaru', 'Ren (2)'])).toBe('Hotaru and Ren (2)');
  });

  it('names catalog bosses by token and drops the rest', () => {
    const catalog = [{ key: 'Limbo', name: 'Limbo', level: 285, hue: 0, portrait: null, difficulties: [{ letter: 'h' as const, name: 'Hard', token: 'HLimbo', in_use: true }] }];
    expect(catalogBoss(catalog, 'HLimbo')).toEqual({ name: 'Limbo', difficulty: 'h' });
    expect(catalogBoss(catalog, 'XUnknown')).toBeNull();
  });

  it('says each error the boards draw, and never a partial party total', () => {
    const error = (code: EstimateError['code'], status: number | null): EstimateError => ({ code, message: '', status, detail: '', retry_after_seconds: 40, character_id: null });
    expect(estimateErrorText(error('critical_rate', 422), 'YuzuPala', true).text).toContain('No party total is made from a partial lookup.');
    expect(estimateErrorText(error('manual_check', 503), null, false).lead).toBe('Scouter needs a manual check.');
    expect(estimateErrorText(error('failed', 504), null, false).text).toBe('MapleScouter didn’t answer within 2 minutes (504). Nothing was kept.');
    expect(estimateErrorText(error('busy', 429), null, false).text).toContain('Try again in 40 s.');
  });
});

describe('Config › Character data', () => {
  it('flags the cards that need someone, and names a missing key', () => {
    expect(healthNote(health({}))).toEqual({ text: 'ok', flagged: 0 });
    expect(healthNote(health({ nexon: 'no_key', scouter: { state: 'off', url: null, last_lookup_at: null, last_error: null, last_error_at: null } }))).toEqual({ text: 'no key', flagged: 0 });
    expect(healthNote(health({ nexon: 'key_refused', scouter: { state: 'unreachable', url: 'http://scouter:8091', last_lookup_at: null, last_error: 'unreachable', last_error_at: null } })).flagged).toBe(2);
    const stuck = { due: 0, unknown: 1, stuck: 1, overdue: 0, items: [] };
    expect(healthNote(health({ nexon: 'backoff', posts: stuck })).flagged).toBe(1);
  });
});
