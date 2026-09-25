/**
 * A profile prompt's summary as plain text: the server passes the file's
 * Markdown through, and the table shows words, not `**`, `#` or list marks.
 */
export function plainText(markdown: string): string {
  return plainLines(markdown).replace(/\s+/g, ' ').trim();
}

/** The same, keeping one line per line (the full-prompt viewer). */
export function plainLines(markdown: string): string {
  return markdown
    .split('\n')
    .map((line) =>
      line
        .replace(/^\s{0,3}#{1,6}\s+/, '')
        .replace(/^\s*(?:[-*+]|\d+[.)])\s+/, '')
        .replace(/^\s*>\s?/, '')
        .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
        .replace(/(\*\*|__)(.+?)\1/g, '$2')
        .replace(/(^|[^\w*])[*_]([^*_\s][^*_]*?)[*_](?=[^\w*]|$)/g, '$1$2')
        .replace(/`([^`]*)`/g, '$1')
        .trimEnd(),
    )
    .join('\n')
    .trim();
}

/** A one-line preview of at most `max` characters, cut at a word. */
export function preview(text: string, max = 60): string {
  if (text.length <= max) return text;
  const cut = text.slice(0, max);
  const space = cut.lastIndexOf(' ');
  return `${(space > max / 2 ? cut.slice(0, space) : cut).trimEnd()}…`;
}
