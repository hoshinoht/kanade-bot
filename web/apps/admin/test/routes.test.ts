import { describe, expect, it } from 'vitest';
import { forModules, ROUTES, routesFor, SECTIONS, sumBadges } from '../src/routes';

/** Behind the rail's More item, in nav order (as `NavList` derives it). */
const MORE = SECTIONS.filter((s) => s.more);

describe('Schedule', () => {
  it('holds Week, Fixed and Bosses, and Drafts is gone', () => {
    expect(SECTIONS.filter((s) => s.group === 'Schedule').map((s) => s.label)).toEqual(['Week', 'Fixed', 'Bosses']);
    expect(ROUTES.map((r) => r.pattern)).not.toContain('/drafts');
  });
});

describe('More', () => {
  it('holds Rewrites, Limits, Reminders, Guides and Setup in nav order', () => {
    expect(MORE.map((s) => s.label)).toEqual(['Rewrites', 'Limits', 'Reminders', 'Guides', 'Setup']);
    expect(MORE.find((s) => s.key === 'guides')).toMatchObject({ href: '/guides', icon: 'book-open', module: 'guides' });
  });

  it('drops Guides from the rail, the palette and the router while the module is off', () => {
    const off = forModules(SECTIONS, { guides: false });
    expect(off.map((s) => s.key)).not.toContain('guides');
    expect(routesFor(off).map((r) => r.pattern)).not.toContain('/guides');
    expect(routesFor(forModules(SECTIONS, { guides: true })).map((r) => r.pattern)).toContain('/guides');
  });

  it("sums its destinations' badges and nobody else's", () => {
    expect(sumBadges(MORE, { inbox: 9 })).toBe(0);
    expect(sumBadges(MORE, { inbox: 9, limits: 2, reminders: 3 })).toBe(5);
    expect(sumBadges(MORE, { inbox: 9, setup: 4 })).toBe(4);
  });
});
