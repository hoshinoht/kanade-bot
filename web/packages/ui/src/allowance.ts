// A chat allowance in words, as the admin Limits and both Account screens print it.

/** An allowance window: "5 min", "6 h", "1 d", "90 s". */
export function windowWords(perS: number): string {
  if (perS % 86_400 === 0) return `${perS / 86_400} d`;
  if (perS % 3600 === 0) return `${perS / 3600} h`;
  if (perS % 60 === 0) return `${perS / 60} min`;
  return `${perS} s`;
}

/**
 * How long until an allowance's oldest counted answer leaves its window, as
 * "resets in …" reads it: "1 d 3 h", "5 h 12 m", "2 m 12 s" or "45 s"
 * (hours and days round up to the minute and hour).
 * `now` is the server's clock in epoch ms, never the browser's.
 * Null when nothing will reset (staff, an empty window, unreadable times);
 * '' once it is due, until the next read brings the new window.
 */
export function resetSpan(resetsAt: string | null | undefined, now: number | null): string | null {
  const at = resetsAt ? Date.parse(resetsAt) : NaN;
  if (Number.isNaN(at) || now === null) return null;
  const s = Math.ceil((at - now) / 1000);
  if (s <= 0) return '';
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} m ${s % 60} s`;
  // Coarser spans round up, so the last unit never reads as already gone.
  const m = Math.ceil(s / 60);
  if (m < 1440) return `${Math.floor(m / 60)} h ${m % 60} m`;
  const h = Math.ceil(s / 3600);
  return `${Math.floor(h / 24)} d ${h % 24} h`;
}
