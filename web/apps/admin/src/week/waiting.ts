import type { Run } from '@kanade/api-types';

/** One member who still owes answers in the shown week. */
export interface Owed {
  id: string;
  name: string;
  /** Runs they have not answered (or only answered maybe), in board order. */
  runs: { id: string; title: string; maybe: boolean }[];
}

const LIVE = (run: Run) => run.status !== 'done' && run.status !== 'cancelled';

/**
 * "Still waiting, by member, most first" (B_WeekAnswers): every member with a
 * waiting or maybe answer on a run that is still ahead, most owed first, then
 * by name. Runs come in the order given (sort them first).
 */
export function owedByMember(runs: Run[], title: (run: Run) => string): Owed[] {
  const byId = new Map<string, Owed>();
  for (const run of runs.filter(LIVE)) {
    for (const person of run.participants) {
      if (person.answer !== 'waiting' && person.answer !== 'maybe') continue;
      const entry = byId.get(person.id) ?? { id: person.id, name: person.name, runs: [] };
      entry.runs.push({ id: run.id, title: title(run), maybe: person.answer === 'maybe' });
      byId.set(person.id, entry);
    }
  }
  return [...byId.values()].sort((a, b) => b.runs.length - a.runs.length || a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}

/** "3/4 · 1 open": who is on, the party size, and the places still open (full when none). */
export function openPlaces(run: Pick<Run, 'tally'>): string {
  const open = Math.max(0, run.tally.total - run.tally.on);
  return open ? `${open} open` : 'full';
}
