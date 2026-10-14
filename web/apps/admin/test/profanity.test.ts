import { describe, expect, it } from 'vitest';
import { check, matches, wordProblem, wordsOf } from '../src/config/profanity';

// Invented placeholder words, as in the pwa-mock.
const saved = { builtin_words: ['blarg', 'drat', 'frak', 'gorram', 'smeg', 'zounds'] };
const draft = (over: Partial<Parameters<typeof check>[0]> = {}) => ({ extra_words: [], allowed_words: [], deflection_line: 'Keep it clean.', ...over });

describe('profanity settings', () => {
  it('reads typed words as the server stores them', () => {
    expect(wordsOf(' Heck, Darn  gosh,')).toEqual(['heck', 'darn', 'gosh']);
    expect(wordsOf('   ')).toEqual([]);
  });

  it('refuses what the server refuses', () => {
    expect(wordProblem('heck', saved)).toBe('');
    expect(wordProblem('é', saved)).toMatch(/2–32 letters/);
    expect(wordProblem('h3ck', saved)).toMatch(/no digits/);
    expect(wordProblem('drat', saved)).toMatch(/already on the built-in list/);
    expect(check(draft({ allowed_words: ['heck'] }), saved)).toEqual({ error: expect.stringMatching(/not on the built-in list/), field: 'allowed' });
    expect(check(draft({ deflection_line: '   ' }), saved)).toMatchObject({ field: 'line' });
    expect(check(draft({ deflection_line: 'x'.repeat(201) }), saved)).toMatchObject({ field: 'line' });
  });

  it('sends a trimmed line and copies of the lists', () => {
    const value = draft({ extra_words: ['heck'], allowed_words: ['smeg'], deflection_line: '  Calm down.  ' });
    expect(check(value, saved)).toEqual({ value: { extra_words: ['heck'], allowed_words: ['smeg'], deflection_line: 'Calm down.' } });
  });

  it('offers built-in words not yet allowed, prefix matches first', () => {
    expect(matches(saved.builtin_words, [], 'ra')).toEqual(['drat', 'frak', 'gorram']);
    expect(matches(saved.builtin_words, ['drat'], 'dr')).toEqual([]);
    expect(matches(saved.builtin_words, [], 'go')).toEqual(['gorram']);
    expect(matches(saved.builtin_words, [], ' ')).toEqual([]);
  });
});
