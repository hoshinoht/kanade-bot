import type { ChatTurn } from '@kanade/api-types';
import { describe, expect, it } from 'vitest';
import { preview, plainLines } from '../src/config/markdown';
import { transcriptJson, transcriptMarkdown, rounds } from '../src/chat/transcript';
import { directory } from '../src/names/directory.svelte';
import { artUrl } from '../src/shared/identity';

directory.setMembers([{ id: '1004', name: 'Yuzu' }] as never);
directory.setChannels([{ id: '123', name: 'limbo-trio' }] as never);
directory.setIdentity({ name: 'Kanade', avatar: '', banner: '', cached: false, bot_user_id: '777' });

const ROUND = { model: 'kanata/chat', effort: 'low', route: 'homelab', latency_ms: 2951, guardrail: { clean: false, content_filter: false } } as const;

const turn = (over: Partial<ChatTurn> = {}): ChatTurn => ({
  id: 'c-1',
  at: '2026-09-29T04:00:00Z',
  member: { id: '1004', name: 'Yuzu' },
  member_id: '1004',
  channel: 'limbo-trio',
  channel_id: '123',
  model: 'kanata/chat',
  models: ['kanata/chat', 'kanata/chat'],
  latency_ms: 5902,
  outcome: 'answered',
  asked: '<@777> tips for <#123>?',
  tools_used: ['knowledge.read'],
  said: 'Three tips.',
  tools: [{ round: 1, name: 'knowledge.read', arguments: '{"boss":"Limbo"}', result: 'line 1\nline 2\n… [truncated, 12034 bytes]', took_ms: 12, outcome: 'ok' }],
  rounds: [
    { ...ROUND, round: 1, requested_tools: ['knowledge.read'], finish: 'tool_calls' },
    { ...ROUND, round: 2, requested_tools: [], finish: 'stop', latency_ms: null },
  ],
  cards: [{ kind: 'proposal', url: 'https://discord.com/channels/0/0/1' }],
  raw: '{"role":"assistant"}',
  persona: 'kanade',
  profile: 'gentle',
  profile_source: 'saved',
  route: 'homelab',
  error: null,
  error_code: null,
  guardrail: {},
  masked: false,
  model_view: null,
  ...over,
});

describe('chat transcript', () => {
  it('deals tool calls out to the rounds that requested them', () => {
    const r = rounds(turn());
    expect(r.map((x) => x.calls.length)).toEqual([1, 0]);
    expect(r[0]!.model).toBe('kanata/chat');
  });

  it('groups calls by their round, not by the requested count', () => {
    const call = turn().tools[0]!;
    const r = rounds(turn({ tools: [{ ...call, round: 2 }, { ...call, round: 1 }, { ...call, round: 9 }] }));
    expect(r.map((x) => x.calls.length)).toEqual([1, 2]);
  });

  it('writes round facts, the reply profile and unknown timings', () => {
    const t = turn({ tools: [{ ...turn().tools[0]!, took_ms: null }], error: 'no answer within 60s', error_code: 'timeout' });
    const md = transcriptMarkdown(t, { timeZone: 'UTC' });
    expect(md).toContain('- Reply profile: gentle (saved)');
    expect(md).toContain('- Error: no answer within 60s (timeout)');
    expect(md).toContain('- Effort: low\n- Route: Homelab\n- Latency: 3.0 s');
    expect(md).toContain('- Latency: unknown');
    expect(md).toContain('### knowledge.read — ok, unknown');
  });

  it('writes Markdown with names, not ids, and the full tool result', () => {
    const md = transcriptMarkdown(turn(), { timeZone: 'UTC' });
    expect(md).toContain('- Who: Yuzu');
    expect(md).toContain('- Channel: #limbo-trio');
    expect(md).toContain('@Kanade tips for #limbo-trio?');
    expect(md).toContain('line 2\n… [truncated, 12034 bytes]');
    expect(md).toContain('## Round 2 — kanata/chat');
    expect(md).toContain('- proposal: https://discord.com/channels/0/0/1');
    expect(md).not.toContain('1004');
  });

  it('writes JSON with rounds and calls nested', () => {
    const json = JSON.parse(transcriptJson(turn(), { timeZone: 'UTC' }));
    expect(json.who).toBe('Yuzu');
    expect(json.question).toBe('@Kanade tips for #limbo-trio?');
    expect(json.rounds[0].calls[0].result).toContain('truncated');
    expect(json.models).toEqual(['kanata/chat']);
  });

  it('keeps a withheld turn redacted', () => {
    const W = '[message withheld]';
    const t = turn({ outcome: 'withheld', asked: W, said: '', raw: W, tools: [{ round: 1, name: 'schedule.read', arguments: W, result: W, took_ms: 3, outcome: 'ok' }] });
    const md = transcriptMarkdown(t, { timeZone: 'UTC' });
    expect(md).toContain(W);
    expect(md).not.toContain('Limbo');
    expect(JSON.parse(transcriptJson(t, { timeZone: 'UTC' })).rounds[0].calls[0].arguments).toBe(W);
  });

  it('keeps the tool calls of a turn with no rounds', () => {
    const t = turn({ rounds: [], models: [], tools: [{ round: 1, name: 'schedule.read', arguments: '{}', result: 'ok', took_ms: null, outcome: 'ok' }] });
    const md = transcriptMarkdown(t, { timeZone: 'UTC' });
    expect(md).toContain('## Tool calls\n\n### schedule.read — ok, unknown');
    expect(JSON.parse(transcriptJson(t, { timeZone: 'UTC' })).tool_calls[0].name).toBe('schedule.read');
    expect(JSON.parse(transcriptJson(turn(), { timeZone: 'UTC' })).tool_calls).toBeUndefined();
  });

  it('never copies the masked model view', () => {
    const model_view = {
      rounds: [{ round: 1, clean: false, request: [{ role: 'user', content: 'Midori asks about Limbo' }], reply: 'Midori: SECRET-RAW', tool_calls: [{ name: 'x', arguments: '{"who":"Midori"}' }] }],
      reply: 'Yuzu: done',
      mapping: [{ token: 'Midori', name: 'Yuzu' }],
    };
    const t = turn({ masked: true, model_view } as Partial<ChatTurn>);
    for (const out of [transcriptMarkdown(t, { timeZone: 'UTC' }), transcriptJson(t, { timeZone: 'UTC' })]) {
      expect(out).not.toContain('Midori');
      expect(out).not.toContain('SECRET-RAW');
      expect(out).not.toContain('model_view');
    }
  });

  it('fences the question and reply, so a stray fence or heading cannot break the document', () => {
    const md = transcriptMarkdown(turn({ asked: '# Two Sum\n```py\ndef f(): pass', said: 'use ````a hash map````' }), { timeZone: 'UTC' });
    expect(md).toContain('## Question\n\n````\n# Two Sum\n```py\ndef f(): pass\n````\n\n## Reply');
    expect(md).toContain('## Reply\n\n`````\nuse ````a hash map````\n`````\n');
  });

  it('leaves a reply that was never sent unfenced', () => {
    expect(transcriptMarkdown(turn({ said: '' }), { timeZone: 'UTC' })).toContain('## Reply\n\n— nothing was sent —\n');
  });

  it('fences text that holds backticks with a longer fence', () => {
    const md = transcriptMarkdown(turn({ raw: 'a ``` b' }), { timeZone: 'UTC' });
    expect(md).toContain('````\na ``` b\n````');
  });
});

describe('prompt previews', () => {
  it('cuts at a word within the limit', () => {
    const p = preview('word '.repeat(30).trim());
    expect(p.length).toBeLessThanOrEqual(61);
    expect(p.endsWith('…')).toBe(true);
    expect(preview('short')).toBe('short');
  });

  it('keeps lines when stripping Markdown for the viewer', () => {
    expect(plainLines('# Title\n- **bold** item\n> quote')).toBe('Title\nbold item\nquote');
  });
});

describe('identity art', () => {
  it('tags the URL with a version only when one is sent', () => {
    expect(artUrl('/identity/avatar', {})).toBe('/identity/avatar');
    expect(artUrl('/identity/avatar', { version: 7 })).toBe('/identity/avatar?v=7');
    expect(artUrl('/identity/banner?x=1', { version: 'a b' })).toBe('/identity/banner?x=1&v=a%20b');
  });
});
