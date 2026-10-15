import type { FixedChoices, FixedRow } from '@kanade/api-types';
import type { Outcome } from '@kanade/ui';
import { send } from '../resource.svelte';

/**
 * The runs a save of this day and time needs an update/keep decision for, as
 * the server's save decides them: runs off the weekly slot whose own day and
 * time differ from the new ones. A save that keeps the day and time asks
 * nothing (no request), and a run changed only in its roster is never listed.
 */
export async function runsToChoose(row: FixedRow, weekday: number, time: string): Promise<Outcome<FixedRow['runs']>> {
  if (weekday === row.weekday && time.trim() === row.time) return { ok: true, value: [] };
  const path = `/api/admin/fixed/${encodeURIComponent(row.id)}/choices`;
  const result = await send((c) => c.post<FixedChoices>(path, { weekday, time: time.trim() }));
  return result.ok ? { ok: true, value: result.value.runs } : result;
}
