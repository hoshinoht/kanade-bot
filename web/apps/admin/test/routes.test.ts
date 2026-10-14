import { describe, expect, it } from 'vitest';
import { ALL_SECTIONS, CAPABILITIES, enabledSections, MORE, ROUTES, SECTIONS, sumBadges } from '../src/routes';

describe('capabilities', () => {
  it('put Drafts in Schedule after Bosses, routed, now that its page has landed', () => {
    expect(CAPABILITIES.drafts).toBe(true);
    expect(SECTIONS.filter((s) => s.group === 'Schedule').map((s) => s.label)).toEqual(['Week', 'Fixed', 'Bosses', 'Drafts']);
    expect(SECTIONS.find((s) => s.key === 'drafts')).toMatchObject({ href: '/drafts', icon: 'layers' });
    expect(ROUTES.map((r) => r.pattern)).toContain('/drafts');
  });

  it('leave a destination out everywhere while its capability is off', () => {
    const off = enabledSections(ALL_SECTIONS, { ...CAPABILITIES, drafts: false });
    expect(off.map((s) => s.key)).not.toContain('drafts');
  });
});

describe('More', () => {
  it('holds Rewrites, Limits and Reminders in nav order', () => {
    expect(MORE.map((s) => s.label)).toEqual(['Rewrites', 'Limits', 'Reminders']);
  });

  it("sums its destinations' badges and nobody else's", () => {
    expect(sumBadges(MORE, { inbox: 9 })).toBe(0);
    expect(sumBadges(MORE, { inbox: 9, limits: 2, reminders: 3 })).toBe(5);
  });
});
