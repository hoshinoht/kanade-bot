import { describe, expect, it } from 'vitest';
import { cardText, stamps } from '../src/reminders/discord';

const NOW = Date.parse('2026-09-09T01:00:00Z');
const AT = Date.parse('2026-09-09T13:30:00Z') / 1000;

describe('Discord timestamps in the card preview', () => {
  it('draws clock, relative and full stamps', () => {
    expect(stamps(`<t:${AT}:t>`, NOW, 'Asia/Kuala_Lumpur')).toBe('21:30');
    expect(stamps(`<t:${AT}:R>`, NOW)).toBe('in 12 hours');
    expect(stamps(`<t:${NOW / 1000 - 300}:R>`, NOW)).toBe('5 minutes ago');
    expect(stamps(`<t:${AT}:F>`, NOW, 'Asia/Kuala_Lumpur')).toContain('9 September 2026');
    expect(stamps('no stamps <t:x:t>', NOW)).toBe('no stamps <t:x:t>');
  });

  it('marks subtext lines and keeps bold runs', () => {
    expect(cardText(`📅 **Today**\n-# Waiting on <@1> · react`, NOW)).toEqual([
      { sub: false, runs: [{ bold: false, text: '📅 ' }, { bold: true, text: 'Today' }] },
      { sub: true, runs: [{ bold: false, text: 'Waiting on <@1> · react' }] },
    ]);
  });
});
