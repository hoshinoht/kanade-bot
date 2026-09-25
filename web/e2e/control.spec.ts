import { ADMIN, expect, test, HEADING } from './support';

// Proves the harness can fail: an inline style attribute (enforced policy) and
// an HTML string sink (report-only Trusted Types) must both be reported.
test.use({ cspControl: true });

test('control: deliberate violations are detected and reported', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);
  const events = await page.evaluate(async () => {
    document.body.setAttribute('style', 'outline: 1px solid red');
    try {
      // eslint-disable-next-line no-restricted-properties -- the deliberate violation under test
      document.createElement('div').innerHTML = '<b>x</b>';
    } catch {
      // Report-only: the assignment proceeds and is only reported.
    }
    await new Promise((r) => setTimeout(r, 200));
    return (window as unknown as { __csp: { directive: string; disposition: string }[] }).__csp;
  });
  expect(events).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ directive: 'style-src-attr', disposition: 'enforce' }),
      expect.objectContaining({ directive: 'require-trusted-types-for', disposition: 'report' }),
    ]),
  );
});
