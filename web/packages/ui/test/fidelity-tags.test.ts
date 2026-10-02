import { describe, expect, it } from 'vitest';
import { stripFidelityTags } from '../src/vite';

type Hook = (code: string, id: string) => { code: string } | null;
const run = (keep: boolean, code: string, id = '/w/apps/admin/src/inbox/InboxDetail.svelte') =>
  (stripFidelityTags(keep).transform as unknown as Hook)(code, id)?.code ?? code;

describe('stripFidelityTags', () => {
  const source = '<aside class="decision" data-fid="decision">\n  <button\n    data-fid="decision-reject" onclick={reject}>Reject…</button>\n</aside>';

  it('removes every literal tag from Svelte sources', () => {
    expect(run(false, source)).toBe('<aside class="decision">\n  <button onclick={reject}>Reject…</button>\n</aside>');
  });

  it('keeps them for a KANADE_FIDELITY build', () => {
    expect(run(true, source)).toBe(source);
  });

  it('leaves other files and other data attributes alone', () => {
    expect(run(false, 'const s = \'data-fid="x"\';', '/w/src/a.ts')).toBe('const s = \'data-fid="x"\';');
    expect(run(false, '<div data-history={seq}></div>')).toBe('<div data-history={seq}></div>');
  });
});
