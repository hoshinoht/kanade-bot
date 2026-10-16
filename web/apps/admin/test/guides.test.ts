import type { GuideBlock, SettingsChangeRow } from '@kanade/api-types';
import { describe, expect, it } from 'vitest';
import { blockChanges, leadOf, listWords, messagesLabel, slotLines, units } from '../src/guides/guides';
import { guideEvent, type GuideLookup } from '../src/history/guides';

describe('guide editor words', () => {
  it('counts UTF-16 units, as Discord does', () => {
    expect(units('ab')).toBe(2);
    expect(units('🐉')).toBe(2);
  });

  it('names the messages a block renders to', () => {
    expect(messagesLabel([1])).toBe('message 2');
    expect(messagesLabel([2, 3, 4])).toBe('messages 3–5');
    expect(listWords([1, 4, 5])).toBe('1, 4 and 5');
  });

  it('draws headings, subtext and list items', () => {
    expect(slotLines('# Title\n## Part\n-# small\n- item\nplain **bold**').map((l) => l.kind)).toEqual(['h1', 'h2', 'sub', 'li', 'text']);
  });

  it('finds the knowledge lead between the facts and the note', () => {
    expect(leadOf('Easy · Normal\n`EK · NK`\n\nStand left.\n\nBring two.', 'Bring two.')).toBe('Stand left.');
    expect(leadOf('Easy · Normal\n`EK`\n\nBring two.', 'Bring two.')).toBeNull();
  });

  it('lists unsaved block changes for the save bar', () => {
    const saved: GuideBlock[] = [{ kind: 'text', id: 'a', content: 'one' }];
    const draft: GuideBlock[] = [{ kind: 'text', id: 'a', content: 'one two' }, { kind: 'spacer', id: 'b' }];
    expect(blockChanges(saved, draft, { channel: ['#a', '#a'], title: ['T', 'T'] })).toEqual([
      { label: 'block 1', from: '3', to: '7' },
      { label: 'block 2', from: '—', to: 'new spacer' },
    ]);
  });
});

describe('History guide rows', () => {
  const row = (id: number, key: string, from: string, to: string, kind: 'admin' | 'system' = 'admin'): SettingsChangeRow =>
    ({ id, at: '2026-09-29T04:00:00Z', actor: { kind, id: kind === 'admin' ? 'discord:1' : 'guides' }, surface: 'admin_portal', section: 'guides', revision: 0, week: '', values: [{ key, from, to }] }) as unknown as SettingsChangeRow;
  const lookup: GuideLookup = { titles: { g1: 'Limbo party roles' }, byChannel: { c1: 'g1' }, channelName: () => '#guide-limbo' };
  const request = row(2, 'publish.r1', '{"slots":0}', JSON.stringify({ kind: 'publish', version: 1, channel: 'c1', plan: 'x', ops: { keep: 0, edit: 0, create: 7, delete: 0 }, confirmed: {} }));
  const outcome = row(3, 'publish.r1', '{"state":"running"}', JSON.stringify({ state: 'done', stop_reason: null, ops: { done: 7 } }), 'system');
  const all = [row(1, 'guide.g1', '', '{"title":"Limbo party roles","channel":"c1"}'), request, outcome];

  it('words the request, the outcome and the create as the board draws them', () => {
    expect(guideEvent(request, all, lookup)?.summary).toBe('Guides · Limbo party roles — publish requested: add 7');
    expect(guideEvent(request, all, lookup)?.head).toBe('Limbo party roles: publish requested');
    expect(guideEvent(outcome, all, lookup)?.summary).toBe('Guides · Limbo party roles — published: 7 added');
    expect(guideEvent(all[0]!, all, lookup)?.summary).toBe('Guides · Limbo party roles — created');
  });
});
