/**
 * Modules the public build must never contain (checked by `forbidModules`).
 * Add any @kanade/ui file that only the admin app uses.
 */
export const ADMIN_ONLY_MODULES = [
  '/apps/admin/',
  '/@dnd-kit/',
  '/uplot/',
  '/packages/ui/src/components/CommandPalette.svelte',
];
