import { describe, expect, it } from 'vitest';
import type {
  BossReadiness,
  CharacterDataHealth,
  CharacterLink,
  CharacterLookup,
  CharacterRow,
  EstimateError,
  ReadinessSlot,
  ScouterReading,
} from '@kanade/api-types';
import {
  asOfDate,
  assumedCrit,
  catalogBoss,
  clock,
  combatPower,
  dayTime,
  estimateErrorText,
  lookupLine,
  lookupMiss,
  minutes,
  percent,
  queuedText,
  scouterChip,
  stripState,
} from '@kanade/ui';
import { createdLinkNote, healthNote, joinNames, levelChip, readinessLine, scouterAt } from '../src/chardata/format';

// Wholly invented data, as the e2e seed has it.
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

describe('run readiness', () => {
  const boss = (level: BossReadiness['level'], scouter: ScouterReading | null = null): BossReadiness => ({ boss: 'Carling', difficulty: 'h', entry_level: 275, level, scouter });
  const reading = (over: Partial<ScouterReading> = {}): ScouterReading => ({
    percentage: 164.2,
    rating: '2p Min Cut',
    estimated_minutes: 18.3,
    fetched_at: '2026-09-29T02:10:00Z',
    critical_rate_assumed: false,
    critical_rate_actual: null,
    ...over,
  });
  const slot = (over: Partial<ReadinessSlot>): ReadinessSlot => ({
    member_id: '1004',
    character: null,
    basis: null,
    candidates: 0,
    link: 'linked',
    level: { value: 276, source: 'nexon', as_of: '2026-09-29' },
    power: { kind: 'nexon_cp', value: 52_104_000, as_of: '2026-09-29' },
    bosses: [boss('ok')],
    ...over,
  });

  it('shows level and CP with their source; CP is never judged', () => {
    expect(readinessLine(slot({}))).toEqual({ values: 'Lv. 276 · CP 52.1M', source: 'NEXON · as of 29 Sep' });
    expect(readinessLine(slot({ link: 'expired', level: { value: 275, source: 'manual', as_of: null }, power: null }))).toEqual({
      values: 'Lv. 275 · CP —',
      source: 'manual · NEXON data expired',
    });
  });

  it('judges each boss by level only', () => {
    expect(levelChip(boss('ok'))).toEqual({ text: 'Lv ok', tone: 'ok' });
    expect(levelChip(boss('under'))).toEqual({ text: 'under Lv. 275', tone: 'warn' });
    expect(levelChip(boss('unknown'))).toEqual({ text: 'Lv unknown', tone: 'neutral' });
  });

  it('replaces CP with an estimate per boss once a slot has a reading', () => {
    const read = slot({ bosses: [boss('ok', reading()), boss('under')] });
    expect(readinessLine(read)).toEqual({ values: 'Lv. 276', source: 'NEXON · as of 29 Sep' });
    expect(scouterAt(read)).toBe('2026-09-29T02:10:00Z');
    expect(scouterAt(slot({}))).toBeNull();
    expect(scouterChip(reading())).toEqual({ text: 'Estimate 164% · ~18 min', rating: '2p Min Cut', assumed: null });
    expect(scouterChip(reading({ percentage: 9, estimated_minutes: null }))).toEqual({ text: 'Estimate 9%', rating: '2p Min Cut', assumed: null });
    expect(scouterChip(reading({ estimated_minutes: 0.2 })).text).toBe('Estimate 164% · ~1 min');
    // An assumed critical rate: a short marker on the chip, the full label for its title.
    expect(scouterChip(reading({ critical_rate_assumed: true, critical_rate_actual: 89 }))).toEqual({
      text: 'Estimate 164% · ~18 min · crit assumed',
      rating: '2p Min Cut',
      assumed: 'Assumes 100% critical rate (actual 89%)',
    });
    // Manual level, no NEXON data: the reading still hides the CP dash.
    expect(readinessLine(slot({ link: 'not_linked', level: { value: 270, source: 'manual', as_of: null }, power: null, bosses: [boss('unknown', reading())] }))).toEqual({
      values: 'Lv. 270',
      source: 'manual · not linked',
    });
  });
});

describe('scouter estimates', () => {
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

  it('labels an assumed critical rate, and only then', () => {
    expect(assumedCrit({ critical_rate_assumed: true, critical_rate_actual: 89 })).toBe('Assumes 100% critical rate (actual 89%)');
    expect(assumedCrit({ critical_rate_assumed: true, critical_rate_actual: 89.46 })).toBe('Assumes 100% critical rate (actual 89.5%)');
    expect(assumedCrit({ critical_rate_assumed: true, critical_rate_actual: null })).toBe('Assumes 100% critical rate');
    expect(assumedCrit({ critical_rate_assumed: false, critical_rate_actual: null })).toBeNull();
  });

  it('says a queued lookup’s place', () => {
    expect(queuedText(2)).toBe('Queued · 2 ahead');
    expect(queuedText(null)).toBe('Queued · 1 ahead');
  });

  it('says each error the boards draw', () => {
    const error = (code: EstimateError['code'], status: number | null, detail = ''): EstimateError => ({ code, message: '', status, detail, retry_after_seconds: 40, character_id: null });
    expect(estimateErrorText(error('critical_rate', 422), 'YuzuPala').text).toContain('YuzuPala’s critical rate under 100% and couldn’t assume 100% for it.');
    expect(estimateErrorText(error('manual_check', 503), null).lead).toBe('Scouter needs a manual check.');
    expect(estimateErrorText(error('failed', 504), null).text).toBe('The scouter didn’t answer within 2 minutes (504). Nothing was kept.');
    expect(estimateErrorText(error('failed', null, 'timeout'), null).text).toBe('The scouter didn’t answer in time. Nothing was kept.');
    expect(estimateErrorText(error('busy', 429), null).text).toBe('It stayed busy with other lookups. Try again in 40 s.');
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

describe('the Add character lookup', () => {
  const found: CharacterLookup = {
    name: 'Wisteria',
    world: 'Aquila',
    class: 'Bishop',
    level: 271,
    image_url: null,
    as_of: '2026-09-28',
    attribution: 'Data based on NEXON Open API',
  };

  it('reads level, class and world in one line, skipping what NEXON left out', () => {
    expect(lookupLine(found)).toBe('Lv. 271 · Bishop · Aquila');
    expect(lookupLine({ ...found, level: null, world: null })).toBe('Bishop');
    expect(lookupLine({ ...found, level: null, class: null, world: null })).toBe('');
  });

  it('puts each refusal in words, and still lets the admin add', () => {
    expect(lookupMiss('not_found', 'Nobody42', 'x')).toEqual({
      miss: 'not_found',
      lead: 'Not found.',
      text: 'NEXON has no MapleSEA character named “Nobody42”. Check the spelling; you can still add it.',
    });
    const held = lookupMiss('nexon_unavailable', 'Wisteria', "The Open API can't be called right now (maintenance).");
    expect(held.miss).toBe('held');
    expect(held.text).toContain('(maintenance). You can still add Wisteria');
    expect(lookupMiss('not_connected', 'Wisteria', 'x').miss).toBe('not_connected');
    expect(lookupMiss('lookup_cooldown', 'Wisteria', 'x').miss).toBe('wait');
    expect(lookupMiss(null, 'Wisteria', 'The server did not answer.')).toEqual({ miss: 'failed', lead: 'Lookup failed.', text: 'The server did not answer.' });
  });

  it('tells the toast whether the member was asked to link', () => {
    expect(createdLinkNote('pending', 'Mika')).toBe(' Asked Mika to link it: they confirm with /character link in Discord.');
    expect(createdLinkNote('failed', 'Mika')).toContain('use Link… in Edit');
    expect(createdLinkNote(null, 'Mika')).toBe('');
  });
});
