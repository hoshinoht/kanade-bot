/**
 * The bit of Discord markdown the bot's cards use: `**bold**` runs and line
 * breaks. Anything else stays literal text (mentions are left in for
 * `Mentions` to name).
 */
export type Run = { bold: boolean; text: string };

/** A run's leading whitespace apart from the rest (`Mentions` trims its start). */
export function lead(text: string): [string, string] {
  const space = /^\s*/.exec(text)![0];
  return [space, text.slice(space.length)];
}

/** One card text as lines of bold/plain runs; an unclosed `**` stays literal. */
export function cardLines(text: string): Run[][] {
  return text.split('\n').map((line) => {
    const parts = line.split('**');
    if (parts.length % 2 === 0) return [{ bold: false, text: line }];
    return parts.map((part, i) => ({ bold: i % 2 === 1, text: part })).filter((run) => run.text !== '');
  });
}
