import { describe, expect, it } from 'vitest';
import { dayTime, deviceName, isHandheld, seenWords } from '../src/account';

describe('session words', () => {
  it('says last seen against the server clock, never the device', () => {
    const now = '2026-09-29T04:00:00Z';
    expect(seenWords('2026-09-29T03:59:30Z', now)).toBe('now');
    expect(seenWords('2026-09-29T03:20:00Z', now)).toBe('40 min ago');
    expect(seenWords('2026-09-29T01:00:00Z', now)).toBe('3 h ago');
    expect(seenWords('2026-09-27T01:00:00Z', now)).toBe('2 d ago');
    expect(seenWords('bad', now)).toBe('');
  });

  it('prints times in the given zone', () => {
    expect(dayTime('2026-10-02T13:14:00Z', 'Asia/Kuala_Lumpur')).toBe('2 Oct 21:14');
    expect(dayTime('2026-10-02T13:14:00Z', 'UTC')).toBe('2 Oct 13:14');
  });

  it('names the device and picks its glyph', () => {
    expect(deviceName({ device: 'Safari · iPhone' })).toBe('Safari · iPhone');
    expect(deviceName({ device: null })).toBe('Unknown browser');
    expect(isHandheld({ device: 'Safari · iPhone' })).toBe(true);
    expect(isHandheld({ device: 'Chrome · Android' })).toBe(true);
    expect(isHandheld({ device: 'Firefox · macOS' })).toBe(false);
  });
});
