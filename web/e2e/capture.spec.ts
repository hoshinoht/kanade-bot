import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, REAL_ART, expect, test } from './support';

// Reference captures for the v4 comparison, both git-ignored. Default: the
// synthetic placeholder art (fixtures, not the game's art) into
// e2e/.captures/synthetic/. With KANADE_REAL_ART=1: the local, private art
// into e2e/.captures/real/ for the user's visual review.
const OUT = REAL_ART ? 'e2e/.captures/real' : 'e2e/.captures/synthetic';
const VIEWPORTS = [
  { name: 'wide', width: 1280, height: 800 },
  { name: 'narrow', width: 390, height: 844 },
];
const LOOKS = [
  { name: 'marigold-light', colorway: 'marigold', theme: 'light' },
  { name: 'marigold-dark', colorway: 'marigold', theme: 'dark' },
  { name: 'twilight-dark', colorway: 'twilight', theme: 'dark' },
];

async function settle(page: Page) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    // Lazy images outside the viewport never decode; wait for the rest, briefly.
    const eager = [...document.images].filter((i) => i.loading !== 'lazy' || i.getBoundingClientRect().top < innerHeight);
    await Promise.race([Promise.all(eager.map((i) => i.decode().catch(() => {}))), new Promise((r) => setTimeout(r, 3000))]);
  });
}

async function shot(page: Page, name: string) {
  await settle(page);
  await page.screenshot({ path: `${OUT}/${name}.png`, animations: 'disabled' });
}

for (const vp of VIEWPORTS) {
  for (const look of LOOKS) {
    test(`capture ${vp.name} ${look.name}`, async ({ page }) => {
      await page.setViewportSize({ width: vp.width, height: vp.height });
      await page.addInitScript(
        ([c, t]) => {
          localStorage.setItem('colorway', c!);
          localStorage.setItem('theme', t!);
        },
        [look.colorway, look.theme],
      );
      const tag = `${vp.name}-${look.name}`;

      await page.goto(`${ADMIN}/?sw=off`);
      await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
      await shot(page, `admin-week-${tag}`);

      await page.locator('[data-run="r-carling"] .plan-card__open').click();
      await expect(page.getByRole('dialog', { name: 'HCarling + HStar' })).toBeVisible();
      await shot(page, `admin-sheet-${tag}`);
      await page.keyboard.press('Escape');

      await page.goto(`${ADMIN}/config?section=access&sw=off`);
      await expect(page.getByRole('table', { name: "The bot's permissions in each channel" })).toBeVisible();
      await shot(page, `admin-config-access-${tag}`);

      for (const [section, name, ready] of [
        ['models', 'admin-config-models', page.getByText(/Capacity groups/)],
        ['persona', 'admin-config-persona', page.getByText(/Reload profiles/)],
        ['self-service', 'admin-config-self-service', page.getByText(/pre-filled link to the public portal/)],
      ] as const) {
        await page.goto(`${ADMIN}/config?section=${section}&sw=off`);
        await expect(ready).toBeVisible();
        await shot(page, `${name}-${tag}`);
      }
      // Models with the cloud warning showing.
      await page.goto(`${ADMIN}/config?section=models&sw=off`);
      await page.getByRole('combobox', { name: /^Model/ }).first().selectOption('kanata/chat-cloud');
      await expect(page.getByText(/go to an external provider/)).toBeVisible();
      await shot(page, `admin-config-models-cloud-${tag}`);

      for (const path of ['fixed', 'bosses', 'members', 'reminders', 'inbox', 'extractions', 'chat', 'limits', 'history', 'config']) {
        await page.goto(`${ADMIN}/${path}?sw=off`);
        await expect(page.getByRole('heading', { level: 1 })).not.toHaveText(
          /^(Weekly timings|Bosses|Members|Reminders|Inbox|Extractions|Chat|Limits|0 changes)$/,
        );
        await shot(page, `admin-${path}-${tag}`);
      }

      for (const [path, name] of [
        ['extractions/x-kalos', 'extraction'],
        ['chat/c-move', 'chat-turn'],
        ['bosses/MaleficStar/knowledge', 'knowledge'],
        ['bosses/Kai/knowledge', 'knowledge-event'],
      ] as const) {
        await page.goto(`${ADMIN}/${path}?sw=off`);
        await expect(page.getByRole('tablist').or(page.getByRole('heading', { name: 'Sources' })).first()).toBeVisible();
        await shot(page, `admin-${name}-${tag}`);
      }

      await page.goto(`${ADMIN}/history?sw=off`);
      await page.getByRole('button', { name: 'Revert #2' }).click();
      await expect(page.getByRole('dialog', { name: 'Revert #2?' }).getByRole('alert')).toBeVisible();
      await shot(page, `admin-history-revert-${tag}`);

      await page.goto(`${ADMIN}/fixed?sw=off`);
      await page.getByRole('button', { name: 'Edit Friday 21:30 — XKalos' }).click();
      await expect(page.getByRole('dialog')).toBeVisible();
      await shot(page, `admin-fixed-editor-${tag}`);
      await page.getByRole('dialog').getByLabel('Time').fill('21:00');
      await page.getByRole('button', { name: 'Save…' }).click();
      await expect(page.getByRole('radio', { name: 'Update to the new timing' })).toBeVisible();
      await shot(page, `admin-fixed-choice-${tag}`);

      await page.goto(`${ADMIN}/members?sw=off`);
      await page.getByRole('button', { name: /^Asahi/ }).click();
      await expect(page.getByRole('dialog', { name: 'Asahi' })).toBeVisible();
      await shot(page, `admin-member-sheet-${tag}`);

      await page.goto(`${ADMIN}/login?sw=off`);
      await expect(page.getByRole('button', { name: 'Sign in with Discord' })).toBeVisible();
      await shot(page, `admin-login-${tag}`);

      await page.goto(`${PUBLIC}/?sw=off`);
      await expect(page.getByRole('tab', { selected: true })).toBeVisible();
      await shot(page, `public-week-${tag}`);
    });
  }
}

// The planner card's grip: at rest, under a fine pointer's hover, and with
// keyboard focus. Full frame plus a close crop around the card.
for (const look of LOOKS) {
  test(`capture admin grip states ${look.name}`, async ({ page }) => {
    await page.addInitScript(
      ([c, t]) => {
        localStorage.setItem('colorway', c!);
        localStorage.setItem('theme', t!);
      },
      [look.colorway, look.theme],
    );
    await page.goto(`${ADMIN}/?sw=off`);
    const card = page.locator('[data-run="r-carling"]');
    await expect(card).toBeVisible();
    const crop = async (name: string) => {
      await settle(page);
      const box = (await card.boundingBox())!;
      await page.screenshot({
        path: `${OUT}/${name}-crop.png`,
        animations: 'disabled',
        clip: { x: box.x - 24, y: box.y - 24, width: box.width + 48, height: box.height + 48 },
      });
    };
    await crop(`admin-week-grip-rest-wide-${look.name}`);
    await card.hover();
    await page.waitForTimeout(200);
    await shot(page, `admin-week-grip-hover-wide-${look.name}`);
    await crop(`admin-week-grip-hover-wide-${look.name}`);
    await page.mouse.move(2, 2);
    await page.locator('[data-handle="r-carling"]').focus();
    await page.keyboard.press('Shift+Tab');
    await page.keyboard.press('Tab');
    await expect(page.locator('[data-handle="r-carling"]')).toBeFocused();
    await page.waitForTimeout(200);
    await shot(page, `admin-week-grip-focus-wide-${look.name}`);
    await crop(`admin-week-grip-focus-wide-${look.name}`);
  });
}

test('capture admin keyboard lift and palette', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-handle="r-carling"]').focus();
  await page.keyboard.press('m');
  await page.keyboard.press('ArrowRight');
  await shot(page, 'admin-wide-lifted');
  await page.keyboard.press('Escape');
  await page.keyboard.press('ControlOrMeta+k');
  await page.getByRole('combobox', { name: 'Search commands' }).fill('go');
  await shot(page, 'admin-wide-palette');
});

// Side-by-side with the live v4 captures in e2e/.captures/v4-live/ (same page
// names, 1280×800 wide; 422 px narrow like v4's). Only with KANADE_REAL_ART=1.
const COMPARE: [string, string][] = [
  ['week', '/'],
  ['week-next', '/?week=next'],
  ['fixed', '/fixed'],
  ['bosses', '/bosses'],
  ['boss-knowledge', '/bosses/Carling/knowledge'],
  ['inbox', '/inbox'],
  ['extractions', '/extractions'],
  ['extraction-detail', '/extractions/x-kalos'],
  ['chat', '/chat'],
  ['chat-detail', '/chat/c-move'],
  ['limits', '/limits'],
  ['members', '/members'],
  ['reminders', '/reminders'],
  ['audit', '/history'],
  ['config', '/config'],
  ['config-pings', '/config?section=pings'],
  ['config-watching', '/config?section=watching'],
  ['config-chatbot', '/config?section=chatbot'],
  ['config-persona', '/config?section=persona'],
  ['config-models', '/config?section=models'],
  ['config-self-service', '/config?section=self-service'],
  ['config-notifications', '/config?section=notifications'],
  ['config-theme', '/config?section=theme'],
  ['config-digest', '/config?section=digest'],
  ['config-rescan', '/config?section=rescan'],
  ['config-access', '/config?section=access'],
  ['config-env', '/config?section=env'],
];
for (const vp of [
  { name: 'wide', width: 1280, height: 800 },
  { name: 'narrow', width: 422, height: 900 },
]) {
  test(`capture v4 comparison set ${vp.name}`, async ({ page }) => {
    test.skip(!REAL_ART, 'real art only');
    await page.setViewportSize({ width: vp.width, height: vp.height });
    await page.addInitScript(() => {
      localStorage.setItem('colorway', 'marigold');
      localStorage.setItem('theme', 'light');
    });
    for (const [name, path] of COMPARE) {
      await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      await page.waitForTimeout(250);
      await shot(page, `compare/v5-${name}-${vp.name}`);
    }
    await page.goto(`${ADMIN}/?sw=off`);
    await page.locator('[data-run="r-carling"] .plan-card__open').click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await shot(page, `compare/v5-run-sheet-${vp.name}`);
  });
}

// Batch 6 evidence: Week at the five layout sizes, the Inbox (both tabs,
// wide and narrow) and filtered Chat / Extractions.
test('capture batch 6 layout set', async ({ page }) => {
  test.setTimeout(120_000);
  await page.addInitScript(() => {
    localStorage.setItem('colorway', 'marigold');
    localStorage.setItem('theme', 'light');
  });
  for (const [w, h] of [[1280, 800], [1000, 670], [1280, 600], [390, 844], [844, 390]] as const) {
    await page.setViewportSize({ width: w, height: h });
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    await shot(page, `b6/week-${w}x${h}`);
  }
  for (const [w, h, name] of [[1280, 800, 'wide'], [390, 844, 'narrow']] as const) {
    await page.setViewportSize({ width: w, height: h });
    await page.goto(`${ADMIN}/inbox?tab=extractor&sw=off`);
    await expect(page.getByRole('listbox')).toBeVisible();
    await shot(page, `b6/inbox-extractor-${name}`);
    await page.goto(`${ADMIN}/inbox?tab=self_service&item=p-fa-request&sw=off`);
    await expect(page.getByText('Changed since the member asked')).toBeVisible();
    await shot(page, `b6/inbox-self-service-${name}`);
    await page.goto(`${ADMIN}/chat?outcome=timeout,error,refused&sw=off`);
    await page.getByRole('button', { name: /^Filters/ }).click();
    await shot(page, `b6/chat-filtered-${name}`);
    await page.goto(`${ADMIN}/extractions?outcome=proposed,failed&model=kanata%2Fextract&sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toContainText('of 34');
    await shot(page, `b6/extractions-filtered-${name}`);
  }
});
