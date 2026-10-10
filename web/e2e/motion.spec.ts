import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, expect, signInPublic, test } from './support';

declare global {
  interface Window {
    /** CSS animations as they start (name, element tag and classes): the member portal cases. */
    __css: { name: string; what: string }[];
  }
}

// M3E static motion (m3e-rail-design-spec "Motion and loading"): press
// shape-morph, pane enter/exit, toast exit, own-move Week FLIP, the 200 ms
// loading standard and the opt-in planner overshoot (Experiment E). Every
// piece is CSP-safe (CSS classes, @keyframes or Web Animations) and instant
// with reduced motion. Frame timings at 390×844 are attached to the report.

const PHONE = { width: 390, height: 844 };

/** Records every Web Animation started on the page (target and first keyframe). */
async function recordAnimations(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as { __animated: { target: string; from: string }[] };
    w.__animated = [];
    const original = Element.prototype.animate;
    Element.prototype.animate = function (this: Element, frames, options) {
      const first = Array.isArray(frames) ? (frames[0] as Record<string, unknown> | undefined) : undefined;
      const id = this instanceof HTMLElement ? (this.dataset.run ?? this.getAttribute('aria-label') ?? this.className) : this.tagName;
      w.__animated.push({ target: String(id), from: String(first?.transform ?? '') });
      return original.call(this, frames, options);
    };
  });
}

const animated = (page: Page) => page.evaluate(() => (window as unknown as { __animated: { target: string; from: string }[] }).__animated);
const clearAnimated = (page: Page) => page.evaluate(() => ((window as unknown as { __animated: unknown[] }).__animated = []));

/** Frame intervals (ms) while `act` runs and for `ms` after it. */
async function frameTimes(page: Page, act: () => Promise<void>, ms = 600): Promise<{ fps: number; worst: number; frames: number }> {
  await page.evaluate((span) => {
    const w = window as unknown as { __frames: number[]; __framesDone: Promise<void> };
    w.__frames = [];
    w.__framesDone = new Promise((done) => {
      let last = 0;
      let end = 0;
      const step = (t: number) => {
        if (last) w.__frames.push(t - last);
        else end = t + span;
        last = t;
        if (t < end) requestAnimationFrame(step);
        else done();
      };
      requestAnimationFrame(step);
    });
  }, ms);
  await act();
  const gaps = await page.evaluate(async () => {
    const w = window as unknown as { __frames: number[]; __framesDone: Promise<void> };
    await w.__framesDone;
    return w.__frames;
  });
  const mean = gaps.reduce((a, b) => a + b, 0) / gaps.length;
  return { fps: Math.round(1000 / mean), worst: Math.round(Math.max(...gaps) * 10) / 10, frames: gaps.length };
}

function note(name: string, timing: { fps: number; worst: number; frames: number }) {
  test.info().annotations.push({ type: 'frames', description: `${name}: ${timing.fps} fps mean, worst frame ${timing.worst} ms over ${timing.frames} frames` });
}

const FPS_FLOOR = 50;

/** The frame-rate floor, enforced locally. CI runners have no GPU and share 4 vCPUs, so there it is only reported. */
function fpsFloor(name: string, fps: number) {
  if (process.env.CI) {
    test.info().annotations.push({ type: 'fps-floor', description: `${name}: ${fps} fps (floor ${FPS_FLOOR} reported only on CI${fps < FPS_FLOOR ? ', BELOW' : ''})` });
    return;
  }
  expect(fps, `${name} fps`).toBeGreaterThanOrEqual(FPS_FLOOR);
}

async function liftAndDrop(page: Page) {
  const handle = page.locator('[data-handle="r-carling"]');
  await handle.focus();
  await page.keyboard.press('m');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Enter');
}

test.describe('Week: own-move FLIP', () => {
  test.beforeEach(async ({ page }) => recordAnimations(page));

  test('a keyboard move glides the card from its old day to the new one (390×844, with frame timing)', async ({ page }) => {
    await page.setViewportSize(PHONE);
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    await clearAnimated(page);
    const timing = await frameTimes(page, () => liftAndDrop(page));
    note('Week FLIP 390×844', timing);
    const moves = (await animated(page)).filter((a) => a.target === 'r-carling');
    expect(moves.length).toBeGreaterThan(0);
    expect(moves[0]!.from).toMatch(/^translate\(-?[\d.]+px, -?[\d.]+px\)$/);
    fpsFloor('Week FLIP 390×844', timing.fps);
    // Undo moves it back the same way.
    await clearAnimated(page);
    await page.getByRole('group', { name: 'Notification' }).getByRole('button', { name: 'Undo' }).click();
    await expect.poll(async () => (await animated(page)).some((a) => a.target === 'r-carling')).toBe(true);
  });

  test('a polled week does not animate the board', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    await clearAnimated(page);
    await page.getByRole('button', { name: 'Refresh' }).click();
    await page.waitForTimeout(400);
    expect((await animated(page)).filter((a) => a.from.startsWith('translate('))).toEqual([]);
  });

  test('reduced motion: the card lands at once', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    await clearAnimated(page);
    await liftAndDrop(page);
    await expect(page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Moved HCarling' })).toBeVisible();
    expect(await animated(page)).toEqual([]);
  });
});

test.describe('panes', () => {
  test.beforeEach(async ({ page }) => recordAnimations(page));

  test('Members: the pane enters forward, leaves through is-leaving (inert), then unmounts', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    // CSS exit animations as they start: the leaving pane can unmount before a
    // single computed-style read lands (a detached node reads as "").
    await page.addInitScript(() => {
      const w = window as unknown as { __exits: string[] };
      w.__exits = [];
      document.addEventListener('animationstart', (e) => {
        if (e.target instanceof Element && e.target.matches('aside.side-pane.is-leaving')) w.__exits.push(e.animationName);
      }, true);
    });
    await page.goto(`${ADMIN}/members?sw=off`);
    await page.getByRole('button', { name: /^Tsubame/ }).first().click();
    const pane = page.getByRole('complementary', { name: 'Member details' });
    await expect(pane).toBeVisible();
    expect((await animated(page)).some((a) => a.target === 'Member details' && a.from === 'translateX(24px)')).toBe(true);
    // Another member: the same pane, its content enters again.
    await clearAnimated(page);
    await page.locator('.memberlist__row').nth(1).click();
    await expect.poll(async () => (await animated(page)).some((a) => a.target === 'Member details')).toBe(true);
    const leaving = page.locator('aside.side-pane.is-leaving');
    await pane.getByRole('button', { name: 'Close member details' }).click();
    await expect(leaving).toHaveAttribute('inert', '');
    await expect.poll(() => page.evaluate(() => (window as unknown as { __exits: string[] }).__exits)).toContain('pane-exit');
    await expect(page.locator('aside.side-pane')).toHaveCount(0);
    await expect(page.locator('.memberlist__row--active')).toHaveCount(0);
  });

  test('Inbox on a phone: detail forward, list backward (with frame timing)', async ({ page }) => {
    await page.setViewportSize(PHONE);
    await page.goto(`${ADMIN}/inbox?tab=self_service&sw=off`);
    const list = page.getByRole('listbox', { name: 'Self-service items' });
    await expect(list).toBeVisible();
    await clearAnimated(page);
    const forward = await frameTimes(page, () => list.getByRole('option', { name: /HCarling/ }).click());
    note('Inbox detail enter 390×844', forward);
    expect((await animated(page)).some((a) => a.target.includes('inbox__detail') && a.from === 'translateX(24px)')).toBe(true);
    await clearAnimated(page);
    const backward = await frameTimes(page, () => page.locator('.topbar').getByRole('button', { name: 'Back to the list (Inbox)' }).click());
    note('Inbox list return 390×844', backward);
    await expect(list).toBeVisible();
    expect((await animated(page)).some((a) => a.target.includes('inbox__list') && a.from === 'translateX(-24px)')).toBe(true);
    fpsFloor('Inbox detail/list 390×844 (slower of the two)', Math.min(forward.fps, backward.fps));
  });

  test('History on a phone: the detail dialog fades out before it unmounts (with frame timing)', async ({ page }) => {
    await page.setViewportSize(PHONE);
    await page.goto(`${ADMIN}/history?sw=off`);
    await page.locator('.history-row').first().click();
    const dialog = page.locator('dialog.history-detail');
    await expect(dialog).toHaveAttribute('open', '');
    await dialog.evaluate((el) => Promise.all(el.getAnimations().map((a) => a.finished)));
    const exit = await frameTimes(page, () => page.keyboard.press('Escape'), 400);
    note('History dialog exit 390×844', exit);
    await expect(page.locator('dialog.history-detail')).toHaveCount(0);
    fpsFloor('History dialog exit 390×844', exit.fps);
  });

  test('reduced motion: no enter, and the pane goes at once', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}/members?sw=off`);
    await page.getByRole('button', { name: /^Tsubame/ }).first().click();
    const pane = page.getByRole('complementary', { name: 'Member details' });
    await expect(pane).toBeVisible();
    expect(await animated(page)).toEqual([]);
    await pane.getByRole('button', { name: 'Close member details' }).click();
    expect(await page.locator('aside.side-pane').count()).toBe(0);
  });
});

test('toasts: a dismissed toast leaves through its exit, hidden from assistive tech', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await liftAndDrop(page);
  const toast = page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Moved HCarling' });
  await expect(toast).toBeVisible();
  await toast.getByRole('button', { name: 'Dismiss' }).click();
  const leaving = page.locator('.toast.is-leaving');
  await expect(leaving).toHaveAttribute('aria-hidden', 'true');
  await expect(leaving).toHaveAttribute('inert', '');
  await expect(page.locator('.toast')).toHaveCount(0);
});

// The member portal mirrors the admin's motion on its equivalent screens: the
// run pane enters forward and leaves through `is-leaving` (the admin side
// panes), a phone run comes in forward and the Week returns backward (the
// admin's phone list-detail), cards settle as they mount on a week switch,
// the drawer slides, toasts leave through their exit and the loading
// indicator morphs. Reduced motion keeps none of it.
test.describe('member portal', () => {
  test.beforeEach(async ({ page }) => {
    await recordAnimations(page);
    await page.addInitScript(() => {
      window.__css = [];
      document.addEventListener(
        'animationstart',
        (e) => {
          if (e.target instanceof Element) window.__css.push({ name: e.animationName, what: `${e.target.tagName.toLowerCase()}.${[...e.target.classList].join('.')}` });
        },
        true,
      );
    });
    await signInPublic(page);
  });

  const css = (page: Page) => page.evaluate(() => window.__css);
  const clearCss = (page: Page) => page.evaluate(() => (window.__css = []));

  test('the run pane enters forward, leaves through is-leaving (inert), then Your week returns', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${PUBLIC}/?sw=off`);
    await expect(page.locator('main [data-run]').first()).toBeVisible();
    await clearAnimated(page);
    await page.getByRole('button', { name: /, you are in$/ }).first().click();
    const pane = page.getByRole('complementary', { name: /^Your run · / });
    await expect(pane).toBeVisible();
    expect((await animated(page)).some((a) => a.target.startsWith('Your run · ') && a.from === 'translateX(24px)')).toBe(true);
    // Another run: its content enters again.
    await clearAnimated(page);
    await page.getByRole('button', { name: /, not in this run, view only$/ }).first().click();
    const other = page.getByRole('complementary', { name: /^View only · / });
    await expect(other).toBeVisible();
    expect((await animated(page)).some((a) => a.target.startsWith('View only · ') && a.from === 'translateX(24px)')).toBe(true);
    // The List tab keeps the pane open without playing its enter again.
    await clearAnimated(page);
    await page.getByRole('tab', { name: /^List/ }).click();
    await expect(other).toBeVisible();
    await page.waitForTimeout(150); // "nothing starts" needs real time to pass
    expect((await animated(page)).filter((a) => a.from.startsWith('translateX'))).toEqual([]);
    await page.getByRole('tab', { name: /^Week/ }).click();
    await clearCss(page);
    await other.getByRole('button', { name: /^Close / }).click();
    const leaving = page.locator('aside.member-pane.is-leaving');
    await expect(leaving).toHaveAttribute('inert', '');
    await expect.poll(async () => (await css(page)).some((a) => a.name === 'pane-exit' && a.what.includes('member-pane'))).toBe(true);
    await expect(page.locator('aside.member-pane')).toHaveCount(0);
    await expect(page.getByRole('complementary', { name: 'Your week' })).toBeVisible();
  });

  test("This/Next week: the open pane leaves and the new week's cards settle in", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${PUBLIC}/?sw=off`);
    await page.getByRole('button', { name: /, you are in$/ }).first().click();
    await expect(page.getByRole('complementary', { name: /^Your run · / })).toBeVisible();
    await clearCss(page);
    const nextWeek = page.getByRole('navigation', { name: 'Which week' }).getByRole('link', { name: 'Next week' });
    await nextWeek.click();
    await expect(nextWeek).toHaveAttribute('aria-current', 'page');
    await expect.poll(async () => (await css(page)).some((a) => a.name === 'pane-exit')).toBe(true);
    await expect(page.locator('aside.member-pane')).toHaveCount(0);
    expect((await css(page)).filter((a) => a.name === 'settle' && a.what.includes('runcard')).length).toBeGreaterThan(0);
  });

  test('a phone run comes in forward and the Week returns backward (390×844, with frame timing)', async ({ page }) => {
    await page.setViewportSize(PHONE);
    await page.goto(`${PUBLIC}/?sw=off`);
    const card = page.getByRole('button', { name: /, you are in$/ }).first();
    await expect(card).toBeVisible();
    await clearAnimated(page);
    const forward = await frameTimes(page, () => card.click());
    note('Portal phone run enter 390×844', forward);
    await expect(page.getByRole('region', { name: 'Run', exact: true })).toBeVisible();
    const ran = await animated(page);
    expect(ran.some((a) => a.target === 'Run' && a.from === 'translateX(24px)')).toBe(true);
    expect(ran.some((a) => a.target === 'Run detail' && a.from === 'translateX(24px)')).toBe(true);
    await clearAnimated(page);
    const backward = await frameTimes(page, () => page.locator('.topbar').getByRole('button', { name: /Week/ }).first().click());
    note('Portal Week return 390×844', backward);
    await expect(page.getByRole('region', { name: 'Week' })).toBeVisible();
    expect((await animated(page)).some((a) => a.target === 'Week' && a.from === 'translateX(-24px)')).toBe(true);
    fpsFloor('Portal phone run/Week 390×844 (slower of the two)', Math.min(forward.fps, backward.fps));
  });

  test('the phone drawer slides in and fades out before it closes', async ({ page }) => {
    await page.setViewportSize(PHONE);
    await page.goto(`${PUBLIC}/?sw=off`);
    await expect(page.locator('main [data-run]').first()).toBeVisible();
    await clearCss(page);
    await page.locator('.topbar').getByRole('button', { name: 'Open the navigation' }).click();
    const drawer = page.locator('dialog.drawer');
    await expect(drawer).toHaveAttribute('open', '');
    await expect.poll(async () => (await css(page)).map((a) => a.name)).toContain('drawer-in');
    await page.keyboard.press('Escape');
    await expect(drawer).not.toHaveAttribute('open', '');
    expect(await drawer.evaluate((el) => getComputedStyle(el).transitionProperty)).toContain('transform');
  });

  test('toasts: a dismissed toast leaves through its exit, hidden from assistive tech', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${PUBLIC}/?sw=off`);
    await expect(page.locator('main [data-run]').first()).toBeVisible();
    await page.route(/\/api\/public\/week(\?|$)/, (route) => route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"internal","message":"Boom"}' }));
    await page.getByRole('button', { name: 'Refresh the week' }).click();
    const toast = page.getByRole('group', { name: 'Notification' }).filter({ hasText: "Couldn't refresh the week" });
    await expect(toast).toBeVisible();
    await toast.getByRole('button', { name: 'Dismiss' }).click();
    const leaving = page.locator('.toast.is-leaving');
    await expect(leaving).toHaveAttribute('aria-hidden', 'true');
    await expect(leaving).toHaveAttribute('inert', '');
    await expect(page.locator('.toast')).toHaveCount(0);
  });

  test('a slow week shows the morphing loading indicator; reduced motion stills it', async ({ page }) => {
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route(/\/api\/public\/week(\?|$)/, async (route) => {
      await held;
      await route.continue();
    });
    await page.goto(`${PUBLIC}/?sw=off`);
    const shape = page.locator('.loading-state .xp-loading__shape');
    await expect(shape).toBeVisible();
    expect(await shape.evaluate((el) => getComputedStyle(el).animationName)).toBe('xp-morph');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    expect(await shape.evaluate((el) => getComputedStyle(el).animationName)).toBe('none');
    release();
    await expect(page.locator('main [data-run]').first()).toBeVisible();
  });

  test('reduced motion: no enter, the pane goes at once, a phone run comes in still', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${PUBLIC}/?sw=off`);
    await page.getByRole('button', { name: /, you are in$/ }).first().click();
    const pane = page.getByRole('complementary', { name: /^Your run · / });
    await expect(pane).toBeVisible();
    await pane.getByRole('button', { name: /^Close / }).click();
    expect(await page.locator('aside.member-pane').count()).toBe(0);
    await page.setViewportSize(PHONE);
    await page.getByRole('button', { name: /, you are in$/ }).first().click();
    await expect(page.getByRole('region', { name: 'Run', exact: true })).toBeVisible();
    await page.locator('.topbar').getByRole('button', { name: /Week/ }).first().click();
    await expect(page.getByRole('region', { name: 'Week' })).toBeVisible();
    await page.waitForTimeout(150); // "nothing starts" needs real time to pass
    expect(await animated(page)).toEqual([]);
  });
});

test.describe('press shape-morph (Experiment D, on by default)', () => {
  test('a key button steps its corners down while pressed and springs back', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}/fixed?sw=off`);
    const key = page.locator('[data-fixed-add]');
    await expect(key).toBeVisible();
    const radius = () => key.evaluate((el) => parseFloat(getComputedStyle(el).borderTopLeftRadius));
    const resting = await radius();
    expect(await key.evaluate((el) => getComputedStyle(el).transitionProperty)).toContain('border-radius');
    const box = (await key.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await expect.poll(radius).toBe(10);
    await page.mouse.up();
    await expect.poll(radius).toBe(resting);
    await page.keyboard.press('Escape');
  });

  test('reduced motion keeps the shape still', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}/fixed?sw=off`);
    const key = page.locator('[data-fixed-add]');
    const resting = await key.evaluate((el) => getComputedStyle(el).borderTopLeftRadius);
    const box = (await key.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(100);
    expect(await key.evaluate((el) => getComputedStyle(el).borderTopLeftRadius)).toBe(resting);
    await page.mouse.up();
    await page.keyboard.press('Escape');
  });
});

test.describe('loading standard (200 ms)', () => {
  async function slowChat(page: Page, ms: number) {
    await page.route(/\/api\/admin\/chat(\?|$)/, async (route) => {
      await new Promise((r) => setTimeout(r, ms));
      await route.continue();
    });
  }

  test('a quick load shows nothing; a slow one shows the indicator with words, centred', async ({ page }) => {
    // The page's timers run on a paused clock (the loading `Delay` is a
    // setTimeout), so the 200 ms is stepped, not raced against round trips;
    // the chat response is held until the indicator has been checked.
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route(/\/api\/admin\/chat(\?|$)/, async (route) => {
      await held;
      await route.continue();
    });
    await page.clock.install();
    await page.clock.pauseAt(Date.now() + 1000);
    await page.goto(`${ADMIN}/chat?sw=off`);
    const state = page.locator('.loading-state');
    // Under 200 ms: the status region is already there, empty.
    const region = state.locator('.loading-state__body[role="status"]');
    await expect(region).toHaveCount(1);
    await page.clock.runFor(150);
    expect((await region.textContent())?.trim()).toBe('');
    await page.clock.runFor(100);
    const body = state.getByRole('status');
    await expect(body).toHaveText('Loading interactions…');
    await expect(body.locator('.xp-loading__shape')).toBeVisible();
    expect(await body.locator('.xp-loading__shape').evaluate((el) => getComputedStyle(el).animationName)).toBe('xp-morph');
    release();
    await page.clock.resume();
    await expect(state).toHaveCount(0);
  });

  test('reduced motion: a still indicator', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await slowChat(page, 900);
    await page.goto(`${ADMIN}/chat?sw=off`);
    const shape = page.locator('.loading-state .xp-loading__shape');
    await expect(shape).toBeVisible();
    expect(await shape.evaluate((el) => getComputedStyle(el).animationName)).toBe('none');
    await page.locator('.loading-state').screenshot({ path: 'e2e/.captures/synthetic/motion-loading-reduced.png' });
  });

  test('experiments off: the words alone', async ({ page }) => {
    await slowChat(page, 900);
    await page.goto(`${ADMIN}/chat?sw=off&experiments=off`);
    await expect(page.locator('.loading-state').getByRole('status')).toHaveText('Loading interactions…');
    await expect(page.locator('.loading-state .xp-loading')).toHaveCount(0);
  });
});

test.describe('planner overshoot (Experiment E, opt-in)', () => {
  test('off by default; ?overshoot=on pops the lifted card', async ({ page }) => {
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('html')).toHaveAttribute('data-overshoot', 'off');
    const card = page.locator('[data-run="r-carling"]');
    await page.locator('[data-handle="r-carling"]').focus();
    await page.keyboard.press('m');
    await expect(card).toHaveClass(/plan-card--lifted/);
    expect(await card.evaluate((el) => getComputedStyle(el).animationName)).not.toBe('xp-pickup');
    await page.keyboard.press('Escape');

    await page.goto(`${ADMIN}/?sw=off&overshoot=on`);
    await expect(page.locator('html')).toHaveAttribute('data-overshoot', 'on');
    await page.locator('[data-handle="r-carling"]').focus();
    await page.keyboard.press('m');
    await expect(card).toHaveClass(/plan-card--lifted/);
    expect(await card.evaluate((el) => getComputedStyle(el).animationName)).toBe('xp-pickup');
    await page.keyboard.press('Escape');
    // The choice is remembered; turn it off again for later tests in this context.
    await page.goto(`${ADMIN}/?sw=off&overshoot=off`);
    await expect(page.locator('html')).toHaveAttribute('data-overshoot', 'off');
  });
});
