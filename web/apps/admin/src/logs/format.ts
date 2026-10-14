/**
 * Log-table formats: guild-local times instead of raw ISO, and compact
 * durations that never wrap.
 */
export interface LogTime {
  text: string;
  /** The full timestamp for the tooltip. */
  title: string;
  /** `datetime` for <time>, when the input was an instant. */
  iso: string | null;
}

const ISO = /^\d{4}-\d{2}-\d{2}T/;

/** "Wed 24 Sep · 00:34" in the guild's zone; text the server already formatted passes through. */
export function logTime(at: string, timeZone: string): LogTime {
  const date = ISO.test(at) ? new Date(at) : null;
  if (!date || Number.isNaN(date.getTime())) return { text: at, title: at, iso: null };
  const p: Record<string, string> = {};
  // en-US parts: "Sep", not en-GB's "Sept"; the order is ours.
  for (const part of new Intl.DateTimeFormat('en-US', {
    timeZone,
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(date))
    p[part.type] = part.value;
  return {
    text: `${p.weekday} ${p.day} ${p.month} · ${p.hour}:${p.minute}`,
    title: `${p.weekday} ${p.day} ${p.month} ${p.year}, ${p.hour}:${p.minute}:${p.second} (${timeZone})`,
    iso: at,
  };
}

/** "545 ms", "4.9 s", "1.2 min"; "—" when nothing ran. */
export function duration(ms: number | null | undefined): string {
  if (!ms) return '—';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`;
  return `${(ms / 60_000).toFixed(1)} min`;
}

/** null is "unknown" (not recorded); 0 is a real "0 ms". */
export function took(ms: number | null): string {
  if (ms === null) return 'unknown';
  return ms === 0 ? '0 ms' : duration(ms);
}

// Opening/closing code fences, with an optional info string (```py).
const FENCE = /```[\w+#.-]*/g;

/** One-line preview of free text for a log cell: fences dropped, newlines and whitespace runs as single spaces. */
export function preview(text: string): string {
  return text.replace(FENCE, ' ').replace(/\s+/g, ' ').trim();
}
