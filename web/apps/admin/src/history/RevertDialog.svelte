<!--
  One dialog for the three rollbacks in docs/notes/history.md: revert records,
  restore a week to a point, revert one actor's changes. It always previews
  first; strict apply is offered when nothing conflicts, otherwise the
  conflict report is shown and Force needs an explicit acknowledgement. A
  strict refusal plans no rows, so a forced preview says what Force would do.
-->
<script lang="ts">
  import type { RevertPlan, RollbackMode, RowChange } from '@kanade/api-types';
  import { Modal } from '@kanade/ui';
  import { send } from '../resource.svelte';
  import { describe, reminderKind, type Names } from './describe';

  let {
    open = $bindable(false),
    title,
    path,
    body,
    names,
    timezone,
    ondone,
    returnFocus,
  }: {
    open: boolean;
    title: string;
    /** `/api/admin/history/revert`, `/restore-week` or `/revert-actor`. */
    path: string;
    body: Record<string, unknown>;
    names: Names;
    timezone: string;
    ondone: (plan: RevertPlan) => void;
    returnFocus?: () => HTMLElement | null;
  } = $props();

  let plan = $state<RevertPlan | null>(null);
  /** What Force would change, when the strict plan was refused. */
  let forced = $state<RevertPlan | null>(null);
  let error = $state('');
  let busy = $state(false);
  let acknowledged = $state(false);
  let requested: string | null = null;

  async function run(mode: RollbackMode) {
    busy = true;
    const result = await send((c) => c.post<RevertPlan>(path, { ...body, ...mode }));
    busy = false;
    if (!result.ok) {
      error = result.message;
      return null;
    }
    error = '';
    return result.value;
  }

  // Preview whenever the dialog opens on a new request.
  $effect(() => {
    const key = `${path} ${JSON.stringify(body)}`;
    if (open && requested !== key) {
      requested = key;
      plan = null;
      forced = null;
      acknowledged = false;
      void preview();
    }
    if (!open) requested = null;
  });

  async function preview() {
    const strict = await run({ preview: true });
    plan = strict;
    forced = strict?.outcome === 'conflicts' ? await run({ preview: true, force: true }) : null;
  }

  // The client's Idempotency-Key is the request id; a body `request_id` that differed would be refused.
  async function apply(force: boolean) {
    const result = await run({ force });
    if (!result) return;
    if (result.outcome === 'conflicts') {
      plan = result;
      acknowledged = false;
      forced = await run({ preview: true, force: true });
      return;
    }
    open = false;
    ondone(result);
  }

  const lines = (rows: RowChange[]) =>
    describe(
      { format: 'kanade.change.v1', seq: 0, id: '', revision: 0, at: '', actor: { kind: 'admin', id: '' }, surface: 'rollback', request_id: null, weeks: [], rows, notices: [], refs: [], prev_hash: '', hash: '' },
      names,
      timezone,
    );
  const refused = $derived(plan?.outcome === 'conflicts');
  /** Rows to show: the plan's own, or on a strict refusal what Force would change. */
  const changes = $derived(refused ? (forced?.rows ?? []) : (plan?.rows ?? []));
  type Conflict = RevertPlan['conflicts'][number];
  const bossesOf = (row: unknown) => (row && typeof row === 'object' && Array.isArray((row as { bosses?: unknown }).bosses) ? (row as { bosses: string[] }).bosses.join(' + ') : '');
  // Name the row as people know it: bosses for runs and timings, the card for reminders.
  function conflictText(c: Conflict, all: Conflict[]): string {
    const row = (c.found ?? c.expected) as Record<string, unknown> | null;
    const runName = (id: string) => bossesOf(all.find((o) => 'id' in o.key && o.key.table === 'runs' && o.key.id === id)?.found) || `run ${id}`;
    if (!('id' in c.key)) return `${names(c.key.user_id)}'s answer on ${runName(c.key.run_id)}`;
    if (c.key.table === 'reminders') return `${reminderKind(String(row?.kind ?? ''))} for ${runName(String(row?.run_id ?? ''))}`;
    const name = bossesOf(row) || c.key.id;
    return c.key.table === 'fixed_runs' ? `weekly timing ${name}` : name;
  }
</script>

  <Modal bind:open {title} eyebrow="History" narrow {returnFocus}>
  {#if error}<p class="flash flash--error" role="alert">{error}</p>{/if}
  {#if !plan}
    <p class="note" aria-busy="true">Working out what would change…</p>
  {:else if plan.outcome === 'unchanged'}
    <p>Nothing to do: everything is already as it was.</p>
  {:else}
    <p>
      Reverts {plan.reverts.map((s) => `#${s}`).join(', ')}; the result is a new change that refers to
      {plan.reverts.length === 1 ? 'it' : 'them'}. Nothing already sent is sent again.
    </p>
    {#if changes.length}
      <h3 class="pane__section">{refused ? 'Forcing it would change' : 'Would change'}</h3>
      <ul class="plan">{#each lines(changes) as line, i (i)}<li>{line}</li>{/each}</ul>
    {/if}
    {#if plan.skipped.length}
      <p class="note">Left alone ({plan.skipped[0]?.reason}): {plan.skipped.length} row{plan.skipped.length === 1 ? '' : 's'}.</p>
    {/if}
    {#if plan.conflicts.length}
      <div class="flash flash--error" role="alert">
        <strong>Changed again since.</strong> A strict revert is refused because {plan.conflicts.length}
        row{plan.conflicts.length === 1 ? ' no longer matches' : 's no longer match'} what the change left:
        <ul>
          {#each plan.conflicts as c, i (i)}<li>#{c.seq}: {conflictText(c, plan.conflicts)}</li>{/each}
        </ul>
      </div>
      <label class="ack">
        <input type="checkbox" bind:checked={acknowledged} />
        Force it: put the recorded values back and overwrite those later changes.
      </label>
    {/if}
  {/if}
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    {#if plan && plan.outcome !== 'unchanged'}
      {#if plan.conflicts.length}
        <button class="btn btn--primary" type="button" disabled={busy || !acknowledged} onclick={() => void apply(true)}>Force revert</button>
      {:else}
        <button class="btn btn--primary" type="button" disabled={busy} onclick={() => void apply(false)}>Revert</button>
      {/if}
    {/if}
  {/snippet}
</Modal>

<style>
  .plan {
    margin: 0;
    padding-left: 1.2rem;
  }

  .ack {
    display: flex;
    align-items: flex-start;
    gap: 0.4rem;
    margin-top: 0.6rem;
  }
</style>
