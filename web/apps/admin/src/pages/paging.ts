export const PAGE_SIZE = 25;

/** One page of an already-filtered list; `page` is clamped so a narrowing search never lands past the end. */
export function paged<T>(rows: T[], page: number, size = PAGE_SIZE): { rows: T[]; page: number; pages: number } {
  const pages = Math.max(1, Math.ceil(rows.length / size));
  const at = Math.min(Math.max(1, page), pages);
  return { rows: rows.slice((at - 1) * size, at * size), page: at, pages };
}

/** The page a pager shows: a bound `page` past the end (a Reload shrank the list) reads as the last one. */
export function clampPage(page: number, pages: number): number {
  return Math.min(Math.max(1, page), Math.max(1, pages));
}

/** "1–15 of 33" for page `at` of `size` rows. */
export function rangeLabel(at: number, size: number, total: number): string {
  return `${Math.min((at - 1) * size + 1, total)}–${Math.min(at * size, total)} of ${total.toLocaleString('en')}`;
}
