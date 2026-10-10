import { describe, expect, it } from 'vitest';
import { discordStart, LOGIN_ERRORS, loginErrorText, loginHref, safeNext } from '../src/auth';

describe('sign-in paths', () => {
  it('returns only to same-origin pages, as the server does', () => {
    for (const good of ['/', '/week?week=next', '/inbox?tab=self_service&item=p1', '/runs/r1#sheet']) expect(safeNext(good)).toBe(good);
    for (const bad of [null, '', 'https://evil.example/', '//evil.example/', '/api/admin/week', '/\\evil', 'relative', '/with space', '/login?next=/x']) {
      expect(safeNext(bad)).toBe('/');
    }
  });

  it('builds the sign-in and Discord start links with next', () => {
    expect(loginHref('/fixed?x=1')).toBe('/login?next=%2Ffixed%3Fx%3D1');
    expect(loginHref('/')).toBe('/login');
    expect(discordStart('//evil')).toBe('/api/admin/auth/discord/start?next=%2F');
  });

  it('words every login_error code, and an unknown one', () => {
    for (const code of ['state', 'denied', 'forbidden', 'discord', 'unavailable', 'rate_limited']) expect(LOGIN_ERRORS[code]).toBeTruthy();
    expect(loginErrorText('forbidden')).toContain("isn't recognised as a Kanade admin");
    expect(loginErrorText('other')).toBe('Sign-in did not complete. Try again.');
    expect(loginErrorText('')).toBe('');
  });
});
