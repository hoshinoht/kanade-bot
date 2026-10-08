import { describe, expect, it } from 'vitest';
import { discordStart, landingOf, safeNext, withoutLoginError } from '../src/landing';

describe('sign-in landing', () => {
  it('maps the callback codes: not_eligible is Denied, closed is Closed, the rest a generic notice', () => {
    expect(landingOf('')).toBe('none');
    expect(landingOf('?login_error=not_eligible')).toBe('denied');
    expect(landingOf('?login_error=closed')).toBe('closed');
    for (const code of ['state', 'denied', 'discord', 'unavailable', 'rate_limited', 'made-up']) {
      expect(landingOf(`?sw=off&login_error=${code}`)).toBe('failed');
    }
  });

  it('starts Discord with a same-origin next only', () => {
    expect(discordStart('/account?tab=1')).toBe('/api/public/auth/discord/start?next=%2Faccount%3Ftab%3D1');
    for (const bad of ['//evil.example/', '/\\evil', '/api/public/session', 'https://evil.example/', '', '/a b']) {
      expect(safeNext(bad), bad).toBe('/');
    }
  });

  it('drops only login_error from the address', () => {
    expect(withoutLoginError('/', '?sw=off&login_error=state', '#x')).toBe('/?sw=off#x');
    expect(withoutLoginError('/', '?login_error=not_eligible', '')).toBe('/');
  });
});
