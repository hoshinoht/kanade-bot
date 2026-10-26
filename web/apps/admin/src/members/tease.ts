// The running-joke note's refusals (D10) in the Members sheet's words. The
// server answers only the rule (`note_too_long`, `note_not_plain`,
// `note_names_member`); the sheet names the offending word when it knows it
// (the roster's names and aliases, this member's IGNs), with the same
// whole-word match as the server (`crates/kanade-domain/src/tease.rs`).

export const NOTE_MAX = 80;

export type NameKind = 'IGN' | 'name' | 'alias';

export interface KnownName {
  text: string;
  kind: NameKind;
}

// An ASCII letter or digit right before or after a name makes it part of another word ("ali" in "alibi").
const WORD = /[A-Za-z0-9]/;

/** The first known name the note carries as a whole word, case-insensitively. */
export function namedIn(note: string, names: readonly KnownName[]): { name: KnownName; at: number } | null {
  const text = note.toLowerCase();
  for (const name of names) {
    const needle = name.text.trim().toLowerCase();
    if (!needle) continue;
    for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + 1)) {
      if (!WORD.test(text[at - 1] ?? '') && !WORD.test(text[at + needle.length] ?? '')) return { name, at };
    }
  }
  return null;
}

/** What the field says under a refused note. */
export function refusalWords(code: string, message: string, note: string, names: readonly KnownName[]): string {
  if (code === 'note_too_long') return `Not saved: keep the note to ${NOTE_MAX} characters.`;
  if (code === 'note_not_plain') return 'Not saved: plain text only, with no mentions, @ or # handles, links or formatting.';
  if (code !== 'note_names_member') return `Not saved: ${message}`;
  const trimmed = note.trim();
  const found = namedIn(trimmed, names);
  if (!found) return 'Not saved: it names someone on the roster (a name, alias or IGN). A note never names anyone.';
  const end = found.at + found.name.text.trim().length;
  // The note without the name (and a possessive or punctuation left at its edges), as a suggestion.
  const rest = `${trimmed.slice(0, found.at)} ${trimmed.slice(end)}`
    .replace(/\s+/g, ' ')
    .replace(/^\s*['’]s\b/i, '')
    .replace(/^[\s,.:;'’-]+|[\s,:;-]+$/g, '');
  const what = found.name.kind === 'alias' ? 'an alias' : `a roster ${found.name.kind}`;
  return `Not saved: “${trimmed.slice(found.at, end)}” is ${what}. A note never names anyone${rest ? `; try “${rest}”` : ''}.`;
}
