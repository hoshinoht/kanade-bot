import { describe, expect, it } from 'vitest';
import { initial } from '../src/initial';

describe('initial', () => {
  it.each([
    ['🥔猫铃薯🥔', '猫'],
    ['kanade', 'K'],
    ['✨Mafu', 'M'],
    ['  ~*luna*~', 'L'],
    ['🥔🥔', '🥔'],
    ['👨‍👩‍👧 family', 'F'],
    ['👨‍👩‍👧', '👨‍👩‍👧'],
    ['', '?'],
    ['   ', '?'],
    ['Ｚeta', 'Ｚ'],
    ['élan', 'É'],
    ['e\u0301lan', 'E\u0301'],
    ['\u0301abc', 'A'],
    ['1️⃣Rin', 'R'],
    ['42', '4'],
    ['ßtraße', 'ß'],
  ])('%j -> %j', (name, want) => {
    expect(initial(name)).toBe(want);
  });

  it('never returns a lone surrogate', () => {
    expect(initial('🥔')).toBe('🥔');
    expect(initial('🥔').length).toBe(2);
  });
});
