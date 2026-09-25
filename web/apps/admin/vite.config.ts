import { svelte } from '@sveltejs/vite-plugin-svelte';
import { themeBoot } from '@kanade/ui/vite';
import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig({
  plugins: [
    themeBoot(),
    svelte(),
    VitePWA({
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.ts',
      // Registration is ours (@kanade/ui registerServiceWorker): Trusted Types
      // needs a policy around the script URL, and an inline register script is banned.
      injectRegister: false,
      manifest: {
        id: '/?app=kanade-admin',
        name: 'Kanade Admin',
        short_name: 'Kanade Admin',
        description: 'Plan and reschedule the boss week.',
        start_url: '/',
        scope: '/',
        display: 'standalone',
        background_color: '#232735',
        theme_color: '#5f6579',
        icons: [
          { src: '/icons/admin-192.png', sizes: '192x192', type: 'image/png' },
          { src: '/icons/admin-512.png', sizes: '512x512', type: 'image/png' },
          { src: '/icons/admin-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
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
  },
});
