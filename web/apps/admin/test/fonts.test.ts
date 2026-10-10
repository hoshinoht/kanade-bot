import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

describe('code font', () => {
  it('is Maple Mono, self-hosted at an exact pin', () => {
    expect(read('../../../packages/tokens/src/_tokens.scss')).toMatch(/--mono: "Maple Mono",/);
    const fonts = read('../../../packages/tokens/src/fonts.css');
    for (const w of [400, 500, 600]) expect(fonts).toContain(`@fontsource/maple-mono/latin-${w}.css`);
    expect(fonts).not.toMatch(/@import[^;]*sometype/);
    const deps = JSON.parse(read('../../../packages/tokens/package.json')).dependencies;
    expect(deps['@fontsource/maple-mono']).toBe('5.3.0');
    expect(deps['@fontsource/sometype-mono']).toBeUndefined();
  });
});
