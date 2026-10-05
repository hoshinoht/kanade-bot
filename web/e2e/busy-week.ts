import type { Page } from '@playwright/test';
import type { Boss, Week } from '@kanade/api-types';
import { ADMIN, unconditional } from './support';

/**
 * A busy week: the mock's week plus a 1-, 2- and 3-boss run on every day
 * (`busy-<day>-<count>`), which squeezes populated columns to their minimum.
 */
export async function busyWeek(page: Page) {
  await page.route(`${ADMIN}/api/admin/week*`, async (route) => {
    const response = await route.fetch(unconditional(route));
    const week = (await response.json()) as Week;
    const base = week.runs.find((r) => r.status !== 'done' && r.status !== 'cancelled')!;
    const like = base.bosses[0]!;
    const boss = (key: string, name: string): Boss => ({ ...like, key, name, token: `N${key}`, difficulty: 'n', art: null });
    // The live report: "NCarling NORMAL" over "NMaleficStar NORMAL", the long token last, beside the grip.
    const sets = {
      1: [boss('MaleficStar', 'Malefic Star')],
      2: [boss('Carling', 'Carling'), boss('MaleficStar', 'Malefic Star')],
      3: [boss('Carling', 'Carling'), boss('Baldrix', 'Baldrix'), boss('MaleficStar', 'Malefic Star')],
    };
    const runs = [...week.runs];
    for (const day of week.days) {
      for (const count of [1, 2, 3] as const) {
        runs.push({ ...base, id: `busy-${day.index}-${count}`, short_id: `b${day.index}${count}`, day: day.index, time: '22:00', status: 'planned', bosses: sets[count] });
      }
    }
    await route.fulfill({ response, json: { ...week, runs } });
  });
}
