import type { Extraction, Rewrite } from '@kanade/api-types';
import { describe, expect, it } from 'vitest';
import { extractionJson, extractionMarkdown } from '../src/extractions/transcript';
import { directory } from '../src/names/directory.svelte';
import { rewriteJson, rewriteMarkdown } from '../src/rewrites/transcript';

directory.setMembers([
  { id: '1002', name: 'Ren' },
  { id: '1001', name: 'Yuzu' },
] as never);
directory.setChannels([{ id: '456', name: 'kalos-four' }] as never);

const UTC = { timeZone: 'UTC' };

const rewrite = (over: Partial<Rewrite> = {}): Rewrite => ({
  id: 'rw-over',
  short_id: 'rwover',
  at: '2026-09-29T04:00:00Z',
  kind: 'countdown',
  stage: 'batch',
  context: 'countdown:r-kalos:60',
  verdict: 'unavailable',
  rule: null,
  code: 'budget_exceeded',
  latency_ms: 14_708,
  model: 'kanata/rewrite',
  reasoning: 'medium',
  prompt_tokens: 300,
  completion_tokens: 112,
  reasoning_tokens: 98,
  reservation: 287,
  budget: null,
  max_output_tokens: 96,
  prompt_estimate: 191,
  request_id: 'kanade-rewrite-0000beef-104-1',
  seed: 'Onward!',
  reply: 'Waku waku!',
  reasoning_content: 'Which interjection fits?',
  line: 'Onward!',
  ...over,
});

describe('rewrite transcript', () => {
  it('writes the verdict, the call and its token check, then the seed, reply, line and reasoning', () => {
    const md = rewriteMarkdown(rewrite(), UTC);
    expect(md).toMatch(/^# Rewrite rw-over \(#rwover\)\n\n- When: Tue 29 Sep 2026, 04:00:00 \(UTC\) \(2026-09-29T04:00:00Z\)\n/);
    expect(md).toContain('- Kind: countdown\n- Stage: daily batch\n- Context: countdown:r-kalos:60\n- Verdict: unavailable (budget_exceeded)');
    expect(md).toContain('- Model: kanata/rewrite\n- Effort: medium\n- Latency: 15 s');
    expect(md).toContain('- Tokens: prompt 300, completion 112\n- Reasoning tokens: 98\n- Token check: used 412 > reserved 287');
    expect(md).toContain('- Max tokens: 96\n- Prompt estimate: 191\n- Request id: kanade-rewrite-0000beef-104-1');
    expect(md).toContain('## Seed\n\n```\nOnward!\n```\n\n## Reply\n\n```\nWaku waku!\n```\n\n## Line used\n\n```\nOnward!\n```\n\n## Reasoning\n\n```\nWhich interjection fits?\n```\n');
  });

  it('reads unknowns as unknown and leaves out what was never sent', () => {
    const r = rewrite({
      verdict: 'no_persona',
      code: null,
      model: null,
      reasoning: null,
      latency_ms: null,
      prompt_tokens: null,
      completion_tokens: null,
      reasoning_tokens: null,
      reservation: null,
      max_output_tokens: null,
      prompt_estimate: null,
      request_id: null,
      context: null,
      reply: null,
      reasoning_content: null,
      line: null,
    });
    const md = rewriteMarkdown(r, UTC);
    expect(md).toContain('- Context: —\n- Verdict: no persona\n- Model: no model call\n- Effort: —\n- Latency: unknown\n- Tokens: prompt unknown, completion unknown\n- Max tokens: unknown');
    for (const absent of ['Reasoning tokens', 'Token check', 'Request id', '## Reasoning']) expect(md).not.toContain(absent);
    expect(md).toContain('## Reply\n\n— nothing came back —\n\n## Line used\n\n—\n');
    // A real 0 ms (refused before sending) is not unknown.
    expect(rewriteMarkdown(rewrite({ latency_ms: 0 }), UTC)).toContain('- Latency: 0 ms');
    const json = JSON.parse(rewriteJson(r, UTC));
    expect(json.latency_ms).toBeNull();
    expect(json.prompt_tokens).toBeNull();
    expect(json.token_check).toBeNull();
    expect(json.reply).toBeNull();
    expect(json.request_id).toBeNull();
  });

  it('writes JSON with raw values and the token check in words', () => {
    const json = JSON.parse(rewriteJson(rewrite({ reservation: 16_391, budget: 16_384 }), UTC));
    expect(json).toMatchObject({
      id: 'rw-over',
      when: 'Tue 29 Sep 2026, 04:00:00 (UTC)',
      kind: 'countdown',
      stage: 'batch',
      verdict: 'unavailable',
      code: 'budget_exceeded',
      effort: 'medium',
      reservation: 16_391,
      budget: 16_384,
      token_check: 'reserved 16,391 > budget 16,384',
      seed: 'Onward!',
      reply: 'Waku waku!',
      line: 'Onward!',
      reasoning_content: 'Which interjection fits?',
    });
    expect(rewriteJson(rewrite(), UTC).endsWith('}\n')).toBe(true);
  });

  it('fences a reply holding backticks with a longer fence', () => {
    expect(rewriteMarkdown(rewrite({ reply: 'a ``` b' }), UTC)).toContain('## Reply\n\n````\na ``` b\n````');
  });
});

const call = (over: Partial<Extraction> = {}): Extraction => ({
  id: 'x-kalos',
  short_id: 'c5d6e7f8',
  at: '2026-09-29T04:00:00Z',
  model: 'kanata/extract',
  latency_ms: 11_874,
  channel: 'kalos-four',
  channel_id: '456',
  error: null,
  outcome: 'proposed',
  prompt: 'System: reply with JSON only.\n\nMessages:\n[Ren] kalos 10pm instead?',
  raw_response: '{"amendments":[{"kind":"move"}]}',
  amendments: [{ kind: 'move', bosses: 'XKalos', when: 'Fri 22:00', confidence: 0.93, status: 'confirmed' }],
  messages: [
    { id: 'm1', author: 'Ren', author_id: '1002', at: '2026-09-29T03:55:00Z', content: 'kalos 10pm instead? <@1001>' },
    { id: 'm2', author: 'Yuzu', author_id: '1001', at: '2026-09-29T03:56:00Z', content: 'ok 10\nsee you in <#456>' },
  ],
  prompt_tokens: 2_010,
  completion_tokens: 72,
  reasoning_tokens: 24,
  prompt_estimate: 1_880,
  context: { window: 8_192, reserve: 2_500, source: 'local_default' },
  reasoning_content: 'They agree on 22:00.',
  refusals: [],
  session_id: 'kanade-extraction-1a2b3c4d-3',
  request_ids: ['kanade-extraction-1a2b3c4d-3-1', 'kanade-extraction-1a2b3c4d-3-2'],
  ...over,
});

describe('extraction transcript', () => {
  it('writes Markdown with names, not ids, and every part of the call', () => {
    const md = extractionMarkdown(call(), UTC);
    expect(md).toMatch(/^# Extraction x-kalos \(#c5d6e7f8\)\n/);
    expect(md).toContain('- Channel: #kalos-four\n- Model: kanata/extract\n- Outcome: proposed\n- Latency: 12 s');
    expect(md).toContain('- Tokens: prompt 2,010, completion 72\n- Reasoning tokens: 24\n- Prompt estimate: 1,880\n- Context: window 8,192, reserve 2,500 (local_default)');
    expect(md).toContain('- Gateway session: kanade-extraction-1a2b3c4d-3\n- Request ids: kanade-extraction-1a2b3c4d-3-1, kanade-extraction-1a2b3c4d-3-2');
    expect(md).toContain(
      '## Messages read (2)\n\n```\n[Tue 29 Sep 2026, 03:55:00 (UTC)] Ren: kalos 10pm instead? @Yuzu\n[Tue 29 Sep 2026, 03:56:00 (UTC)] Yuzu: ok 10\nsee you in #kalos-four\n```',
    );
    expect(md).toContain('## Changes proposed (1)\n\n- move · XKalos · Fri 22:00 · confidence 0.93 · confirmed');
    expect(md).toContain('## Reasoning\n\n```\nThey agree on 22:00.\n```');
    expect(md).toContain('## Prompt as sent\n\n```\nSystem: reply with JSON only.');
    // The raw response pretty-printed, as the Raw tab shows it.
    expect(md).toContain('## Raw response\n\n```\n{\n  "amendments": [');
    expect(md).not.toMatch(/\b100[12]\b|\b456\b/);
    expect(md).not.toContain('Refused changes');
    expect(md).not.toContain('- Error:');
  });

  it('writes a failed call: the error, no response, unknown counts, nothing to change', () => {
    const md = extractionMarkdown(
      call({
        outcome: 'failed',
        error: 'Gateway timed out after 60 s.',
        latency_ms: null,
        raw_response: 'null',
        amendments: [],
        messages: [],
        prompt_tokens: null,
        completion_tokens: null,
        reasoning_tokens: null,
        prompt_estimate: undefined,
        context: null,
        reasoning_content: null,
        session_id: undefined,
        request_ids: undefined,
        refusals: undefined,
      }),
      UTC,
    );
    expect(md).toContain('- Outcome: failed\n- Error: Gateway timed out after 60 s.\n- Latency: unknown\n- Tokens: prompt unknown, completion unknown\n- Prompt estimate: unknown\n- Context: —\n\n');
    expect(md).toContain('## Messages read (0)\n\n— no messages were stored —');
    expect(md).toContain('## Changes proposed (0)\n\nnone');
    expect(md).toContain('## Raw response\n\n— no response (the call failed) —\n');
    for (const absent of ['Reasoning tokens', 'Gateway session', 'Request ids', '## Reasoning']) expect(md).not.toContain(absent);
  });

  it('lists the changes the scheduler refused', () => {
    const md = extractionMarkdown(call({ refusals: [{ change: 'move XKalos to Fri 22:00', code: 'stale_version', message: 'The week changed first.' }] }), UTC);
    expect(md).toContain('## Refused changes (1)\n\n- move XKalos to Fri 22:00 — stale_version: The week changed first.');
  });

  it('names a channel and an author the directory does not know from the call itself', () => {
    const md = extractionMarkdown(
      call({ channel: 'new-party', channel_id: '999', messages: [{ id: 'm', author: 'Mika', at: '2026-09-29T03:55:00Z', content: 'hi' }] }),
      UTC,
    );
    expect(md).toContain('- Channel: #new-party');
    expect(md).toContain('] Mika: hi');
    expect(extractionMarkdown(call({ channel: null, channel_id: '' }), UTC)).toContain('- Channel: —');
  });

  it('writes JSON with names, messages nested, and absent fields as null or empty', () => {
    const json = JSON.parse(extractionJson(call(), UTC));
    expect(json).toMatchObject({ id: 'x-kalos', channel: '#kalos-four', outcome: 'proposed', prompt_tokens: 2_010, request_ids: ['kanade-extraction-1a2b3c4d-3-1', 'kanade-extraction-1a2b3c4d-3-2'] });
    expect(json.messages[0]).toEqual({ who: 'Ren', at: '2026-09-29T03:55:00Z', when: 'Tue 29 Sep 2026, 03:55:00 (UTC)', content: 'kalos 10pm instead? @Yuzu' });
    expect(json.amendments[0].bosses).toBe('XKalos');
    expect(json.raw_response).toBe('{"amendments":[{"kind":"move"}]}');
    expect(json.prompt).toContain('[Ren] kalos 10pm instead?');
    const failed = JSON.parse(
      extractionJson(call({ raw_response: 'null', session_id: undefined, request_ids: undefined, refusals: undefined, reasoning_content: undefined, prompt_tokens: undefined }), UTC),
    );
    expect(failed.raw_response).toBeNull();
    expect(failed.session_id).toBeNull();
    expect(failed.request_ids).toEqual([]);
    expect(failed.refusals).toEqual([]);
    expect(failed.reasoning_content).toBeNull();
    expect(failed.prompt_tokens).toBeNull();
  });
});
