/**
 * Run-sheet staging (B_RunSheetStage*): while on, the sheet's Move, Swap,
 * party, Reset and Status post draft operations instead of live writes.
 * App state, not the sheet's: it outlives a closed sheet and shows on every
 * run sheet until Stop staging (user decision, drafts README question 2).
 */
import type { DraftDetail, DraftOpBody, DraftWritten, Run, Week } from '@kanade/api-types';
import { runTitle, whenLabel, type Slot } from '@kanade/ui';
import { swapSlots } from '../planner/dropTime';
import { send } from '../resource.svelte';
import type { MoveOutcome } from '../store.svelte';
import { DRAFTS_PATH, changesLabel, draftPath } from './drafts';

export interface StagingDraft {
  id: string;
  title: string;
  version: number;
}

export class Staging {
  draft = $state<StagingDraft | null>(null);
  /** The draft as last read: its ops mark what this run already has in it. */
  detail = $state<DraftDetail | null>(null);
  busy = $state(false);

  /** Start staging into an existing draft (its version as listed) or a new one named `title`. */
  async start(pick: { draft: StagingDraft } | { title: string }): Promise<MoveOutcome> {
    if ('draft' in pick) {
      this.draft = { ...pick.draft };
      await this.reread();
      // Merged or discarded since the list was read: reread stopped staging.
      if (this.draft?.id !== pick.draft.id) return { ok: false, message: `${pick.draft.title} is closed; it can take no more changes.` };
      return { ok: true, message: `Staging into ${this.draft.title}.` };
    }
    const title = pick.title.trim();
    const made = await send((c) => c.post<DraftWritten>(DRAFTS_PATH, title ? { title } : {}));
    if (!made.ok) return { ok: false, message: `Couldn't start a draft: ${made.message}` };
    this.draft = { id: made.value.id, title: title || 'Untitled draft', version: made.value.version };
    await this.reread();
    if (this.draft?.id !== made.value.id) return { ok: false, message: 'The new draft closed before staging started.' };
    return { ok: true, message: `Staging into ${this.draft.title}.` };
  }

  stop(): void {
    this.draft = null;
    this.detail = null;
  }

  async reread(): Promise<void> {
    const draft = this.draft;
    if (!draft) return;
    const read = await send((c) => c.get<DraftDetail>(draftPath(draft.id)));
    if (!read.ok || this.draft?.id !== draft.id) return;
    // A draft closed elsewhere (merged, discarded) can take no more changes.
    if (read.value.status !== 'open') return this.stop();
    this.detail = read.value;
    this.draft = { id: draft.id, title: read.value.title, version: read.value.version };
  }

  /** The ops already staged on `runId`, in order. */
  opsFor(runId: string) {
    return this.detail?.ops.filter((op) => op.run_id === runId) ?? [];
  }

  /**
   * Post the ops one after another, each against the version the last one
   * returned. All or nothing: if a later op is refused, the earlier ones are
   * removed again (each op is appended, so its ord is the list length then).
   */
  async #stage(ops: DraftOpBody[], what: string): Promise<MoveOutcome> {
    if (!this.draft) return { ok: false, message: 'Not staging into a draft.' };
    this.busy = true;
    try {
      // Several ops need the list length at this version to know their ords.
      if (ops.length > 1 && this.detail?.version !== this.draft.version) await this.reread();
      const draft = this.draft;
      if (!draft) return { ok: false, message: 'Not staging into a draft.' };
      const length = this.detail?.version === draft.version ? this.detail.ops.length : null;
      if (ops.length > 1 && length === null) return { ok: false, message: `Couldn't stage ${what}: the draft couldn't be read.` };
      let version = draft.version;
      const added: number[] = [];
      for (const op of ops) {
        const done = await send((c) => c.post<DraftWritten>(draftPath(draft.id, '/ops'), { version, op }));
        if (!done.ok) {
          const undone = added.length === 0 || (await this.#unstage(draft.id, version, added));
          // A stale version (the draft changed in another tab): re-read so the next try lands.
          await this.reread();
          const label = changesLabel(added);
          const kept = undone ? '' : ` ${label[0]!.toUpperCase()}${label.slice(1)} stayed in ${draft.title}; remove ${added.length === 1 ? 'it' : 'them'} there.`;
          return { ok: false, message: `Couldn't stage ${what}: ${done.message}${kept}` };
        }
        added.push((length ?? 0) + added.length);
        version = done.value.version;
        if (this.draft?.id === draft.id) this.draft = { ...this.draft, version };
      }
      await this.reread();
      return { ok: true, message: `Staged ${what} in ${draft.title}; nothing is live until it is merged.` };
    } finally {
      this.busy = false;
    }
  }

  /** Remove the ops this call added, last first; false when a removal is refused. */
  async #unstage(id: string, version: number, ords: number[]): Promise<boolean> {
    for (const ord of [...ords].reverse()) {
      const undone = await send((c) => c.delete<DraftWritten>(draftPath(id, `/ops/${ord}?version=${version}`)));
      if (!undone.ok) return false;
      version = undone.value.version;
    }
    return true;
  }

  move(run: Run, to: Slot, week: Week): Promise<MoveOutcome> {
    const time = run.status === 'otot' ? null : to.time;
    return this.#stage([{ kind: 'move', run: run.id, day: to.day, time }], `${runTitle(run)} to ${whenLabel(week, to.day, time)}`);
  }

  /** A swap is two staged moves: each run to the other's slot, as the live swap places them. */
  swap(a: Run, b: Run, week: Week): Promise<MoveOutcome> {
    const to = swapSlots(a, a.status === 'otot', b, b.status === 'otot');
    return this.#stage(
      [
        { kind: 'move', run: a.id, day: to.a.day, time: a.status === 'otot' ? null : to.a.time },
        { kind: 'move', run: b.id, day: to.b.day, time: b.status === 'otot' ? null : to.b.time },
      ],
      `a swap of ${runTitle(a)} and ${runTitle(b)} (${whenLabel(week, to.a.day, to.a.time)} / ${whenLabel(week, to.b.day, to.b.time)})`,
    );
  }

  status(run: Run, status: string, label: string): Promise<MoveOutcome> {
    return this.#stage([{ kind: 'status', run: run.id, status }], `${runTitle(run)} as ${label.toLowerCase()}`);
  }

  party(run: Run, change: { add?: string; remove?: string }, name: string): Promise<MoveOutcome> {
    const op: DraftOpBody = { kind: 'party', run: run.id, add: change.add ? [change.add] : [], remove: change.remove ? [change.remove] : [] };
    return this.#stage([op], `${change.add ? `${name} on` : `${name} off`} ${runTitle(run)}`);
  }

  reset(run: Run): Promise<MoveOutcome> {
    return this.#stage([{ kind: 'reset', run: run.id }], `${runTitle(run)} back on its weekly timing`);
  }
}
