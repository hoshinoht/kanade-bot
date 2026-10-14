import type { ContextSource, EffectiveContext, ModelInfo } from '@kanade/api-types';

/** The server's hard limit for every window, cap and reserve. */
export const MAX_CONTEXT_TOKENS = 131_072;
/** Each model call's token budget (prompt estimate + reply reserve), as the server's runner checks it. */
export const CALL_TOKEN_BUDGET = 16_384;
/** Of that budget, what a reserve must leave for the prompt (the server's `PROMPT_FLOOR_TOKENS`). */
export const PROMPT_FLOOR_TOKENS = 1_024;
/** The largest reply reserve the server accepts: one below budget − floor. */
export const MAX_RESERVE = CALL_TOKEN_BUDGET - PROMPT_FLOOR_TOKENS - 1;

/** The reserve fields' help: the effective limit and why. */
export function reserveHelp(): string {
  return `At most ${tokens(MAX_RESERVE)}: each call's token budget is ${tokens(CALL_TOKEN_BUDGET)}, and at least ${tokens(PROMPT_FLOOR_TOKENS)} of it stays for the prompt.`;
}

/** Past this a local route warns (never blocks). */
export const LOCAL_WARNING_TOKENS = 16_384;
/** The server's own notice text, shown inline beside the slider. */
export const LOCAL_WARNING = 'Context past 16k may result in degraded performance on local models.';

const FIRST_STOP = 64;

/**
 * Slider stops: quarter steps between powers of two (…, 8192, 10240, 12288,
 * 14336, 16384, …), so a keyboard arrow moves a useful amount at every scale
 * and 16384 is always a stop. `max` itself is the last stop even when it is
 * not a round number (a published window). The number field takes any value.
 */
export function tokenStops(max: number): number[] {
  const stops: number[] = [];
  for (let base = FIRST_STOP; base <= max; base *= 2)
    for (const step of [1, 1.25, 1.5, 1.75]) {
      const value = base * step;
      if (value < max) stops.push(value);
    }
  stops.push(max);
  return stops;
}

/** The slider position for a value: the last stop at or below it. */
export function stopIndex(stops: number[], value: number | null): number {
  if (value === null) return 0;
  let index = 0;
  for (let i = 0; i < stops.length; i++) if (stops[i]! <= value) index = i;
  return index;
}

/** As the server resolves it: a listed route that stays in the homelab. */
export function isLocal(model: ModelInfo | undefined): boolean {
  return model !== undefined && !model.leaves_homelab;
}

/** An override may not exceed the alias's published window, nor the hard limit. */
export function overrideMax(model: ModelInfo | undefined): number {
  return Math.min(model?.context_tokens ?? MAX_CONTEXT_TOKENS, MAX_CONTEXT_TOKENS);
}

export const SOURCE_LABELS: Record<ContextSource, string> = {
  override: 'Override',
  catalog: 'Published by Kanata',
  cloud_default: 'Cloud default',
  local_default: 'Local default',
};

/** Why the window or reserve is smaller than asked, in the server's own flags. */
export function clampNotes(context: EffectiveContext, savedReserve: number): string[] {
  const notes: string[] = [];
  if (context.clamped_by_published) notes.push('held to the published window');
  if (context.clamped_by_hard_cap) notes.push(`held to the ${tokens(MAX_CONTEXT_TOKENS)} limit`);
  if (context.clamped_by_role_cap) notes.push('held to the role cap');
  if (context.reserve < savedReserve) notes.push('reserve held to the max output');
  return notes;
}

export function tokens(value: number): string {
  return value.toLocaleString('en-US');
}
