import { describe, expect, it } from 'vitest';
import { plainText } from '../src/config/markdown';
import { resolveName, type Known } from '../src/names/directory.svelte';
import { leadingMember, parseMentions } from '../src/names/mentions';

describe('mention parser', () => {
  it('splits text around member, nickname, channel and role mentions', () => {
    expect(parseMentions('<@1543532497948909578> is <@!1004> in <#123> with <@&300001>?')).toEqual([
      { kind: 'member', id: '1543532497948909578' },
      { kind: 'text', text: ' is ' },
      { kind: 'member', id: '1004' },
      { kind: 'text', text: ' in ' },
      { kind: 'channel', id: '123' },
      { kind: 'text', text: ' with ' },
      { kind: 'role', id: '300001' },
      { kind: 'text', text: '?' },
    ]);
  });

  it('leaves text without tokens, and near-misses, alone', () => {
    expect(parseMentions('no mentions here')).toEqual([{ kind: 'text', text: 'no mentions here' }]);
    expect(parseMentions('a <@ 12> b <#> c')).toEqual([{ kind: 'text', text: 'a <@ 12> b <#> c' }]);
    expect(parseMentions('')).toEqual([]);
  });

  it('finds the member a question opens with', () => {
    expect(leadingMember('  <@99> when is carling')).toBe('99');
    expect(leadingMember('when is <@99>')).toBeNull();
    expect(leadingMember('<#5> hi')).toBeNull();
  });
});

describe('names, never ids', () => {
  const known: Known = {
    members: new Map([['1004', 'Yuzu']]),
    channels: new Map([['123', 'limbo-trio']]),
    roles: new Map([['300001', '@staff']]),
    bot: { id: '777', name: 'Kanade' },
  };

  it('uses the lists first, then a real server name', () => {
    expect(resolveName(known, 'member', '1004')).toBe('Yuzu');
    expect(resolveName(known, 'member', '1004', '', true)).toBe('@Yuzu');
    expect(resolveName(known, 'member', '5', 'Hotaru')).toBe('Hotaru');
    expect(resolveName(known, 'channel', '123')).toBe('#limbo-trio');
    expect(resolveName(known, 'channel', '9', '#fa-night')).toBe('#fa-night');
    expect(resolveName(known, 'role', '300001')).toBe('@staff');
    expect(resolveName(known, 'member', '777', '', true)).toBe('@Kanade');
  });

  it('falls back to neutral words for placeholders, never the id', () => {
    expect(resolveName(known, 'member', '114948601234567890', 'user 11494860')).toBe('Unknown member');
    expect(resolveName(known, 'member', '5', '5')).toBe('Unknown member');
    expect(resolveName(known, 'member', '5')).toBe('Unknown member');
    expect(resolveName(known, 'channel', '999000111222333444', '999000111222333444')).toBe('#unknown-channel');
    expect(resolveName(known, 'channel', '9', null as unknown as string)).toBe('#unknown-channel');
    expect(resolveName(known, 'role', '42')).toBe('@unknown-role');
  });
});

describe('profile summaries as plain text', () => {
  it('drops Markdown marks, keeps the words', () => {
    expect(plainText("## Kanade\n- **Teases** lightly in the persona's voice\n- keeps every *schedule* fact exact")).toBe(
      "Kanade Teases lightly in the persona's voice keeps every schedule fact exact",
    );
    expect(plainText('1. See [the guide](https://x) and `code`')).toBe('See the guide and code');
    expect(plainText('snake_case_names stay')).toBe('snake_case_names stay');
  });
});
