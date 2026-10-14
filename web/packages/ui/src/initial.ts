/** A readable letter or digit, optionally with combining marks (not a keycap or emoji form). */
const READABLE = /^[\p{L}\p{N}]\p{M}*$/u;
const EMOJI_FORM = /[\uFE0F\u20E3]/u;

function graphemes(text: string): string[] {
  if (typeof Intl !== 'undefined' && 'Segmenter' in Intl) {
    return Array.from(new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(text), (s) => s.segment);
  }
  return Array.from(text);
}

/**
 * The avatar letter for a display name: the first readable letter or digit, so
 * "🥔猫铃薯🥔" gives "猫"; a name with none falls back to its first whole
 * grapheme (never half a surrogate pair), and an empty one to "?".
 */
export function initial(name: string): string {
  const parts = graphemes(name.trim());
  const letter = parts.find((g) => READABLE.test(g) && !EMOJI_FORM.test(g));
  if (!letter) return parts[0] ?? '?';
  const upper = letter.toUpperCase();
  // "ß" uppercases to "SS"; an avatar holds one letter.
  return graphemes(upper).length === 1 ? upper : letter;
}
