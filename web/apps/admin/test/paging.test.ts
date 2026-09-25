import { describe, expect, it } from 'vitest';
import { paged } from '../src/pages/paging';

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
});
