import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Live bug: a pasted multi-line problem statement made one Chat log row fill
// the screen. Free text in a log row is a two-line preview; the turn page keeps it whole.

const ID = 'c-move';
const TAIL = 'Return the answer in any order, and explain the complexity.';
const LONG = [
  'can you solve this leetcode problem for me',
  '',
  '1. Two Sum',
  '',
  'Given an array of integers nums and an integer target, return indices of the two numbers such that they add up to target.',
  '',
  '```py',
  'class Solution:',
  '    def twoSum(self, nums: List[int], target: int) -> List[int]:',
  '        pass',
  '```',
  '',
  ...Array.from({ length: 16 }, (_, i) => `Example ${i + 1}: Input: nums = [2,7,11,15], target = 9    Output: [0,1]   Explanation: nums[0] + nums[1] == 9.`),
  '',
  TAIL,
].join('\n');

async function longQuestion(page: Page) {
  await page.route(/\/api\/admin\/chat(\?.*)?$/, async (route) => {
    const res = await route.fetch();
    const body = (await res.json()) as { rows: { id: string; asked: string }[] };
    for (const row of body.rows) if (row.id === ID) row.asked = LONG;
    await route.fulfill({ response: res, json: body });
  });
  await page.route(`${ADMIN}/api/admin/chat/${ID}`, async (route) => {
    const res = await route.fetch();
    const body = (await res.json()) as { asked: string };
    body.asked = LONG;
    await route.fulfill({ response: res, json: body });
  });
}

for (const [label, width, height] of [
  ['desktop', 1280, 800],
  ['phone', 390, 844],
] as const) {
  test(`chat log: a ~2,000-character question is a two-line preview (${label})`, async ({ page }) => {
    expect(LONG.length).toBeGreaterThan(1900);
    await page.setViewportSize({ width, height });
    await longQuestion(page);
    await page.goto(`${ADMIN}/chat?sw=off`);
    const table = page.getByRole('table', { name: /Chatbot interactions/ });
    const link = table.locator(`a[href="/chat/${ID}"]`);
    await expect(link).toContainText('can you solve this leetcode problem for me 1. Two Sum');
    // Fences and newlines are gone from the preview; the tooltip keeps the whole question.
    await expect(link).not.toContainText('```');
    expect(await link.getAttribute('title')).toBe(LONG);

    const box = await link.evaluate((el) => {
      const cs = getComputedStyle(el);
      const line = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.5;
      return { height: el.getBoundingClientRect().height, line, clamp: cs.webkitLineClamp, overflow: cs.overflow, scroll: el.scrollHeight };
    });
    expect(box.clamp).toBe('2');
    expect(box.overflow).toBe('hidden');
    expect(box.height).toBeLessThanOrEqual(box.line * 2 + 1);
    // The text runs past the two lines: the clamp is what stops it (the ellipsis).
    expect(box.scroll).toBeGreaterThan(box.height + 1);

    // The row stays bounded: the two-line preview plus cell padding.
    const row = link.locator('xpath=ancestor::tr');
    const rowHeight = (await row.boundingBox())!.height;
    expect(rowHeight).toBeLessThanOrEqual(box.line * 2 + 64);
    expect(rowHeight).toBeLessThan(height * 0.2);

    // Wait for the shell's entry transform before measuring CSS-pixel targets.
    await page.waitForTimeout(350);
    // The small copy affordances meet WCAG 2.2 without widening the dense table.
    const targets = await table.locator('.log__who button.name--copy').evaluateAll((buttons) =>
      buttons.map((button) => {
        const { x, y, width, height } = button.getBoundingClientRect();
        return { x, y, right: x + width, bottom: y + height, width, height };
      }),
    );
    expect(targets.length).toBeGreaterThan(0);
    for (const target of targets) {
      expect(target.width).toBeGreaterThanOrEqual(24);
      expect(target.height).toBeGreaterThanOrEqual(24);
    }
    for (let i = 0; i < targets.length; i += 1) {
      for (let j = i + 1; j < targets.length; j += 1) {
        const a = targets[i]!;
        const b = targets[j]!;
        const overlaps = a.x < b.right && b.x < a.right && a.y < b.bottom && b.y < a.bottom;
        expect(overlaps, 'copy targets do not overlap').toBe(false);
      }
    }

    const documentSize = await page.evaluate(() => ({
      width: document.documentElement.scrollWidth,
      height: document.documentElement.scrollHeight,
    }));
    expect(documentSize.width).toBeLessThanOrEqual(width);
    expect(documentSize.height).toBeLessThanOrEqual(height);

    // Like a11y.spec: no serious or critical findings.
    const scan = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
    const bad = scan.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical' || v.id === 'target-size');
    expect(bad.map((v) => `${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);

    await link.click();
    await expect(page).toHaveURL(`${ADMIN}/chat/${ID}`);
    const asked = page.locator('p').filter({ hasText: 'can you solve this leetcode problem' });
    await expect(asked).toContainText(TAIL);
    await expect(asked).toContainText('```py');
    await expect(asked).toContainText('Example 16:');
  });
}
