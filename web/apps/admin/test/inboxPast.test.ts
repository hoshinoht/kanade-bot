import { describe, expect, it } from 'vitest';
import type { PastItem, PastOutcome } from '@kanade/api-types';
import { OUTCOME, outcomeSentence, sourceLink } from '../src/inbox/past';

const TZ = 'Asia/Kuala_Lumpur';

const item = (over: Partial<PastItem>): PastItem => ({
  id: 'c-x',
  short_id: 'x',
  kind: 'move',
  kind_label: 'Move',
  tab: 'extractor',
  source: 'extraction',
  source_id: 'x-1',
  summary: 'Someone asks to move a run',
  channel: null,
  requester: null,
  outcome: 'approved',
  decided_by: { kind: 'member', id: '1001', name: 'Asahi' },
  // 14:00 guild time (UTC+8).
  decided_at: '2026-09-24T06:00:00Z',
  reason: null,
  created_at: '2026-09-24T05:00:00Z',
  history_seq: 3,
  evidence: [],
  card_url: null,
  ...over,
});

describe('past outcomes in words', () => {
  it('gives every outcome its own word and mark, not colour alone', () => {
    const outcomes: PastOutcome[] = ['approved', 'rejected', 'superseded', 'discarded', 'withdrawn', 'expired'];
    expect(new Set(outcomes.map((o) => OUTCOME[o].label)).size).toBe(outcomes.length);
    expect(new Set(outcomes.map((o) => OUTCOME[o].icon)).size).toBe(outcomes.length);
    expect(OUTCOME.approved.tone).toBe('ok');
    expect(OUTCOME.rejected.tone).toBe('risk');
  });

  it('says who decided and when, in guild time', () => {
    expect(outcomeSentence(item({}), TZ)).toBe('Approved by Asahi · Thu 24 Sep 14:00');
    expect(outcomeSentence(item({ outcome: 'rejected', decided_by: { kind: 'admin', id: 'token', name: 'Admin (token)' } }), TZ)).toBe(
      'Rejected by Admin (token) · Thu 24 Sep 14:00',
    );
  });

  it('names no actor for Kanade’s own expiry and superseding', () => {
    const kanade = { kind: 'system' as const, id: 'delivery', name: 'Kanade' };
    expect(outcomeSentence(item({ outcome: 'expired', decided_by: kanade }), TZ)).toBe('Expired · Thu 24 Sep 14:00');
    expect(outcomeSentence(item({ outcome: 'superseded', decided_by: kanade }), TZ)).toBe('Superseded by a newer proposal · Thu 24 Sep 14:00');
    expect(outcomeSentence(item({ outcome: 'withdrawn', decided_by: null }), TZ)).toBe('Withdrawn · Thu 24 Sep 14:00');
  });

  it('links a proposal to the log entry that staged it, never a member request', () => {
    expect(sourceLink(item({}))).toEqual({ href: '/extractions/x-1', label: 'Extraction log entry' });
    expect(sourceLink(item({ source: 'chat', source_id: 'c 1' }))).toEqual({ href: '/chat/c%201', label: 'Chat interaction' });
    expect(sourceLink(item({ source: 'self_service', source_id: null, tab: 'self_service' }))).toBeNull();
  });
});
