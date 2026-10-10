import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const read = (p: string) => readFileSync(new URL(p, import.meta.url), 'utf8');

// NapArt.svelte inlines nap.svg without its <style> (blocked by style-src 'self').
describe('nap illustration', () => {
  it('keeps NapArt markup identical to nap.svg minus the style element', () => {
    const svg = read('../src/nap/nap.svg').replace(/<style>[\s\S]*?<\/style>\n/, '').trim();
    const component = read('../src/components/NapArt.svelte');
    expect(component).toContain(svg);
    const svgRules = /<style>([\s\S]*?)<\/style>/.exec(read('../src/nap/nap.svg'))![1]!.replace(/\/\*[\s\S]*?\*\/\n/, '').trim();
    expect(/^<style>\n([\s\S]*?)<\/style>/m.exec(component)![1]!.trim()).toBe(svgRules);
  });

  it('never inlines a style attribute', () => {
    expect(read('../src/components/NapArt.svelte')).not.toMatch(/\sstyle=/);
  });
});
