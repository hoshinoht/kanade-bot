import { describe, expect, it } from 'vitest';
import { preview } from '../src/logs/format';

describe('preview', () => {
  it('collapses newlines and whitespace runs into single spaces', () => {
    expect(preview('  two sum\n\n\tgiven   nums\r\nreturn  ')).toBe('two sum given nums return');
  });

  it('drops code fences with or without a language', () => {
    expect(preview('solve:\n```py\ndef f(x):\n    return x\n```\nthanks')).toBe('solve: def f(x): return x thanks');
    expect(preview('```\nplain\n```')).toBe('plain');
  });

  it('keeps mention tokens and inline code intact', () => {
    expect(preview('<@1543532497948909578>\nis <#fa-night> `up`?')).toBe('<@1543532497948909578> is <#fa-night> `up`?');
  });

  it('is empty for whitespace-only text', () => {
    expect(preview(' \n\t ')).toBe('');
  });
});
