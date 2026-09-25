import { svelte } from '@sveltejs/vite-plugin-svelte';
import { forbidModules, themeBoot } from '@kanade/ui/vite';
import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';
import { ADMIN_ONLY_MODULES } from './admin-only.ts';

export default defineConfig({
  plugins: [
    themeBoot(),
    svelte(),
    forbidModules({ patterns: ADMIN_ONLY_MODULES }),
    VitePWA({
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.ts',
      // Registration is ours (@kanade/ui registerServiceWorker): Trusted Types
      // needs a policy around the script URL, and an inline register script is banned.
      injectRegister: false,
      manifest: {
        id: '/?app=kanade-public',
        name: 'Kanade — boss schedule',
        short_name: 'Kanade',
        description: 'This boss week at a glance: who is on, and when.',
        start_url: '/',
        scope: '/',
        display: 'standalone',
        background_color: '#eec75f',
        theme_color: '#5f6579',
        icons: [
          { src: '/icons/public-192.png', sizes: '192x192', type: 'image/png' },
          { src: '/icons/public-512.png', sizes: '512x512', type: 'image/png' },
          { src: '/icons/public-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
      },
      injectManifest: {
        // Static app assets only. API responses are never precached or runtime-cached.
        globPatterns: ['**/*.{js,css,html,woff2,png,webmanifest}'],
        rollupFormat: 'iife',
      },
      devOptions: { enabled: false },
    }),
  ],
  build: {
    target: 'es2023',
    // Never inline assets as data: URIs; font-src is 'self' only.
    assetsInlineLimit: 0,
    rollupOptions: { input: { main: 'index.html', offline: 'offline.html' } },
  },
});
