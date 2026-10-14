import { preview } from '../logs/format';

/** What the log keeps of a withheld turn's tool text. */
export const WITHHELD = '[message withheld]';

/** A trace cell shows one line; a short name keeps screen readers from reading up to 8 KiB. */
const PREVIEW = 120;

export function short(text: string): string {
  const line = preview(text);
  return line.length > PREVIEW ? `${line.slice(0, PREVIEW)}…` : line;
}
