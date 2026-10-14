/** @type {import('@sveltejs/vite-plugin-svelte').SvelteConfig} */
export default {
  compilerOptions: {
    // Build fragments with createElement instead of <template>.innerHTML, so no
    // Trusted Types sink (or pass-through policy) is needed under
    // require-trusted-types-for 'script'.
    fragments: 'tree',
    // Component CSS is emitted as files Vite bundles; style-src has no 'unsafe-inline'.
    css: 'external',
  },
};
