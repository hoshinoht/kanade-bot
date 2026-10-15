import { afterEach, describe, expect, it, vi } from 'vitest';
import type { FixedRow } from '@kanade/api-types';
import { runsToChoose } from '../src/fixed/choices';

// f-kalos: Tue 22:00. r-kalos was moved this week; n-kalos changed only its roster.
const run = (run_id: string, time: string) => ({ run_id, short_id: run_id, week: 'this', day: 0, time, status: 'planned', amended: true });
const row = { id: 'f-kalos', weekday: 1, time: '22:00', runs: [run('r-kalos', '23:00'), run('n-kalos', '22:00')] } as unknown as FixedRow;

function server(body: unknown, status = 200) {
  const fetch = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify(body), { status }));
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

describe('runsToChoose', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('asks nothing, and asks no server, when the save keeps the day and time', async () => {
    const fetch = server({ runs: [] });
    // Amended runs exist, but a note or party edit moves none of them.
    expect(await runsToChoose(row, 1, ' 22:00 ')).toEqual({ ok: true, value: [] });
    expect(fetch).not.toHaveBeenCalled();
  });

  it("asks about exactly the runs the server's save would list", async () => {
    const fetch = server({ runs: [row.runs[0]] });
    const result = await runsToChoose(row, 1, '21:30');
    expect(result).toEqual({ ok: true, value: [row.runs[0]] });
    const [url, init] = fetch.mock.calls[0]!;
    expect(String(url)).toContain('/api/admin/fixed/f-kalos/choices');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(String(init?.body))).toEqual({ weekday: 1, time: '21:30' });
  });

  it('asks on a day change alone, and passes a refusal through', async () => {
    server({ error: 'not_found', message: 'That weekly timing no longer exists.' }, 404);
    expect(await runsToChoose(row, 2, '22:00')).toMatchObject({ ok: false, code: 'not_found', message: 'That weekly timing no longer exists.' });
  });
});
