import { getContext, setContext } from 'svelte';

/** One unsaved field: what the save bar names, and its saved and drafted values. */
export interface Change {
  label: string;
  from: string;
  to: string;
}

/** Lists the changes between a saved and a drafted value set, in field order. */
export function changes(fields: { label: string; from: unknown; to: unknown; show?: (v: unknown) => string }[]): Change[] {
  return fields
    .filter((f) => JSON.stringify(f.from) !== JSON.stringify(f.to))
    .map((f) => ({ label: f.label, from: (f.show ?? String)(f.from), to: (f.show ?? String)(f.to) }));
}

const KEY = Symbol('config-dirty');

/** Which section's save bars hold unsaved changes, so the contents list can mark them. */
export interface DirtyReport {
  set(id: string, dirty: boolean): void;
}

export function provideDirty(report: DirtyReport): void {
  setContext(KEY, report);
}

export function dirtyReport(): DirtyReport | undefined {
  return getContext<DirtyReport | undefined>(KEY);
}
