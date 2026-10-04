import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';
import AnswerBar from '../src/components/AnswerBar.svelte';
import WavyProgress from '../src/components/WavyProgress.svelte';
import { answerCounts, answerWords } from '../src/answers';

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
    expect(out).not.toContain('wavy--flat');
  });

  it('has a flat, warning-toned mode without changing its semantics', () => {
    const out = render(WavyProgress, { props: { value: 2, max: 10, label: 'Time left', wavy: false, tone: 'warn', class: 'wavy--inline' } }).body;
    expect(out).toContain('wavy--flat');
    expect(out).toContain('wavy--warn');
    expect(out).toContain('wavy--inline');
    expect(out).toContain('role="progressbar"');
  });
});

describe('answers bar', () => {
  const party = (answers: string[]) => answers.map((answer) => ({ answer }));

  it('counts each answer, unknown ones as waiting', () => {
    expect(answerCounts(party(['yes', 'yes', 'maybe', 'waiting', 'no', 'odd']))).toEqual({ yes: 2, maybe: 1, waiting: 2, no: 1, total: 6 });
  });

  it('says every count in words, leaving out empty ones', () => {
    expect(answerWords(answerCounts(party(['yes', 'yes', 'waiting'])))).toBe('2 on, 1 waiting of 3');
    expect(answerWords(answerCounts(party(['no'])))).toBe('1 out of 1');
  });

  it('renders yes, maybe and waiting segments with its counts as its name', () => {
    const out = render(AnswerBar, { props: { participants: party(['yes', 'maybe', 'waiting', 'no']) } }).body;
    expect(out).toContain('role="img"');
    expect(out).toContain('aria-label="Answers: 1 on, 1 maybe, 1 waiting, 1 out of 4"');
    const order = [...out.matchAll(/answerbar__(seg--\w+|rest)/g)].map((m) => m[1]);
    expect(order).toEqual(['seg--yes', 'seg--maybe', 'seg--waiting', 'rest']);
  });

  it('draws nothing for an empty party', () => {
    expect(render(AnswerBar, { props: { participants: [] } }).body).not.toContain('answerbar');
  });
});
