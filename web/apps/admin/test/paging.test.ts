import { describe, expect, it } from 'vitest';
import { clampPage, paged, rangeLabel } from '../src/pages/paging';

describe('paged', () => {
  const rows = Array.from({ length: 60 }, (_, i) => i);

  it('slices pages and counts them', () => {
    expect(paged(rows, 1).rows).toEqual(rows.slice(0, 25));
    expect(paged(rows, 3)).toEqual({ rows: rows.slice(50), page: 3, pages: 3 });
  });

  it('clamps out-of-range pages, e.g. after a search narrows the list', () => {
    expect(paged(rows.slice(0, 5), 3)).toEqual({ rows: rows.slice(0, 5), page: 1, pages: 1 });
    expect(paged([], 2)).toEqual({ rows: [], page: 1, pages: 1 });
  });

  it('shows a page past the end as the last one, e.g. after a Reload shrinks the list', () => {
    expect(clampPage(3, 2)).toBe(2);
    expect(clampPage(0, 2)).toBe(1);
    expect(clampPage(2, 0)).toBe(1);
    expect(rangeLabel(clampPage(3, 2), 10, 15)).toBe('11–15 of 15');
    expect(rangeLabel(1, 10, 1234)).toBe('1–10 of 1,234');
  });
});
