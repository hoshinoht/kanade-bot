import { createRawSnippet } from 'svelte';
import { render } from 'svelte/server';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { experiments, initExperiments, setExperiments } from '../src/experiments/experiments.svelte';
import LoadingIndicator from '../src/components/LoadingIndicator.svelte';
import PendingLabel from '../src/components/PendingLabel.svelte';
import WavyProgress from '../src/components/WavyProgress.svelte';

const store = new Map<string, string>();
const html = { dataset: {} as Record<string, string> };

beforeEach(() => {
  store.clear();
  html.dataset = {};
  Object.assign(globalThis, {
    localStorage: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
    },
    document: { documentElement: html },
  });
});

afterEach(() => {
  delete (globalThis as { localStorage?: unknown }).localStorage;
  delete (globalThis as { document?: unknown }).document;
  experiments.on = true;
});

const save = createRawSnippet(() => ({ render: () => '<span>Save</span>' }));
const body = (h: string) => h.replace(/<!--[^>]*-->/g, '');

describe('experiments switch', () => {
  it('defaults on and reflects on <html>', () => {
    expect(initExperiments('')).toBe(true);
    expect(html.dataset.experiments).toBe('on');
  });

  it('a query turns it off and is remembered', () => {
    expect(initExperiments('?section=pings&experiments=off')).toBe(false);
    expect(html.dataset.experiments).toBe('off');
    expect(initExperiments('')).toBe(false);
    expect(initExperiments('?experiments=on')).toBe(true);
    expect(store.get('kanade.experiments')).toBe('on');
  });

  it('ignores unknown query values', () => {
    setExperiments(false);
    expect(initExperiments('?experiments=maybe')).toBe(false);
  });
});

describe('PendingLabel', () => {
  it('off: renders the label alone, as before, even while pending', () => {
    experiments.on = false;
    expect(body(render(PendingLabel, { props: { pending: true, label: 'Saving…', children: save } }).body)).toBe('<span>Save</span>');
  });

  it('on but idle: the label alone', () => {
    expect(body(render(PendingLabel, { props: { pending: false, label: 'Saving…', children: save } }).body)).toBe('<span>Save</span>');
  });

  it('on and pending: hides the label from AT and names the indicator', () => {
    const out = render(PendingLabel, { props: { pending: true, label: 'Saving…', children: save } }).body;
    expect(out).toContain('class="xp-pending__label" aria-hidden="true"');
    expect(out).toMatch(/role="status" aria-label="Saving…"/);
    expect(out).toContain('xp-loading--sm');
  });
});

describe('LoadingIndicator', () => {
  it('is a labelled status in the chosen size', () => {
    const out = render(LoadingIndicator, { props: { label: 'Loading calls', size: 'md' } }).body;
    expect(out).toContain('role="status"');
    expect(out).toContain('aria-label="Loading calls"');
    expect(out).toContain('xp-loading--md');
  });
});

describe('WavyProgress', () => {
  it('is a progressbar with its value, range and words', () => {
    const out = render(WavyProgress, { props: { value: 1, max: 3, label: 'Rescan progress', text: '1 of 3 channels read' } }).body;
    expect(out).toContain('role="progressbar"');
    expect(out).toContain('aria-label="Rescan progress"');
    expect(out).toContain('aria-valuemin="0"');
    expect(out).toContain('aria-valuemax="3"');
    expect(out).toContain('aria-valuenow="1"');
    expect(out).toContain('aria-valuetext="1 of 3 channels read"');
    expect(out).toMatch(/<svg[^>]*aria-hidden="true"/);
  });
});
