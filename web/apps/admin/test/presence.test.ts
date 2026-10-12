import { describe, expect, it } from 'vitest';
import type { PresenceEntry } from '@kanade/api-types';
import { ageText, buttonLabel, editingNote, FADE_S, GONE_S, names, nextSeq, shown, startedEditing, tone } from '../src/presence/presence';

const entry = (name: string, id: string, mode: 'viewing' | 'editing', seen_s: number): PresenceEntry => ({ admin: { id, name }, mode, seen_s });
const ren = entry('Ren', 'discord:1002', 'editing', 0);
const kaito = entry('Kaito', 'discord:1009', 'viewing', 40);
const at = 1_000_000;

describe('presence fade', () => {
  it('ages entries on this tab’s clock from the answer', () => {
    expect(shown([ren], at, at).map((e) => [e.age, e.stale])).toEqual([[0, false]]);
    expect(shown([ren], at, at + 19_999).map((e) => [e.age, e.stale])).toEqual([[19, false]]);
    expect(shown([ren], at, at + FADE_S * 1000).map((e) => [e.age, e.stale])).toEqual([[20, true]]);
    // A clock that went back never makes an entry younger than the answer said.
    expect(shown([kaito], at, at - 5_000)[0]!.age).toBe(40);
  });

  it('drops an entry at sixty seconds', () => {
    expect(shown([kaito], at, at + 19_000).map((e) => e.name)).toEqual(['Kaito']);
    expect(shown([kaito], at, at + (GONE_S - 40) * 1000)).toEqual([]);
    expect(shown([ren, kaito], at, at + 30_000).map((e) => [e.name, e.stale])).toEqual([['Ren', true]]);
  });

  it('says the age in tens of seconds once faded', () => {
    expect(ageText(0)).toBe('now');
    expect(ageText(19)).toBe('now');
    expect(ageText(20)).toBe('20 s ago');
    expect(ageText(47)).toBe('40 s ago');
    expect(ageText(59)).toBe('50 s ago');
  });

  it('names the button in words, ages included', () => {
    expect(buttonLabel(shown([ren, kaito], at, at))).toBe('Other admins here: Ren, editing; Kaito, viewing, last seen 40 seconds ago');
  });

  it('keeps one colour per admin', () => {
    expect(tone('discord:1002')).toBe(1);
    expect(tone('discord:1009')).toBe(0);
    expect(tone('discord:1002')).toBe(shown([ren], at, at)[0]!.tone);
  });
});

describe('presence words', () => {
  it('joins names', () => {
    expect(names([])).toBe('');
    expect(names([{ name: 'Ren' }])).toBe('Ren');
    expect(names([{ name: 'Ren' }, { name: 'Kaito' }, { name: 'Mika' }])).toBe('Ren, Kaito and Mika');
  });

  it('says what the version check will do only when someone else edits', () => {
    const list = shown([ren, kaito], at, at);
    expect(editingNote('request', list, false)).toEqual({ lead: 'Ren is editing this too.', rule: 'The first approve wins; the other is refused and starts over.' });
    expect(editingNote('request', list, true)).toEqual({ lead: 'Ren is editing this too', rule: 'The first approve wins; the other is refused.' });
    expect(editingNote('run', list, false)).toEqual({
      lead: 'Ren is editing this run too.',
      rule: 'Nothing is locked: if Ren’s change lands first, yours is refused as out of date and the sheet shows the new run.',
    });
    expect(editingNote('run', list, true)!.rule).toBe('Nothing is locked: if Ren’s change lands first, yours is refused and the sheet shows the new run.');
    expect(editingNote('run', shown([kaito], at, at), false)).toBeNull();
    const two = shown([ren, entry('Mika', 'discord:1003', 'editing', 3)], at, at);
    expect(editingNote('run', two, false)!.lead).toBe('Ren and Mika are editing this run too.');
  });

  it('announces once, only a new editor of what you are editing', () => {
    const list = shown([ren, kaito], at, at);
    expect(startedEditing([], list, 'editing', 'request')).toBe('Ren started editing this request');
    expect(startedEditing(['discord:1002'], list, 'editing', 'request')).toBe('');
    expect(startedEditing([], list, 'viewing', 'run')).toBe('');
  });
});

describe('presence seq', () => {
  it('grows with every call, even within one millisecond or when the clock goes back', () => {
    let last = 0;
    const calls = [1000, 1000, 1000, 999.7, 1500, 1200].map((now) => (last = nextSeq(last, now)));
    expect(calls).toEqual([1000, 1001, 1002, 1003, 1500, 1501]);
    // A leave right after a check-in is always newer than it.
    expect(nextSeq(calls.at(-1)!, 1200)).toBeGreaterThan(calls.at(-1)!);
  });
});
