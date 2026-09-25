import { describe, expect, it } from 'vitest';
import { CHECK_TONE, RUN_TONE } from '@kanade/ui';
import { FLAG_TONE } from '../src/inbox/flags';
import { outcomeTone } from '../src/logs/filters';

describe('pill profiles', () => {
  it('maps every state to one semantic profile', () => {
    expect([outcomeTone('answered'), outcomeTone('proposed')]).toEqual(['success', 'success']);
    expect([outcomeTone('error'), outcomeTone('timeout'), outcomeTone('failed'), outcomeTone('content_blocked')]).toEqual(['danger', 'danger', 'danger', 'danger']);
    expect([outcomeTone('clarified'), outcomeTone('self_service_link')]).toEqual(['info', 'info']);
    expect([outcomeTone('no_change'), outcomeTone('withheld')]).toEqual(['neutral', 'neutral']);
    expect([outcomeTone('rate_limited'), outcomeTone('turned_away')]).toEqual(['warning', 'warning']);
    expect(RUN_TONE).toEqual({ planned: 'warning', confirmed: 'success', at_risk: 'danger', otot: 'info', done: 'neutral', cancelled: 'neutral' });
    expect(CHECK_TONE).toEqual({ ok: 'success', warning: 'warning', error: 'danger' });
    expect(FLAG_TONE.conflict).toBe('danger');
    expect(FLAG_TONE.expired).toBe('neutral');
  });
});
