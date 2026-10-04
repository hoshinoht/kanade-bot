import { describe, expect, it } from 'vitest';
import {
  effectiveReasoning,
  groupRows,
  isReasoningValid,
  kanataLimits,
  keyLine,
  modelOptions,
  reasoningChoices,
  resetStrandedInheritors,
  rowCheckText,
  splitChecks,
} from '../src/config/capacity';
import type { ModelInfo } from '@kanade/api-types';

const catalog: ModelInfo[] = [
  { id: 'x', trust_zone: 'homelab', leaves_homelab: false, function_tools: false, structured_output: true, sampling_controls: true, reasoning_control: true, reasoning_efforts: ['low', 'high'], admission: { max_in_flight: 1, adapter_max_in_flight: 2 } },
  { id: 'c', trust_zone: 'homelab', leaves_homelab: false, function_tools: true, structured_output: true, sampling_controls: true, reasoning_control: true, reasoning_efforts: ['low', 'medium'], admission: { max_in_flight: 4 } },
  { id: 'd', trust_zone: 'external', leaves_homelab: true, function_tools: false, structured_output: false, sampling_controls: false, reasoning_control: false, reasoning_efforts: [], admission: { max_in_flight: 2 } },
  { id: 'n', trust_zone: 'unknown', leaves_homelab: true, function_tools: false, structured_output: true, sampling_controls: true, reasoning_control: true, reasoning_efforts: null, admission: null },
] as ModelInfo[];
const roles = {
  extraction: { alias: 'x', reasoning: 'low' },
  chat: { alias: 'c', reasoning: '' },
  rewrite: { alias: 'd', reasoning: 'off' },
};
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
    expect(isReasoningValid(byId('n'), 'minimal')).toBe(true);
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
    // `null` efforts: Kanata restricts nothing, so every level is offered (and inherit fits any).
    expect(reasoningChoices('rewrite', byId('n'), 'medium').map((c) => c.value)).toEqual(['', 'off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']);
    expect(reasoningChoices('rewrite', byId('n'), 'xhigh')[0]).toEqual({ value: '', label: 'Same as extraction' });
    expect(isReasoningValid(byId('n'), 'max')).toBe(true);
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

describe('model picker', () => {
  const withVariants = [
    ...catalog,
    { ...catalog[1]!, id: 'c:high', variant_of: 'c', fixed_effort: 'high' },
    { ...catalog[0]!, id: 'r', off_allowed: false, reasoning_efforts: ['low', 'high'] },
  ] as ModelInfo[];

  it('offers base models only, a stored variant as "<base> (fixed: <level>)"', () => {
    const plain = modelOptions('extraction', { alias: 'x', reasoning: 'low' }, withVariants).map((o) => o.value);
    expect(plain).toEqual(['x', 'c', 'd', 'n', 'r']);
    const stored = modelOptions('chat', { alias: 'c:high', reasoning: '', variant_of: 'c', fixed_effort: 'high' }, withVariants);
    expect(stored[0]).toEqual({ value: 'c:high', label: 'c (fixed: high)' });
    expect(stored.filter((o) => o.value.includes(':'))).toHaveLength(1);
    // Chat needs tools.
    expect(stored.find((o) => o.value === 'x')).toMatchObject({ disabled: true, label: 'x (no tools)' });
  });

  it('reads an unset role as "Not configured"', () => {
    expect(modelOptions('rewrite', { alias: '', reasoning: 'off' }, catalog)[0]).toEqual({ value: '', label: 'Not configured' });
  });

  it('hides off for a model that requires reasoning', () => {
    const required = withVariants.find((m) => m.id === 'r');
    expect(reasoningChoices('extraction', required, 'low').map((c) => c.value)).toEqual(['low', 'high']);
    expect(reasoningChoices('extraction', catalog[0], 'low').map((c) => c.value)).toEqual(['off', 'low', 'high']);
  });
});

describe('capacity summary', () => {
  const models = {
    reachable: true,
    catalog: [...catalog, { ...catalog[1]!, id: 'c:high', variant_of: 'c', fixed_effort: 'high' }] as ModelInfo[],
    roles,
    groups: [
      { model: 'x', group: 'gateway', permits: 2, in_use: 1 },
      { model: 'c', group: 'gateway', permits: 2, in_use: 1 },
      { model: 'c:high', group: 'gateway', permits: 2, in_use: 1 },
    ],
    groups_source: 'default' as const,
    alias_limits: [
      { alias: 'x', max_in_flight: 2, source: 'published' as const },
      { alias: 'c', max_in_flight: 2, source: 'published' as const },
      { alias: 'c:high', max_in_flight: 2, source: 'published' as const },
      { alias: 'd', max_in_flight: 2, source: 'declared' as const },
    ],
    key_limits: { max_in_flight: null, shared: true },
    capacity_check: [],
    pii_pseudonymise: false,
  };

  it('folds a group into one row with base models only', () => {
    expect(groupRows(models)).toEqual([{ group: 'gateway', permits: 2, inUse: 1, models: ['x', 'c'] }]);
  });

  it('says one line when every base model admits the same, and never lists variants', () => {
    const uniform = kanataLimits(models);
    expect(uniform.uniform).toBe(2);
    expect(uniform.all.map((l) => l.alias)).toEqual(['x', 'c', 'd']);
    const mixed = kanataLimits({ ...models, alias_limits: [...models.alias_limits, { alias: 'n', max_in_flight: 5, source: 'declared' as const }] });
    expect(mixed.uniform).toBeNull();
    expect(mixed.inUse.map((l) => l.alias)).toEqual(['x', 'c', 'd']);
  });

  it('says the key limit once, in words', () => {
    expect(keyLine({ max_in_flight: null, shared: true })).toBe("Kanata publishes no limit for this key; it is shared with the owner's other clients.");
    expect(keyLine({ max_in_flight: 8, shared: false })).toBe('Kanata admits 8 calls at a time for this key.');
  });
});

describe('startup checks', () => {
  it('puts each group check on its row and keeps cross-group ones for the list, each once', () => {
    const over = { level: 'error' as const, message: 'Group over declares 3 permits but Kanata admits at most 2 (capped by a); the bot refuses to start.', group: 'over' };
    const gone = { level: 'error' as const, message: 'Kanata does not list gone.', group: 'over' };
    const ungrouped = { level: 'warning' as const, message: 'The chat model b is in no capacity group; its calls are refused.', group: null };
    const legacy = { level: 'warning' as const, message: 'Kanata is unreachable; capacity is checked again once it lists its models.' };
    const stray = { level: 'ok' as const, message: 'Group elsewhere: 1 permits.', group: 'elsewhere' };
    const split = splitChecks([ungrouped, over, gone, over, legacy, stray], ['over', 'open']);
    expect(split.byGroup.get('over')).toEqual([over, gone]);
    expect(split.byGroup.has('open')).toBe(false);
    expect(split.rest).toEqual([ungrouped, legacy, stray]);
  });

  it('drops the group name the row already says', () => {
    expect(rowCheckText("Group gateway: 4 permits, matching Kanata's limit.", 'gateway')).toBe("4 permits, matching Kanata's limit.");
    expect(rowCheckText('Group local uses 2 of the 4 permits Kanata admits.', 'local')).toBe('Uses 2 of the 4 permits Kanata admits.');
    expect(rowCheckText('Kanata does not list gone.', 'local')).toBe('Kanata does not list gone.');
    expect(rowCheckText('Group locality: 1 permits.', 'local')).toBe('Group locality: 1 permits.');
  });
});
