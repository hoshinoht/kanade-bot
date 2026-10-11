import { describe, expect, it } from 'vitest';
import { ALL_SECTIONS, CAPABILITIES, enabledSections, MORE, ROUTES, SECTIONS, sumBadges } from '../src/routes';

describe('capabilities', () => {
  it('leave Drafts out everywhere while its page has not landed', () => {
    expect(CAPABILITIES.drafts).toBe(false);
    expect(SECTIONS.map((s) => s.key)).not.toContain('drafts');
    expect(ROUTES.map((r) => r.pattern)).not.toContain('/drafts');
  });

  it('put Drafts in Schedule after Bosses once on', () => {
    const on = enabledSections(ALL_SECTIONS, { ...CAPABILITIES, drafts: true });
    expect(on.filter((s) => s.group === 'Schedule').map((s) => s.label)).toEqual(['Week', 'Fixed', 'Bosses', 'Drafts']);
    expect(on.find((s) => s.key === 'drafts')).toMatchObject({ href: '/drafts', icon: 'layers' });
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
