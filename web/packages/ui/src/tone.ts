import type { RunStatus } from '@kanade/api-types';

/**
 * Semantic colour profiles for state pills (pwa-design-guidelines: colour is
 * semantic, the word always rides along). Boss difficulties keep their own
 * palette; these are for states only.
 */
export type Tone = 'success' | 'warning' | 'danger' | 'info' | 'neutral';

export const RUN_TONE: Record<RunStatus, Tone> = {
  planned: 'warning',
  confirmed: 'success',
  at_risk: 'danger',
  otot: 'info',
  done: 'neutral',
  cancelled: 'neutral',
};

export const CHECK_TONE: Record<'ok' | 'warning' | 'error', Tone> = { ok: 'success', warning: 'warning', error: 'danger' };
