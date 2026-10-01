import type { BossRow, ConfigView, Difficulty } from '@kanade/api-types';

/** Bounds the server holds `run_lengths` to (docs/v5/admin-api.md, Config). */
export const DEFAULT_RANGE = { min: 5, max: 240 } as const;
export const OVERRIDE_RANGE = { min: 5, max: 480 } as const;

export type RunLengths = ConfigView['run_lengths'];

export interface OverrideDraft {
  boss: string;
  difficulty: Difficulty | '';
  minutes: number | null;
}

export interface RunLengthsDraft {
  default_minutes: number | null;
  overrides: OverrideDraft[];
}

export const draftOf = (saved: RunLengths): RunLengthsDraft => ({
  default_minutes: saved.default_minutes,
  overrides: saved.overrides.map((o) => ({ ...o })),
});

const whole = (n: number | null, range: { min: number; max: number }) => n !== null && Number.isInteger(n) && n >= range.min && n <= range.max;

/**
 * What is wrong with a draft before it is sent (the server says the same), or
 * the patch body. With the boss list loaded, each override must name a
 * catalog boss and a difficulty it has (kept unknown values are refused here).
 */
export function check(draft: RunLengthsDraft, catalog?: BossRow[] | null): { error: string; field?: 'default' | number } | { value: RunLengths } {
  if (!whole(draft.default_minutes, DEFAULT_RANGE)) {
    return { error: `The default run length is ${DEFAULT_RANGE.min}–${DEFAULT_RANGE.max} whole minutes.`, field: 'default' };
  }
  const seen = new Set<string>();
  for (const [index, o] of draft.overrides.entries()) {
    if (!o.boss || !o.difficulty) return { error: `Override ${index + 1}: pick a boss and a difficulty.`, field: index };
    if (catalog) {
      const boss = catalog.find((b) => b.key === o.boss);
      if (!boss) return { error: `Override ${index + 1}: “${o.boss}” is not in the boss list; pick a boss.`, field: index };
      if (!boss.difficulties.some((d) => d.letter === o.difficulty)) {
        return { error: `Override ${index + 1}: ${boss.name} has no “${o.difficulty}” difficulty; pick one it has.`, field: index };
      }
    }
    if (!whole(o.minutes, OVERRIDE_RANGE)) {
      return { error: `Override ${index + 1}: a run length is ${OVERRIDE_RANGE.min}–${OVERRIDE_RANGE.max} whole minutes.`, field: index };
    }
    const key = `${o.boss}/${o.difficulty}`;
    if (seen.has(key)) return { error: `Override ${index + 1}: that boss and difficulty already has one.`, field: index };
    seen.add(key);
  }
  return {
    value: {
      default_minutes: draft.default_minutes!,
      overrides: draft.overrides.map((o) => ({ boss: o.boss, difficulty: o.difficulty as Difficulty, minutes: o.minutes! })),
    },
  };
}
