import { ADMIN, PUBLIC, expect, test, HEADING } from './support';

// First-load transfer by type (SW off, cold cache): what a reader actually downloads.
for (const [app, origin] of [
  ['public', PUBLIC],
  ['admin', ADMIN],
] as const) {
  test(`${app}: first-load requests stay same-origin; bytes by type`, async ({ page }) => {
    const seen: { url: string; type: string; bytes: number }[] = [];
    page.on('requestfinished', async (request) => {
      const sizes = await request.sizes();
      seen.push({ url: request.url(), type: request.resourceType(), bytes: sizes.responseBodySize + sizes.responseHeadersSize });
    });
    await page.goto(`${origin}/?sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING[app]);
    await page.evaluate(() => document.fonts.ready);
    await page.waitForLoadState('networkidle');
    expect(seen.filter((r) => !r.url.startsWith(origin))).toEqual([]);
    const byType: Record<string, { n: number; kb: string }> = {};
    for (const r of seen) {
      const entry = (byType[r.type] ??= { n: 0, kb: '0' });
      entry.n += 1;
      entry.kb = (Number(entry.kb) + r.bytes / 1024).toFixed(1);
    }
    console.log(`${app} first load (on the wire, uncompressed by the mock server):`, JSON.stringify(byType));
    console.log(`${app} fonts:`, seen.filter((r) => r.type === 'font').map((r) => r.url.split('/').pop()).join(', '));
  });
}
