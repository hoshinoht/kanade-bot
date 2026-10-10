import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  // Compiles `.svelte.ts` rune modules (the admin store) for unit tests.
  plugins: [svelte({ configFile: false, compilerOptions: { css: 'external' } })],
  test: {
    include: ['packages/*/test/**/*.test.ts', 'apps/*/test/**/*.test.ts'],
    environment: 'node',
  },
});
