import { ADMIN, expect, test, HEADING } from './support';

// Proves the harness can fail: an inline style attribute and an HTML string
// sink (Trusted Types) are both blocked by the enforced policy and reported.
test.use({ cspControl: true });

test('control: deliberate violations are detected and reported', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);
  const result = await page.evaluate(async () => {
    document.body.setAttribute('style', 'outline: 1px solid red');
    let sinkThrew = false;
    try {
      // eslint-disable-next-line no-restricted-properties -- the deliberate violation under test
      document.createElement('div').innerHTML = '<b>x</b>';
    } catch (e) {
      sinkThrew = e instanceof TypeError;
    }
    const { promise: waited, resolve } = Promise.withResolvers<void>();
    setTimeout(resolve, 200);
    await waited;
    // `__csp` is installed by the csp fixture's init script (support.ts `watch`).
    const recorded = window as unknown as { __csp: { directive: string; disposition: string }[] };
    return { sinkThrew, events: recorded.__csp };
  });
  expect(result.sinkThrew, 'enforced Trusted Types refuse a raw HTML string').toBe(true);
  expect(result.events).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ directive: 'style-src-attr', disposition: 'enforce' }),
      expect.objectContaining({ directive: 'require-trusted-types-for', disposition: 'enforce' }),
    ]),
  );
});
