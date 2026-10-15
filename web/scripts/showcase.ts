// The core primitives showcase (web/showcase/): bun run showcase
// Builds it into apps/admin/dist/__showcase/ (run `bun run build` first),
// where the e2e server (`bun run serve:dev`) serves it at
// http://127.0.0.1:4173/__showcase/ under the admin origin's production CSP.
// Dev-only: `bun run build` empties dist again, so a normal build never ships
// it. Check it with `KANADE_SHOWCASE=1 bunx playwright test e2e/showcase.spec.ts`.
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { build, type Plugin } from 'vite';
import { stripFidelityTags, themeBoot } from '../packages/ui/src/vite';

const web = join(import.meta.dir, '..');
const root = join(web, 'showcase');
const dist = join(web, 'apps/admin/dist');
if (!existsSync(join(dist, 'index.html'))) {
  console.error('showcase: build the admin app first (bun run build)');
  process.exit(1);
}

// The page belongs to no workspace package: its bare imports (svelte, the
// @kanade packages) resolve as the admin app's do.
const fromAdmin: Plugin = {
  name: 'showcase-resolve-from-admin',
  enforce: 'pre',
  resolveId(source, importer, options) {
    if (!importer?.startsWith(root) || /^[./\0]/.test(source)) return null;
    return this.resolve(source, join(web, 'apps/admin/package.json'), { ...options, skipSelf: true });
  },
};

await build({
  configFile: false,
  root,
  base: '/__showcase/',
  logLevel: 'warn',
  plugins: [fromAdmin, themeBoot(), stripFidelityTags(), svelte({ configFile: false, compilerOptions: { fragments: 'tree', css: 'external' } })],
  build: { outDir: join(dist, '__showcase'), emptyOutDir: true, target: 'es2023', assetsInlineLimit: 0 },
});
console.log('showcase: apps/admin/dist/__showcase/ (the e2e server serves it at /__showcase/)');
