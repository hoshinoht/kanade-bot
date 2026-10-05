import type { ConfigView } from '@kanade/api-types';

/** Bounds the server holds `profanity` to (docs/v5/admin-api.md, Config). */
export const MAX_WORDS = 100;
export const WORD_CHARS = { min: 2, max: 32 } as const;
export const MAX_LINE_CHARS = 200;

export type Profanity = ConfigView['profanity'];
export type ProfanityPatch = Pick<Profanity, 'extra_words' | 'allowed_words' | 'deflection_line'>;
export type ProfanityField = 'extra' | 'allowed' | 'line';

export const draftOf = (saved: Profanity): ProfanityPatch => ({
  extra_words: [...saved.extra_words],
  allowed_words: [...saved.allowed_words],
  deflection_line: saved.deflection_line,
});

/** The words in a typed entry, as the server stores them: trimmed, lowercased, split on commas or spaces. */
export const wordsOf = (text: string): string[] =>
  text
    .split(/[\s,]+/)
    .map((w) => w.toLowerCase())
    .filter(Boolean);

/** Why one word cannot be a blocked word, or '' when it can. */
export function wordProblem(word: string, saved: Pick<Profanity, 'builtin_words'>): string {
  const length = [...word].length;
  if (!/^\p{L}+$/u.test(word) || length < WORD_CHARS.min || length > WORD_CHARS.max)
    return `“${word}” is not a word of ${WORD_CHARS.min}–${WORD_CHARS.max} letters (no digits, spaces or symbols).`;
  if (saved.builtin_words.includes(word)) return `“${word}” is already on the built-in list.`;
  return '';
}

/** What is wrong with a draft before it is sent (the server says the same), or the patch body. */
export function check(draft: ProfanityPatch, saved: Pick<Profanity, 'builtin_words'>): { error: string; field: ProfanityField } | { value: ProfanityPatch } {
  if (draft.extra_words.length > MAX_WORDS) return { error: `At most ${MAX_WORDS} extra words.`, field: 'extra' };
  for (const word of draft.extra_words) {
    const problem = wordProblem(word, saved);
    if (problem) return { error: problem, field: 'extra' };
  }
  const stray = draft.allowed_words.find((w) => !saved.builtin_words.includes(w));
  if (stray) return { error: `“${stray}” is not on the built-in list, so it cannot be allowed again.`, field: 'allowed' };
  const line = draft.deflection_line.trim();
  if (!line || [...line].length > MAX_LINE_CHARS || /[\p{Cc}]/u.test(line))
    return { error: `The deflection line is one line of 1–${MAX_LINE_CHARS} characters.`, field: 'line' };
  return { value: { extra_words: [...draft.extra_words], allowed_words: [...draft.allowed_words], deflection_line: line } };
}

/** Built-in words not yet allowed that contain the typed text, prefix matches first. */
export function matches(builtin: string[], allowed: string[], typed: string, limit = 8): string[] {
  const text = typed.trim().toLowerCase();
  if (!text) return [];
  const open = builtin.filter((w) => !allowed.includes(w) && w.includes(text));
  return [...open.filter((w) => w.startsWith(text)), ...open.filter((w) => !w.startsWith(text))].slice(0, limit);
}
