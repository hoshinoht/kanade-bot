/**
 * Exactly motion.spec.ts (not reduce-motion.spec.ts): it measures frame rates,
 * so both configs give it a project of its own on one worker.
 */
export const MOTION = /(^|[\\/])motion\.spec\.ts$/;

/**
 * Spec files (under `e2e/`) that run on the real-API e2e server
 * (`playwright.harness.config.ts`) instead of the dev mock
 * (`playwright.config.ts`, which skips them). The list goes away with the mock.
 */
export const PORTED: readonly string[] = [
  'bundle.spec.ts',
  'chat.spec.ts',
  'checkpoints.spec.ts',
  'control.spec.ts',
  'date-picker.spec.ts',
  'discord-links.spec.ts',
  'extractions.spec.ts',
  'fonts.spec.ts',
  'header-time.spec.ts',
  'history-config.spec.ts',
  'log-filters.spec.ts',
  'names.spec.ts',
  'pills.spec.ts',
  // Batch 2.
  'move-picker.spec.ts',
  'planner-card.spec.ts',
  'rewrites.spec.ts',
  'routing.spec.ts',
  'select-capture.spec.ts',
  'select.spec.ts',
  'showcase.spec.ts',
  'single-pane.spec.ts',
  'toasts.spec.ts',
  'tokens.spec.ts',
  'trace.spec.ts',
  'week-scroll.spec.ts',
  // Batch 3.
  'config-context.spec.ts',
  'config-privacy.spec.ts',
  'inbox-past.spec.ts',
  'layout.spec.ts',
  'log-clamp.spec.ts',
  'model-view.spec.ts',
  'network.spec.ts',
  'offline.spec.ts',
  'public-meta.spec.ts',
  'public-privacy.spec.ts',
  'quiet-mode.spec.ts',
  // Batch 4.
  'progress.spec.ts',
  'refresh.spec.ts',
  'run-identity-contrast.spec.ts',
  'states.spec.ts',
  'week-hover.spec.ts',
  // Batch 5: each found a real bug, now fixed.
  'a11y.spec.ts',
  'clipping.spec.ts',
  'config-layout.spec.ts',
];
