import type { SetupView } from '@kanade/api-types';
import { Resource } from '../resource.svelte';

/**
 * `GET /api/admin/setup`, shared by the Setup page and the nav badge: read
 * once when the app starts, again on visiting Setup, its Refresh and after a
 * Config save. Never polled (each answer may wait up to 5 s on Discord, the
 * gateway and module health).
 */
export const setupStatus = new Resource<SetupView>('/api/admin/setup');

/** Required items not `ok` yet; 0 before the first answer, so no badge flashes while it loads. */
export const requiredLeft = (view: SetupView | null): number => view?.items.filter((item) => item.required && item.state !== 'ok').length ?? 0;
