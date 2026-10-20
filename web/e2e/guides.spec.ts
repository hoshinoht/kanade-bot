import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, choose, expect, settle, setModules, test } from './support';

// Guides (module `guides`, boards docs/research/2026-10-16-guide-mockups) over
// the e2e server's real publisher and fake Discord. The seed: "Limbo party
// roles" published to #guide-limbo (5 messages: text, Limbo's embed with its
// large image, text, spacer, text) and "Weekly bossing etiquette", a draft
// with no channel. `POST /__e2e/discord/guide {script}` makes the next post
// unconfirmed (`ambiguous_create`), the next edit find its message deleted
// (`edit_404`) or Discord go offline; `GET /__e2e/discord/calls` lists what
// the fake Discord was asked.

const LIMBO = '3d0c5b8a-7e21-4f3a-b6d9-2a4c8e1f5a10';
const toast = (page: Page, text: string | RegExp) => page.locator('.toast:not(.is-leaving)').filter({ hasText: text });

type Call = { op: string; channel?: string; message?: string | null; outcome?: string; silent?: boolean };

async function calls(): Promise<Call[]> {
  const read = await fetch(`${ADMIN}/__e2e/discord/calls`);
  expect(read.status).toBe(200);
  return ((await read.json()) as Call[]).filter((c) => ['create', 'edit', 'delete'].includes(c.op));
}

async function script(name: string) {
  const set = await fetch(`${ADMIN}/__e2e/discord/guide`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ script: name }) });
  expect(set.status, await set.text()).toBe(204);
}

async function guides(page: Page, item = '') {
  await page.goto(`${ADMIN}/guides?${item ? `item=${item}&` : ''}sw=off`);
  await expect(page.getByRole('listbox', { name: 'Guides' })).toBeVisible();
}

// Fidelity tags (`data-fid`) are stripped from normal builds: specs use roles and the screen's classes.
const pane = (page: Page) => page.locator('.guide-pane');
const paneCap = (page: Page) => page.locator('.guide-pane .drafts__panehead .cap');

/** Opens block `n` (1-based) by its summary. */
async function openBlock(page: Page, n: number) {
  await page.getByRole('button', { name: new RegExp(`^Edit block ${n}:`) }).click();
}

async function save(page: Page) {
  await page.getByRole('button', { name: 'Save draft' }).click();
  await expect(page.locator('.guide-savebar')).toContainText(/Draft saved · version \d+/);
}

async function openPlan(page: Page, title = 'Limbo party roles') {
  await page.getByRole('button', { name: 'Review and publish…' }).click();
  const plan = page.getByRole('dialog', { name: new RegExp(`(Publish|Move) ${title}`) });
  await expect(plan.locator('.gplan__counts, .flash--ok').first()).toBeVisible();
  return plan;
}

async function published(page: Page) {
  await expect(paneCap(page)).toHaveText(/^Published/, { timeout: 15_000 });
}

test('a new guide: create, write with the live counter, preview, plan, publish, done', async ({ page }) => {
  await guides(page);
  // The rail's More holds Guides, and on this page takes its name.
  await expect(page.getByRole('button', { name: 'More, current page Guides' })).toBeVisible();
  await expect(page.getByRole('option')).toHaveCount(2);
  await expect(page.getByRole('option', { name: /Limbo party roles/ })).toContainText('Published');
  await expect(page.getByRole('option', { name: /Weekly bossing etiquette/ })).toContainText('Not published');

  await page.getByRole('button', { name: 'New guide' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'New guide' });
  await dialog.getByLabel('Title').fill('Bellona basics');
  // The channel picker: #guide-limbo is Limbo party roles', listed and disabled.
  const list = page.getByRole('listbox', { name: /Channel/ });
  await dialog.getByRole('combobox', { name: 'Channel' }).click();
  await expect(list.getByRole('option', { name: /#guide-limbo/ })).toBeDisabled();
  await list.getByRole('option', { name: /#guide-raids/ }).click();
  await dialog.getByRole('button', { name: 'Start guide' }).click();
  await expect(page.getByRole('option', { name: /Bellona basics/, selected: true })).toBeVisible();
  await expect(toast(page, 'Bellona basics started')).toBeVisible();

  // Text: the live UTF-16 counter, red and refused past 2,000.
  await page.locator('.gblocks__add').getByRole('button', { name: 'Text' }).click();
  const text = page.getByLabel('Block 1 text, Discord Markdown');
  await text.fill('# Bellona basics\nStand **left** of the gate.');
  await expect(page.locator('.gblock__count')).toHaveText('44 / 2,000');
  await text.fill('x'.repeat(2_010));
  await expect(page.locator('.gblock__count')).toHaveText('2,010 / 2,000');
  await expect(page.locator('.gblock__count')).toHaveClass(/gblock__count--over/);
  await expect(page.getByText('10 over Discord’s 2,000 limit')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save draft' })).toBeDisabled();
  await text.fill('# Bellona basics\nStand **left** of the gate.');
  // Publish stays off while anything is unsaved.
  await expect(page.getByRole('button', { name: 'Review and publish…' })).toHaveAttribute('aria-disabled', 'true');
  await expect(pane(page).locator('.decision__why')).toContainText('save your');

  // A bosses block: Kalos with its large image.
  await page.locator('.gblocks__add').getByRole('button', { name: 'Bosses' }).click();
  await choose(page.getByRole('combobox', { name: 'Add boss' }), { label: /Gatekeeper Kalos/ });
  await expect(page.locator('.gentry')).toContainText('Gatekeeper Kalos · Lv265');
  await page.locator('.gentry__image').getByRole('checkbox').check();

  // The preview draws the unsaved blocks as Discord shows them.
  const slots = pane(page).locator('.gslot');
  await expect(slots).toHaveCount(2);
  await expect(slots.first()).toContainText('Bellona basics');
  await expect(slots.nth(1).locator('.dcard__title')).toContainText('Gatekeeper Kalos · Lv265');
  await expect(slots.nth(1).locator('img.dcard__image')).toHaveCount(1);

  await save(page);
  const plan = await openPlan(page, 'Bellona basics');
  await expect(plan.locator('.gplan__counts')).toContainText('Add 2');
  await expect(plan).toContainText('first publish to #guide-raids');
  await expect(plan).toContainText('Posts silently with no mentions; nobody is pinged.');
  await plan.getByRole('button', { name: 'Publish 2 changes' }).click();
  await published(page);
  await expect(pane(page).locator('.guide-run__summary')).toContainText('2 added');
  await expect(toast(page, 'Bellona basics published · 2 added')).toBeVisible();
  const made = await calls();
  expect(made.map((c) => [c.op, c.outcome, c.silent])).toEqual([
    ['create', 'delivered', true],
    ['create', 'delivered', true],
  ]);
  await expect(page.getByRole('option', { name: /Bellona basics/ })).toContainText('Published');
});

test('a middle repost needs its own tick', async ({ page }) => {
  await guides(page, LIMBO);
  // Kalos joins message 2 with its art: an edit cannot add files, so 2–5 are deleted and posted again.
  await openBlock(page, 2);
  await choose(page.getByRole('combobox', { name: 'Add boss' }), { label: /Gatekeeper Kalos/ });
  await save(page);
  const plan = await openPlan(page);
  await expect(plan.locator('.gplan__counts')).toContainText('Delete 4');
  const publish = plan.getByRole('button', { name: /^Publish \d+ changes$/ });
  await expect(publish).toHaveAttribute('aria-disabled', 'true');
  await plan.getByRole('checkbox', { name: 'Delete and repost messages 2–5' }).check();
  await expect(publish).toHaveAttribute('aria-disabled', 'false');
  await publish.click();
  await published(page);
  const made = await calls();
  expect(made.filter((c) => c.op === 'delete')).toHaveLength(4);
  expect(made.filter((c) => c.op === 'create')).toHaveLength(4);
  // Deletes newest first, all before the first new post.
  expect(made.findIndex((c) => c.op === 'create')).toBe(4);
});

test('a channel move posts the new set, then deletes the old one', async ({ page }) => {
  await guides(page, LIMBO);
  await choose(page.locator('.guide-channel').getByRole('combobox', { name: 'Channel' }), { label: /#guide-raids/ });
  await expect(page.locator('.guide-channel')).toContainText('Was #guide-limbo');
  await save(page);
  const plan = await openPlan(page);
  await expect(plan.getByRole('heading', { name: 'Move Limbo party roles to #guide-raids' })).toBeVisible();
  await expect(plan.getByRole('checkbox', { name: 'Then delete the 5 old messages in #guide-limbo' })).toBeChecked();
  await plan.getByRole('button', { name: 'Move and publish 5 messages' }).click();
  await published(page);
  const made = await calls();
  expect(made.map((c) => `${c.op} ${c.channel}`)).toEqual([
    ...Array(5).fill('create 120000000000000012'),
    ...Array(5).fill('delete 120000000000000011'),
  ]);
});

test('a stale save never merges: copy your text, or load the newer version', async ({ page, browser, context }) => {
  await guides(page, LIMBO);
  const other = await browser.newContext({ storageState: await context.storageState() });
  const second = await other.newPage();
  await second.goto(`${ADMIN}/guides?item=${LIMBO}&sw=off`);
  await openBlock(second, 3);
  await second.getByLabel('Block 3 text, Discord Markdown').fill('## Light side\n- Three bishops.');
  await second.getByRole('button', { name: 'Save draft' }).click();
  await expect(second.locator('.guide-savebar')).toContainText('Draft saved · version 3');
  await other.close();

  await openBlock(page, 1);
  await page.getByLabel('Block 1 text, Discord Markdown').fill('# Limbo party roles\nMine.');
  await page.getByRole('button', { name: 'Save draft' }).click();
  const stale = page.getByRole('alert').filter({ hasText: 'while you were editing' });
  await expect(stale).toContainText('saved version 3');
  await expect(page.getByRole('button', { name: 'Copy my block 1 text' })).toBeVisible();
  await page.getByRole('button', { name: 'Load version 3' }).click();
  await expect(stale).toHaveCount(0);
  await expect(page.getByRole('button', { name: /^Edit block 3:/ })).toContainText('Three bishops');
  await expect(page.locator('.guide-savebar')).toContainText('Draft saved · version 3');
});

test('a double press of Publish sends one key and makes one run', async ({ page }) => {
  await guides(page, LIMBO);
  await openBlock(page, 5);
  await page.getByLabel('Block 5 text, Discord Markdown').fill('-# Kept by the officers. Ask in #boss-schedule.');
  await save(page);
  const keys: string[] = [];
  page.on('request', (request) => {
    if (request.method() === 'POST' && request.url().endsWith('/publish')) keys.push(request.headers()['idempotency-key'] ?? '');
  });
  const plan = await openPlan(page);
  await plan.getByRole('button', { name: 'Publish 1 change' }).dblclick();
  await published(page);
  expect(new Set(keys).size).toBe(1);
  expect((await calls()).map((c) => c.op)).toEqual(['edit']);
});

test('an unconfirmed post blocks the guide until it is settled', async ({ page }) => {
  await guides(page, LIMBO);
  await page.locator('.gblocks__add').getByRole('button', { name: 'Text' }).click();
  await page.getByLabel('Block 6 text, Discord Markdown').fill('## Swaps\n- The caller counts down from three.');
  await save(page);
  await script('ambiguous_create');
  const plan = await openPlan(page);
  await plan.getByRole('button', { name: 'Publish 1 change' }).click();
  const unknown = page.locator('.guide-unknown');
  await expect(unknown).toContainText('Couldn’t confirm message 6 was posted', { timeout: 15_000 });
  await expect(page.getByRole('button', { name: 'Review and publish…' })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('option', { name: /Limbo party roles/ })).toContainText('Needs attention');
  // Search again finds nothing (the post never landed): still unconfirmed.
  await page.getByRole('button', { name: 'Search again' }).click();
  await expect(unknown.getByText('Needs attention')).toBeVisible({ timeout: 15_000 });
  // Mark as not posted, confirmed; the next publish posts it again.
  await page.getByRole('button', { name: 'Mark as not posted…' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Mark message 6 as not posted?' });
  await expect(confirm).toContainText('members will see message 6 twice');
  await confirm.getByRole('button', { name: 'Mark as not posted' }).click();
  await expect(toast(page, 'Message 6 is marked as not posted')).toBeVisible();
  await expect(unknown).toHaveCount(0);
  const again = await openPlan(page);
  await again.getByRole('button', { name: 'Publish 1 change' }).click();
  await published(page);
  const creates = (await calls()).filter((c) => c.op === 'create');
  expect(creates.map((c) => c.outcome)).toEqual(['ambiguous: Timeout', 'delivered']);
});

test('an edit that finds its message deleted stops the run, nothing else sent', async ({ page }) => {
  await guides(page, LIMBO);
  await openBlock(page, 1);
  await page.getByLabel('Block 1 text, Discord Markdown').fill('# Limbo party roles\nWho stands where.');
  await openBlock(page, 3);
  await page.getByLabel('Block 3 text, Discord Markdown').fill('## Light side\n- Two bishops.');
  await save(page);
  await script('edit_404');
  const plan = await openPlan(page);
  await plan.getByRole('button', { name: 'Publish 2 changes' }).click();
  const stopped = page.locator('.guide-pane .gplan__flash');
  await expect(stopped).toContainText('Message 1 is gone from #guide-limbo', { timeout: 15_000 });
  await expect(paneCap(page)).toHaveText(/^Stopped/);
  expect((await calls()).map((c) => `${c.op} ${c.outcome}`)).toEqual(['edit rejected: UnknownMessage']);
});

test('Discord offline: the plan is unchecked and Publish is refused with the reason', async ({ page }) => {
  await guides(page, LIMBO);
  await openBlock(page, 5);
  await page.getByLabel('Block 5 text, Discord Markdown').fill('-# Kept by the officers, offline.');
  await save(page);
  await script('offline');
  const plan = await openPlan(page);
  await expect(plan).toContainText('Not checked');
  await plan.getByRole('button', { name: 'Publish 1 change' }).click();
  await expect(plan.getByRole('alert')).toContainText('Discord is not connected; publish once the bot is back online.');
  await plan.getByRole('button', { name: 'Cancel' }).click();
  await expect(pane(page).locator('.decision__why')).toContainText('Discord is offline');
  expect(await calls()).toEqual([]);
});

test('delete: unpublish first, then delete the guide', async ({ page }) => {
  await guides(page, LIMBO);
  await page.getByRole('button', { name: 'Delete guide…' }).click();
  const unpublish = page.getByRole('alertdialog', { name: 'Delete Limbo party roles?' });
  await expect(unpublish).toContainText('Unpublish it first');
  await unpublish.getByRole('button', { name: 'Unpublish 5 messages' }).click();
  await expect(paneCap(page)).toHaveText(/^Unpublished/, { timeout: 15_000 });
  expect((await calls()).map((c) => c.op)).toEqual(Array(5).fill('delete'));
  await pane(page).getByRole('button', { name: 'Delete guide…' }).click();
  const remove = page.getByRole('alertdialog', { name: 'Delete Limbo party roles?' });
  await expect(remove).toContainText('Nothing is posted any more');
  await remove.getByRole('button', { name: 'Delete guide' }).click();
  await expect(toast(page, 'Limbo party roles is deleted.')).toBeVisible();
  await expect(page.getByRole('option')).toHaveCount(1);
});

test('History lists guide changes in their own words', async ({ page }) => {
  await page.goto(`${ADMIN}/history?sw=off`);
  const rows = page.locator('.history-row').filter({ hasText: 'Limbo party roles' });
  await expect(rows.filter({ hasText: 'Guides · Limbo party roles — published: 5 added' })).toHaveCount(1);
  const request = rows.filter({ hasText: 'publish requested: add 5' });
  await request.click();
  const detail = page.locator('.side-pane--history');
  await expect(detail.getByRole('heading', { name: 'Limbo party roles: publish requested' })).toBeVisible();
  await expect(detail.getByRole('link', { name: 'Open Limbo party roles in Guides' })).toHaveAttribute('href', `/guides?item=${LIMBO}`);
});

test('module off: no Guides under More, and /guides is not found', async ({ page }) => {
  await setModules({ guides: false });
  await page.goto(`${ADMIN}/guides?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText("That doesn't exist");
  await page.getByRole('button', { name: 'More' }).click();
  const menu = page.getByRole('menu', { name: 'More destinations' });
  await expect(menu.getByRole('menuitem')).toHaveText(['Rewrites', 'Limits', 'Reminders', /^\s*Setup/]);
});

test('axe: the list, the editor and the plan', async ({ page }) => {
  test.setTimeout(60_000);
  const serious = async (label: string) => {
    await settle(page);
    const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
    const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
    expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
  };
  await guides(page, LIMBO);
  await expect(pane(page).locator('.gslot')).toHaveCount(5);
  await serious('guides list');
  await openBlock(page, 2);
  await serious('bosses block open');
  await openBlock(page, 1);
  await page.getByLabel('Block 1 text, Discord Markdown').fill('# Limbo party roles\nChanged.');
  await save(page);
  await openPlan(page);
  await serious('publish plan');
});

test.describe('phone', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('the list, then the guide with Edit / Preview tabs and the bottom bar', async ({ page }) => {
    await guides(page);
    await page.getByRole('option', { name: /Limbo party roles/ }).click();
    await expect(page.getByRole('button', { name: 'Back to the list (Guides)' })).toBeVisible();
    const tabs = page.locator('.guide-phonetabs');
    await expect(tabs.getByRole('tab', { name: 'Edit' })).toHaveAttribute('aria-selected', 'true');
    await openBlock(page, 1);
    await page.getByLabel('Block 1 text, Discord Markdown').fill('# Limbo party roles\nOn a phone.');
    const bar = page.locator('.guide-phone > .decision-card');
    await expect(bar).toContainText('1 unsaved change');
    await expect(bar.getByRole('button', { name: 'Publish…' })).toHaveAttribute('aria-disabled', 'true');
    await tabs.getByRole('tab', { name: /Preview · 5 messages/ }).click();
    await expect(page.locator('.gslot').first()).toContainText('On a phone.');
    await bar.getByRole('button', { name: 'Save draft' }).click();
    await expect(bar.getByRole('button', { name: 'Publish…' })).toHaveAttribute('aria-disabled', 'false');
    // The document never scrolls; the panel does.
    expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight + 1)).toBe(true);
    await page.getByRole('button', { name: 'Back to the list (Guides)' }).click();
    await expect(page.getByRole('option', { name: /Limbo party roles/ })).toBeVisible();
  });
});
