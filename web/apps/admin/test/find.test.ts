import { describe, expect, it } from 'vitest';
import { score } from '../src/config/find';

describe('find a setting: score', () => {
  it('ranks a title phrase over title words over body text', () => {
    expect(score('countdowns', 'Countdowns', 'Countdowns minutes before start')).toBe(4);
    expect(score('quiet mode', 'Quiet  mode', '')).toBe(4);
    expect(score('mode quiet', 'Quiet mode', '')).toBe(3);
    expect(score('before start', 'Countdowns', 'Countdowns minutes before start')).toBe(2);
    expect(score('start minutes', 'Countdowns', 'Countdowns minutes before start')).toBe(1);
  });

  it('ignores case and spacing, and matches nothing for a blank or absent query', () => {
    expect(score('  MANAGE   messages ', '', 'Manage Messages missing')).toBe(2);
    expect(score('   ', 'Anything', 'anything')).toBe(0);
    expect(score('zzz', 'Pings', 'Morning ping')).toBe(0);
  });
});
