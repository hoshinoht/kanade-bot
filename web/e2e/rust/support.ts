import { expect, test as base, type Page } from '@playwright/test';

const portBase = Number(process.env.KANADE_E2E_PORT_BASE ?? '4373');
export const ADMIN = `http://127.0.0.1:${portBase + 50}`;
export const PUBLIC = `http://127.0.0.1:${portBase + 51}`;
export const ADMIN_TOKEN = 'e2e-break-glass-token-0123456789abcdef';

export interface ApiReply<T> {
  status: number;
  body: T;
  csrf: string | null;
}

interface CspSink {
  events: { directive: string; blocked: string }[];
}

/** The live Rust listener intentionally has no CSP-report sink; inspect DOM events instead. */
export const test = base.extend<{ csp: CspSink }>({
  csp: [
    async ({ page }, use) => {
      const sink: CspSink = { events: [] };
      await page.exposeFunction('__rustE2eCspViolation', (event: { directive: string; blocked: string }) => sink.events.push(event));
      await page.addInitScript(() => {
        document.addEventListener('securitypolicyviolation', (event) => {
          void (window as unknown as { __rustE2eCspViolation: (record: { directive: string; blocked: string }) => void }).__rustE2eCspViolation({
            directive: event.violatedDirective,
            blocked: event.blockedURI,
          });
        });
      });
      await use(sink);
      expect(sink.events, 'securitypolicyviolation events').toEqual([]);
    },
    { auto: true },
  ],
});

export async function signIn(page: Page): Promise<void> {
  await page.goto(`${ADMIN}/login`);
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Sign in with Tailscale' })).toHaveCount(0);
  const token = page.getByLabel('Admin token');
  await expect(token).toBeVisible();
  await token.fill(ADMIN_TOKEN);
  await page.getByRole('button', { name: 'Sign in with the token' }).click();
  await expect(page.locator('.account__name')).toHaveText('Break-glass token');
}

/** Runs in the document origin, so it uses the token-login browser cookie. */
export async function api<T>(page: Page, path: string, init: { method?: string; headers?: Record<string, string>; body?: string } = {}): Promise<ApiReply<T>> {
  return page.evaluate(async ({ path, init }) => {
    const response = await fetch(path, { ...init, credentials: 'same-origin' });
    const text = await response.text();
    return {
      status: response.status,
      body: (text ? JSON.parse(text) : null) as T,
      csrf: response.headers.get('x-kanade-csrf'),
    };
  }, { path, init });
}

export async function csrf(page: Page): Promise<Record<string, string>> {
  const session = await api<unknown>(page, '/api/admin/session');
  expect(session.status).toBe(200);
  const token = session.csrf;
  expect(token).toBeTruthy();
  return { 'X-Kanade-CSRF': token! };
}

export { expect };
