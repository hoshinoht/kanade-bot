import { untrack } from 'svelte';

/**
 * Keeps a section's draft on the saved settings while it is untouched (as
 * `IdListCard` does): when the saved values change underneath it (a live
 * refresh, another admin's save, this section's own save) and the draft
 * still equals what it was seeded from, it is seeded again; an edit in
 * progress is kept, and Save meets the server's answer as before.
 *
 * Call during component setup. `saved` reads the props (tracked) and must
 * return a fresh value in the draft's shape; `draft` reads the draft as it
 * stands; `seed` replaces it.
 */
export function followSaved<T>(saved: () => T, draft: () => unknown, seed: (value: T) => void): void {
  let base = untrack(() => JSON.stringify(saved()));
  $effect(() => {
    const next = saved();
    const key = JSON.stringify(next);
    untrack(() => {
      if (key === base) return;
      const untouched = JSON.stringify($state.snapshot(draft())) === base;
      base = key;
      if (untouched) seed(next);
    });
  });
}
