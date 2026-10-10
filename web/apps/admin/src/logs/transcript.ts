/**
 * Shared pieces of the logs' "Copy transcript" (Chat, Extractions, Rewrites):
 * the format choice, fences that stray backticks cannot close, mention
 * tokens as names, and counts that may be unknown.
 */
import { directory } from '../names/directory.svelte';
import { parseMentions } from '../names/mentions';

export interface TranscriptContext {
  timeZone: string;
}

export type TranscriptFormat = 'markdown' | 'json';

export const FORMAT_LABEL: Record<TranscriptFormat, string> = { markdown: 'Markdown', json: 'JSON' };

/** Message text with mention tokens as @Name / #channel / @role. */
export function mentionsText(text: string): string {
  return parseMentions(text)
    .map((s) => (s.kind === 'text' ? s.text : directory.label(s.kind, s.id, '', true)))
    .join('');
}

export const fence = (text: string, lang = '') => {
  // A fence longer than any run of backticks inside, so the text cannot close it.
  const ticks = '`'.repeat(Math.max(3, ...[...text.matchAll(/`+/g)].map((m) => m[0].length + 1)));
  return `${ticks}${lang}\n${text}\n${ticks}`;
};

/** A reported count in words: "1,820"; null (not reported) is "unknown", never 0. */
export const count = (n: number | null | undefined): string => (n == null ? 'unknown' : n.toLocaleString('en'));

/** The JSON transcript: two-space indent, one trailing newline. */
export const jsonDocument = (value: unknown): string => `${JSON.stringify(value, null, 2)}\n`;
