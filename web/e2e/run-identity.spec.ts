import type { Locator, Page } from '@playwright/test';
import type { Week } from '@kanade/api-types';
import { ADMIN, REAL_ART, expect, settle, test } from './support';

// The run pane's and the full sheet's identity card: entry art confined to the
// card (one picture, or up to three angled slices in run order), the "View
// weekly timing" link, and the Fixed editor's time stepper. With
// KANADE_REAL_ART=1 the capture test writes e2e/.captures/real/ for review.

const OUT = REAL_ART ? 'e2e/.captures/real' : 'e2e/.captures/synthetic';

/** r-carling (HCarling + HStar) with Kalos and Black Mage added: four bosses, all with art. */
async function fourBosses(page: Page) {
  await page.route(`${ADMIN}/api/admin/week*`, async (route) => {
    const response = await route.fetch();
    const week = (await response.json()) as Week;
    const pick = (id: string) => week.runs.find((r) => r.id === id)?.bosses ?? [];
    const runs = week.runs.map((r) => (r.id === 'r-carling' ? { ...r, bosses: [...r.bosses, ...pick('r-kalos'), ...pick('r-bm')] } : r));
    await route.fulfill({ response, json: { ...week, runs } });
  });
}

/** Every art slice lies inside the identity card, and the card ends above the Move picker. */
async function confined(card: Locator) {
  const result = await card.evaluate((el) => {
    const c = el.getBoundingClientRect();
    const group = el.querySelector('.run__arts');
    const g = group?.getBoundingClientRect();
    return {
      inside: g ? g.left >= c.left - 0.5 && g.top >= c.top - 0.5 && g.right <= c.right + 0.5 && g.bottom <= c.bottom + 0.5 : false,
      clip: getComputedStyle(el).overflow,
      strays: [...document.querySelectorAll('img.run__art')].filter((img) => !el.contains(img)).length,
    };
  });
  expect(result).toEqual({ inside: true, clip: 'clip', strays: 0 });
}

async function srcs(card: Locator) {
  return card.locator('.run__arts img.run__art').evaluateAll((imgs) => imgs.map((i) => i.getAttribute('src')));
}

async function openPane(page: Page, id: string, name: string) {
  await page.locator(`[data-run="${id}"] .plan-card__open`).click();
  const pane = page.getByRole('complementary', { name });
  await expect(pane).toBeVisible();
  return pane;
}

test('run pane: one picture, two slices, and three slices for four bosses, all inside the identity card', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  let pane = await openPane(page, 'r-bm', 'XBM');
  let card = pane.locator('.week-pane__art');
  await expect(card.locator('.run__arts--1 .run__slice')).toHaveCount(1);
  await confined(card);
  await expect(card.locator('.run__arts')).toHaveCSS('opacity', '0.38');
  await expect(card.locator('img.run__art')).toHaveAttribute('alt', '');
  // The card stops above the Move picker: the art can no longer run under it.
  const [cardBottom, pickTop] = await Promise.all([
    card.evaluate((el) => el.getBoundingClientRect().bottom),
    pane.locator('.movepick').evaluate((el) => el.getBoundingClientRect().top),
  ]);
  expect(cardBottom).toBeLessThanOrEqual(pickTop + 0.5);
  await page.keyboard.press('Escape');

  pane = await openPane(page, 'r-carling', 'HCarling + HStar');
  card = pane.locator('.week-pane__art');
  await expect(card.locator('.run__arts--2 .run__slice')).toHaveCount(2);
  expect(await srcs(card)).toEqual(['/art/entry/Carling', '/art/entry/MaleficStar']);
  await confined(card);
  await page.keyboard.press('Escape');

  await page.unrouteAll();
  await fourBosses(page);
  await page.reload();
  pane = await openPane(page, 'r-carling', 'HCarling + HStar + XKalos + XBM');
  card = pane.locator('.week-pane__art');
  // At most three slices, lead first in run order; the fourth boss is a portrait only.
  await expect(card.locator('.run__arts--3 .run__slice')).toHaveCount(3);
  expect(await srcs(card)).toEqual(['/art/entry/Carling', '/art/entry/MaleficStar', '/art/entry/Kalos']);
  await expect(card.locator('.week-pane__bosses li')).toHaveCount(4);
  await confined(card);

  // The larger view keeps the same art inside its identity card.
  await pane.getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar + XKalos + XBM' });
  const hero = sheet.locator('.runsheet__hero');
  await expect(hero.locator('.run__arts--3 .run__slice')).toHaveCount(3);
  await confined(hero);
});

// The modal is the one window (HeroSheet's single `.win`): one title bar with
// dots, no window or card frame inside it, and the identity card flush on its surface.
for (const vp of [
  { name: 'wide', width: 1280, height: 800 },
  { name: 'phone', width: 390, height: 844 },
]) {
  test(`full run sheet is one window (${vp.name})`, async ({ page }) => {
    await page.setViewportSize({ width: vp.width, height: vp.height });
    await page.goto(`${ADMIN}/?sw=off`);
    await page.locator('[data-run="r-carling"] .plan-card__open').click();
    if (vp.name === 'wide') await page.getByRole('complementary', { name: 'HCarling + HStar' }).getByRole('button', { name: 'Open in a larger view' }).click();
    const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
    await expect(sheet.getByRole('tab', { name: /^Party/ })).toBeVisible();
    const found = await sheet.evaluate((dialog) => {
      const hero = dialog.querySelector('.runsheet__hero')!;
      const cs = getComputedStyle(hero);
      const body = getComputedStyle(dialog.querySelector('.modal__body')!).backgroundColor;
      return {
        bars: dialog.querySelectorAll('.modal__head, .card__head').length,
        cards: dialog.querySelectorAll('.card').length,
        frame: [cs.borderTopWidth, cs.borderLeftWidth, cs.borderRightWidth, cs.borderTopLeftRadius, cs.boxShadow],
        flush: cs.backgroundColor === body,
      };
    });
    expect(found).toEqual({ bars: 1, cards: 0, frame: ['0px', '0px', '0px', '0px', 'none'], flush: true });
    // Only the selected panel scrolls; the dialog's own body does not.
    const scrolls = await sheet.evaluate((dialog) => {
      const body = dialog.querySelector('.modal__body')!;
      return body.scrollHeight - body.clientHeight;
    });
    expect(scrolls).toBeLessThanOrEqual(1);
  });
}

test('run with no art writes no art layer', async ({ page }) => {
  test.skip(REAL_ART, 'fixture-specific: Jupiter has no art in the fixtures');
  await page.goto(`${ADMIN}/?sw=off`);
  const pane = await openPane(page, 'r-jupiter', 'HJupiter');
  await expect(pane.locator('.run__arts')).toHaveCount(0);
});

test('the identity art never hides under reduced motion and stays CSP-clean on the phone sheet', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  const hero = sheet.locator('.runsheet__hero');
  await expect(hero.locator('.run__arts--2 .run__slice')).toHaveCount(2);
  await confined(hero);
  // No style attribute anywhere in the art (strict CSP: classes only).
  expect(await hero.locator('.run__arts, .run__arts *').evaluateAll((els) => els.filter((e) => e.hasAttribute('style')).length)).toBe(0);
});

test('"View weekly timing" opens the timing in the Fixed editor without a reload', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.evaluate(() => ((window as unknown as { __same: boolean }).__same = true));
  const pane = await openPane(page, 'r-carling', 'HCarling + HStar');
  const link = pane.getByRole('link', { name: 'View weekly timing' });
  await expect(link).toHaveAttribute('href', '/fixed?open=f-carling');
  await link.click();
  await expect(page.getByRole('complementary', { name: 'Weekly timing details' })).toBeVisible();
  await expect(page).toHaveURL(/\/fixed$/);
  await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __same?: boolean }).__same)).toBe(true);

  // A run that came from no weekly timing has no link.
  await page.goBack();
  const own = await openPane(page, 'r-bellona', 'NBellona');
  await expect(own.getByRole('link', { name: 'View weekly timing' })).toHaveCount(0);
});

test('"View weekly timing" from the phone sheet closes the sheet and opens the editor', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-bm"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'XBM', exact: true });
  await sheet.getByRole('link', { name: 'View weekly timing' }).click();
  await expect(sheet).toBeHidden();
  const editor = page.getByRole('dialog', { name: /XBM/ });
  await expect(editor).toBeVisible();
  await expect(editor.getByRole('spinbutton', { name: 'Time' })).toBeVisible();
});

test('fixed editor: the time is a stepper that still takes typing, and Enter saves', async ({ page }) => {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  const time = editor.getByRole('spinbutton', { name: 'Time' });
  await expect(time).toHaveValue('23:30');
  // The caption is a label: a click on it focuses the typed field, whose only focus mark is the pill's ring.
  await editor.locator('.fixedsheet__time label').click();
  await expect(time).toBeFocused();
  await expect(time).toHaveCSS('box-shadow', 'none');
  await expect(editor.locator('.fixedsheet__time .timestep')).not.toHaveCSS('box-shadow', 'none');
  // The step is Config → Run lengths (the mock's default, as the Move picker shows it).
  const step = Number((await editor.getByRole('button', { name: /minutes later$/ }).getAttribute('aria-label'))!.split(' ')[0]);
  await time.press('ArrowDown');
  const down = 23 * 60 + 30 - step;
  const hhmm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
  await expect(time).toHaveValue(hhmm(down));
  await expect(time).toHaveAttribute('aria-valuetext', hhmm(down));
  await editor.getByRole('button', { name: `${step} minutes later` }).click();
  await expect(time).toHaveValue('23:30');
  // Typed entry stays as typed; Enter submits the form as before.
  await time.fill('22:45');
  await expect(time).toHaveAttribute('aria-valuetext', '22:45');
  await time.press('Enter');
  await expect(editor).toBeHidden();
  await expect(page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Saved Tuesday 22:45 — XBM.' })).toBeVisible();
});

test('fixed editor: a new timing starts empty, steps from 21:00, and an empty time is still refused by the server', async ({ page }) => {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: 'Add a weekly timing' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  const time = editor.getByRole('spinbutton', { name: 'Time' });
  await expect(time).toHaveValue('');
  await expect(time).toHaveAttribute('placeholder', '21:30');
  await time.press('ArrowUp');
  await expect(time).toHaveValue('21:00');
  await time.fill('');
  await editor.getByRole('button', { name: 'Add timing' }).click();
  await expect(editor.getByRole('alert').filter({ hasText: /\S/ }).first()).toBeVisible();
});

const LOOKS = [
  { name: 'blossom-light', colorway: 'blossom', theme: 'light' },
  { name: 'twilight-dark', colorway: 'twilight', theme: 'dark' },
];
const VIEWPORTS = [
  { name: 'wide', width: 1280, height: 800 },
  { name: 'narrow', width: 390, height: 844 },
];

test.describe('captures', () => {
  test.describe.configure({ mode: 'parallel' });
  for (const vp of VIEWPORTS) {
    for (const look of LOOKS) {
      test(`capture run identity ${vp.name} ${look.name}`, async ({ page }) => {
        await page.setViewportSize({ width: vp.width, height: vp.height });
        await page.addInitScript(
          ([c, t]) => {
            localStorage.setItem('colorway', c!);
            localStorage.setItem('theme', t!);
          },
          [look.colorway, look.theme],
        );
        const tag = `${vp.name}-${look.name}`;
        const shot = async (name: string) => {
          await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode().catch(() => {}))));
          await settle(page);
          await page.screenshot({ path: `${OUT}/run-identity-${name}-${tag}.png`, animations: 'disabled' });
        };
        await fourBosses(page);
        await page.goto(`${ADMIN}/?sw=off`);
        for (const [id, name, label] of [
          ['r-bm', 'XBM', '1boss'],
          ['r-carling', 'HCarling + HStar + XKalos + XBM', '4boss'],
        ] as const) {
          await page.locator(`[data-run="${id}"] .plan-card__open`).click();
          const surface = page.getByRole(vp.name === 'wide' ? 'complementary' : 'dialog', { name });
          await expect(surface).toBeVisible();
          await shot(`pane-${label}`);
          if (vp.name === 'wide') {
            await surface.getByRole('button', { name: 'Open in a larger view' }).click();
            await expect(page.getByRole('dialog', { name })).toBeVisible();
            await shot(`sheet-${label}`);
          }
          await page.keyboard.press('Escape');
          if (vp.name === 'wide') await page.keyboard.press('Escape');
          await expect(page.getByRole('dialog')).toHaveCount(0);
        }
        // Two bosses: the seeded run, without the extra bosses.
        await page.unrouteAll();
        await page.reload();
        await page.locator('[data-run="r-carling"] .plan-card__open').click();
        const two = page.getByRole(vp.name === 'wide' ? 'complementary' : 'dialog', { name: 'HCarling + HStar' });
        await expect(two).toBeVisible();
        await shot('pane-2boss');
        if (vp.name === 'wide') {
          await two.getByRole('button', { name: 'Open in a larger view' }).click();
          await expect(page.getByRole('dialog', { name: 'HCarling + HStar' })).toBeVisible();
          await shot('sheet-2boss');
        }

        await page.goto(`${ADMIN}/fixed?sw=off`);
        await page.getByRole('button', { name: 'Edit Friday 21:30 — XKalos' }).click();
        const editor = vp.name === 'wide' ? page.getByRole('complementary', { name: 'Weekly timing details' }) : page.getByRole('dialog');
        await expect(editor.getByRole('spinbutton', { name: 'Time' })).toBeVisible();
        await shot('fixed-editor');
      });
    }
  }
});
