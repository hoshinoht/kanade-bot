import type { ChatTurn } from '@kanade/api-types';
import { describe, expect, it } from 'vitest';
import { preview, plainLines } from '../src/config/markdown';
import { transcriptJson, transcriptMarkdown, rounds } from '../src/chat/transcript';
import { directory } from '../src/names/directory.svelte';
import { artUrl } from '../src/shared/identity';

directory.setMembers([{ id: '1004', name: 'Yuzu' }] as never);
directory.setChannels([{ id: '123', name: 'limbo-trio' }] as never);
directory.setIdentity({ name: 'Kanade', avatar: '', banner: '', cached: false, bot_user_id: '777' });

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
  tools: [{ name: 'knowledge.read', arguments: '{"boss":"Limbo"}', result: 'line 1\nline 2\n… [truncated, 12034 bytes]', took_ms: 12, outcome: 'ok' }],
  rounds: [
    { round: 1, requested_tools: ['knowledge.read'], finish: 'tool_calls' },
    { round: 2, requested_tools: [], finish: 'stop' },
  ],
  cards: [{ kind: 'proposal', url: 'https://discord.com/channels/0/0/1' }],
  raw: '{"role":"assistant"}',
  ...over,
});

describe('chat transcript', () => {
  it('deals tool calls out to the rounds that requested them', () => {
    const r = rounds(turn());
    expect(r.map((x) => x.calls.length)).toEqual([1, 0]);
    expect(r[0]!.model).toBe('kanata/chat');
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
    const t = turn({ outcome: 'withheld', asked: W, said: '', raw: W, tools: [{ name: 'schedule.read', arguments: W, result: W, took_ms: 3, outcome: 'ok' }] });
    const md = transcriptMarkdown(t, { timeZone: 'UTC' });
    expect(md).toContain(W);
    expect(md).not.toContain('Limbo');
    expect(JSON.parse(transcriptJson(t, { timeZone: 'UTC' })).rounds[0].calls[0].arguments).toBe(W);
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
