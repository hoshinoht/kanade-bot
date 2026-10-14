import { describe, expect, it } from 'vitest';
import { activeCount, parseFilter, toSearch } from '../src/logs/filters';
import { attemptOf, budget, overran, verdictText, withAttempt } from '../src/rewrites/format';

describe('rewrites log', () => {
  it('keeps kind, stage and verdicts, and drops channel, member and Chat-only keys', () => {
    const search = '?kind=countdown&stage=batch&verdict=unavailable,rejected&channel=star&member=1001&tool=x&outcome=failed';
    const filter = parseFilter(search, { chat: false, rewrites: true });
    expect(filter.kind).toBe('countdown');
    expect(filter.stage).toBe('batch');
    expect(filter.outcome).toEqual(['unavailable', 'rejected']);
    expect([filter.channel, filter.member, filter.tool]).toEqual(['', '', '']);
    expect(activeCount(filter)).toBe(3);
    expect(toSearch(filter, { rewrites: true })).toBe('?kind=countdown&stage=batch&verdict=unavailable%2Crejected');
    expect(parseFilter(toSearch(filter, { rewrites: true }), { chat: false, rewrites: true })).toEqual(filter);
  });

  it('never carries Rewrites keys into Chat or Extractions', () => {
    expect(parseFilter('?kind=nudge&stage=debug').kind).toBe('');
    expect(toSearch(parseFilter('?kind=nudge&outcome=answered'))).toBe('?outcome=answered');
  });

  it('selects an attempt in the query string', () => {
    expect(withAttempt('?kind=nudge', 'rw-1')).toBe('?kind=nudge&attempt=rw-1');
    expect(withAttempt('?attempt=rw-1&kind=nudge', '')).toBe('?kind=nudge');
    expect(attemptOf('?attempt=rw-2')).toBe('rw-2');
    expect(attemptOf('')).toBe('');
  });

  it('names the rule or code beside the verdict', () => {
    expect(verdictText({ verdict: 'accepted', rule: null, code: null })).toBe('accepted');
    expect(verdictText({ verdict: 'rejected', rule: 'factual term', code: null })).toBe('rejected (factual term)');
    expect(verdictText({ verdict: 'unavailable', rule: null, code: 'budget_exceeded' })).toBe('unavailable (budget_exceeded)');
    expect(verdictText({ verdict: 'no_persona', rule: null, code: null })).toBe('no persona');
  });

  it('shows usage against the reservation', () => {
    const over = { prompt_tokens: 300, completion_tokens: 112, reservation: 287, budget: null };
    expect(budget(over)).toBe('used 412 > reserved 287');
    expect(overran(over)).toBe(true);
    expect(budget({ prompt_tokens: 191, completion_tokens: 9, reservation: 287, budget: null })).toBe('used 200 of 287 reserved');
    expect(budget({ prompt_tokens: null, completion_tokens: null, reservation: 1287, budget: null })).toBe('reserved 1,287');
    expect(budget({ prompt_tokens: 10, completion_tokens: 2, reservation: null, budget: null })).toBe('used 12');
    expect(budget({ prompt_tokens: null, completion_tokens: null, reservation: null, budget: null })).toBeNull();
    expect(overran({ prompt_tokens: null, completion_tokens: null, reservation: 10, budget: null })).toBe(false);
    const refused = { prompt_tokens: null, completion_tokens: null, reservation: 16_191, budget: 16_384 };
    expect(budget(refused)).toBe('reserved 16,191 > budget 16,384');
    expect(overran(refused)).toBe(true);
  });
});
