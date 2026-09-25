export const PAGE_SIZE = 25;

/** One page of an already-filtered list; `page` is clamped so a narrowing search never lands past the end. */
export function paged<T>(rows: T[], page: number, size = PAGE_SIZE): { rows: T[]; page: number; pages: number } {
  const pages = Math.max(1, Math.ceil(rows.length / size));
  const at = Math.min(Math.max(1, page), pages);
  return { rows: rows.slice((at - 1) * size, at * size), page: at, pages };
}
