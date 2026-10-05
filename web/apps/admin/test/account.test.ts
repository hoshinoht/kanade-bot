import { describe, expect, it } from 'vitest';
import type { Me } from '@kanade/api-types';
import { dayTime, diagnostics, methodLong, seenWords, serverClock, styleChoices, tabOf, windowWords } from '../src/account/account';

const TZ = 'Asia/Kuala_Lumpur';

describe('account words', () => {
  it('reads the tab from the query, Profile by default', () => {
    expect(tabOf('sessions')).toBe('sessions');
    expect(tabOf('browser')).toBe('browser');
    expect(tabOf('nope')).toBe('profile');
    expect(tabOf(null)).toBe('profile');
  });

  it('names each sign-in method', () => {
    expect(methodLong('discord')).toBe('signed in with Discord');
    expect(methodLong('tailscale')).toBe('signed in with Tailscale');
    expect(methodLong('token')).toBe('signed in with a token');
  });

  it('says last seen against the server clock, never the device', () => {
    const now = '2026-09-29T04:00:00Z';
    expect(seenWords('2026-09-29T03:59:30Z', now)).toBe('now');
    expect(seenWords('2026-09-29T03:20:00Z', now)).toBe('40 min ago');
    expect(seenWords('2026-09-29T01:00:00Z', now)).toBe('3 h ago');
    expect(seenWords('2026-09-27T01:00:00Z', now)).toBe('2 d ago');
    expect(seenWords('bad', now)).toBe('');
  });

  it('prints guild-local times', () => {
    expect(dayTime('2026-10-02T13:14:00Z', TZ)).toBe('2 Oct 21:14');
    expect(serverClock('2026-09-29T04:00:00Z', TZ)).toBe('Tue 29 Sep 12:00');
  });

  it('words the allowance window', () => {
    expect(windowWords(300)).toBe('5 min');
    expect(windowWords(86_400)).toBe('1 d');
    expect(windowWords(7200)).toBe('2 h');
    expect(windowWords(45)).toBe('45 s');
  });

  it('copies ids and versions only', () => {
    const me = { display: 'Asahi', method: 'discord', server_time: '2026-09-29T04:00:00Z', version: '1.0.0-beta.3', member: { id: '1001' } } as Me;
    expect(diagnostics(me, TZ)).toBe('User id: 1001\nMethod: discord\nVersion: 1.0.0-beta.3\nServer: Tue 29 Sep 12:00 (Asia/Kuala_Lumpur)');
    expect(diagnostics({ ...me, method: 'token', member: null }, TZ)).toContain('User id: —');
  });

  it('lists the default voice first, then styles A to Z, filtered by name or voice', () => {
    const personas = [
      { key: 'default', name: 'Default', voice: 'Warm and plain' },
      { key: 'terse', name: 'Terse', voice: 'Short to the point of blunt' },
      { key: 'kanade', name: 'Kanade', voice: 'comedy.' },
    ];
    expect(styleChoices(personas, '').map((p) => p.name)).toEqual(['Default voice', 'Kanade', 'Terse']);
    expect(styleChoices(personas, 'BLUNT').map((p) => p.key)).toEqual(['terse']);
    expect(styleChoices(personas, 'written').map((p) => p.key)).toEqual(['']);
    expect(styleChoices(personas, 'zzz')).toEqual([]);
  });
});
