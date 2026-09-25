import { describe, expect, it } from 'vitest';
import {
  capacityCheck,
  effectiveReasoning,
  isReasoningValid,
  reasoningChoices,
  resetStrandedInheritors,
  type CapacityInputs,
} from '../src/config/capacity';
import type { ModelInfo } from '@kanade/api-types';

const catalog: ModelInfo[] = [
  { id: 'x', trust_zone: 'homelab', leaves_homelab: false, function_tools: false, structured_output: true, sampling_controls: true, reasoning_control: true, reasoning_efforts: ['low', 'high'], admission: { max_in_flight: 1, adapter_max_in_flight: 2 } },
  { id: 'c', trust_zone: 'homelab', leaves_homelab: false, function_tools: true, structured_output: true, sampling_controls: true, reasoning_control: true, reasoning_efforts: ['low', 'medium'], admission: { max_in_flight: 4 } },
  { id: 'd', trust_zone: 'external', leaves_homelab: true, function_tools: false, structured_output: false, sampling_controls: false, reasoning_control: false, reasoning_efforts: [], admission: { max_in_flight: 2 } },
  { id: 'n', trust_zone: 'unknown', leaves_homelab: true, function_tools: false, structured_output: true, sampling_controls: true, reasoning_control: true, reasoning_efforts: null, admission: null },
] as ModelInfo[];
const aliasLimits = [
  { alias: 'x', max_in_flight: 1, adapter_max_in_flight: 2, source: 'published' },
  { alias: 'c', max_in_flight: 4, source: 'published' },
  { alias: 'd', max_in_flight: 2, source: 'declared' },
] as const;
const roles = {
  extraction: { alias: 'x', reasoning: 'low' },
  chat: { alias: 'c', reasoning: '' },
  rewrite: { alias: 'd', reasoning: 'off' },
};
const base: CapacityInputs = {
  groups: [
    { model: 'x', group: 'extract', permits: 1 },
    { model: 'c', group: 'chat', permits: 4 },
    { model: 'd', group: 'rewrite', permits: 2 },
  ],
  roles,
  catalog,
  aliasLimits: [...aliasLimits],
  keyLimits: { max_in_flight: 8, shared: true },
};

const levels = (out: { level: string }[]) => out.map((c) => c.level);

describe('capacityCheck', () => {
  it('passes a fitting declaration with the shared-key warning', () => {
    const out = capacityCheck(base);
    expect(levels(out)).toEqual(['ok', 'ok', 'ok', 'warning']);
    expect(out[3]!.message).toContain('shared');
  });

  it('flags permits over the per-row ceiling, naming the row', () => {
    const out = capacityCheck({ ...base, groups: [...base.groups, { model: 'n', group: 'big', permits: 65 }] });
    expect(out.map((c) => c.message)).toContain('Row 4: permits are at most 64.');
  });

  it('treats a null adapter cap as no cap, not zero', () => {
    const nullAdapter = [{ alias: 'd', max_in_flight: 2, adapter_max_in_flight: null, source: 'declared' }] as unknown as CapacityInputs['aliasLimits'];
    const out = capacityCheck({ ...base, groups: [{ model: 'd', group: 'rewrite', permits: 2 }], aliasLimits: nullAdapter });
    expect(out.filter((c) => c.level === 'error')).toEqual([]);
  });

  it('caps a group by the adapter limit, not the route', () => {
    const out = capacityCheck({ ...base, groups: [{ model: 'x', group: 'extract', permits: 2 }] });
    expect(out.some((c) => c.level === 'error' && c.message.includes('admits at most 1'))).toBe(true);
  });

  it('names rows for two identical zero-permit rows instead of crashing on keys', () => {
    const out = capacityCheck({
      ...base,
      groups: [
        { model: 'x', group: 'extract', permits: 0 },
        { model: 'x', group: 'extract', permits: 0 },
      ],
    });
    const zeroes = out.filter((c) => c.message.includes('declares 0 permits'));
    expect(zeroes.map((c) => c.message)).toEqual([
      'Row 1: x in group extract declares 0 permits.',
      'Row 2: x in group extract declares 0 permits.',
    ]);
  });

  it('treats cleared (null) and non-integer permits as invalid, naming the row', () => {
    for (const permits of [null, Number.NaN, 1.5]) {
      const out = capacityCheck({ ...base, groups: [{ model: 'x', group: 'extract', permits: permits as null }] });
      expect(out.some((c) => c.level === 'error' && c.message === 'Row 1: permits must be a whole number.')).toBe(true);
    }
  });

  it('refuses an alias twice in one group, in two groups, unknown, or groupless', () => {
    const twice = capacityCheck({ ...base, groups: [...base.groups, { model: 'c', group: 'chat', permits: 1 }] });
    expect(twice.some((c) => c.level === 'error' && c.message.includes('twice'))).toBe(true);
    const split = capacityCheck({
      ...base,
      groups: [
        { model: 'c', group: 'chat', permits: 2 },
        { model: 'c', group: 'other', permits: 1 },
      ],
    });
    expect(split.some((c) => c.level === 'error' && c.message.includes('exactly one group'))).toBe(true);
    const unknown = capacityCheck({ ...base, groups: [{ model: 'gone', group: 'extract', permits: 1 }] });
    expect(unknown.some((c) => c.level === 'error' && c.message === 'Row 1: Kanata does not list gone.')).toBe(true);
    const empty = capacityCheck({ ...base, groups: [{ model: 'x', group: '', permits: 1 }] });
    expect(empty.some((c) => c.level === 'error' && c.message === 'Row 1: every row needs a group name.')).toBe(true);
  });

  it('warns, not errors, when a role model is in no group', () => {
    const out = capacityCheck({ ...base, groups: [{ model: 'x', group: 'extract', permits: 1 }] });
    expect(out.filter((c) => c.level === 'error')).toEqual([]);
    expect(out.some((c) => c.level === 'warning' && c.message.includes('chat') && c.message.includes('no capacity group'))).toBe(true);
  });

  it('refuses a key sum over the key limit', () => {
    const out = capacityCheck({
      ...base,
      groups: [
        { model: 'x', group: 'extract', permits: 1 },
        { model: 'c', group: 'chat', permits: 8 },
      ],
    });
    expect(out.some((c) => c.level === 'error' && c.message.includes('the key admits 8'))).toBe(true);
  });
});

describe('reasoning', () => {
  const byId = (id: string) => catalog.find((m) => m.id === id);
  it('resolves inherit to the extraction effort', () => {
    expect(effectiveReasoning('', 'medium')).toBe('medium');
    expect(effectiveReasoning('off', 'medium')).toBe('off');
  });

  it('validates off, published, and model-decides levels', () => {
    expect(isReasoningValid(byId('x'), 'off')).toBe(true);
    expect(isReasoningValid(byId('x'), 'high')).toBe(true);
    expect(isReasoningValid(byId('x'), 'medium')).toBe(false);
    expect(isReasoningValid(byId('n'), 'medium')).toBe(true);
    expect(isReasoningValid(byId('n'), 'minimal')).toBe(false);
    expect(isReasoningValid(byId('d'), 'low')).toBe(false);
    expect(isReasoningValid(undefined, 'low')).toBe(false);
  });

  it('offers inherit only when the extraction effort fits the role model', () => {
    // chat publishes low/medium; extraction runs low.
    expect(reasoningChoices('chat', byId('c'), 'low').map((c) => c.value)).toEqual(['', 'off', 'low', 'medium']);
    // Extraction moved to high: no inherit, choose explicitly.
    expect(reasoningChoices('chat', byId('c'), 'high').map((c) => c.value)).toEqual(['off', 'low', 'medium']);
    // Off is trivially valid everywhere.
    expect(reasoningChoices('chat', byId('c'), 'off')[0]).toEqual({ value: '', label: 'Same as extraction' });
    // A model that decides takes v4's low/medium/high.
    expect(reasoningChoices('rewrite', byId('n'), 'medium').map((c) => c.value)).toEqual(['', 'off', 'low', 'medium', 'high']);
    // Extraction never inherits.
    expect(reasoningChoices('extraction', byId('x'), 'low').map((c) => c.value)).toEqual(['off', 'low', 'high']);
    // Unknown aliases keep a marked current value instead of a blank box.
    expect(reasoningChoices('chat', undefined, 'low', 'high')).toEqual([
      { value: 'off', label: 'Off' },
      { value: 'high', label: 'high (not listed)' },
    ]);
  });

  it('never renders a stored inherit as a blank box', () => {
    const choices = reasoningChoices('chat', byId('c'), 'high', '');
    expect(choices[0]).toEqual({ value: '', label: 'Same as extraction (high, not published)' });
  });

  it('resets inheriting roles to off when extraction moves to an unpublished effort', () => {
    const local = structuredClone({ ...roles, rewrite: { alias: 'n', reasoning: '' } });
    local.extraction.reasoning = 'high';
    // chat (low/medium) cannot take high; the model-decides alias can.
    expect(resetStrandedInheritors(local, catalog)).toEqual(['chat']);
    expect(local.chat.reasoning).toBe('off');
    expect(local.rewrite.reasoning).toBe('');
    // Nothing to do once every inherit fits.
    local.extraction.reasoning = 'low';
    local.chat.reasoning = '';
    expect(resetStrandedInheritors(local, catalog)).toEqual([]);
  });
});
