<!--
  One dialog for the three rollbacks in docs/v5/history.md: revert records,
  restore a week to a point, revert one actor's changes. It always previews
  first; strict apply is offered when nothing conflicts, otherwise the
  conflict report is shown and Force needs an explicit acknowledgement.
-->
<script lang="ts">
  import type { RevertPlan, RollbackMode, RowChange } from '@kanade/api-types';
  import { Modal } from '@kanade/ui';
  import { send } from '../resource.svelte';
  import { describe, type Names } from './describe';

  let {
    open = $bindable(false),
    title,
    path,
    body,
    names,
    ondone,
  }: {
    open: boolean;
    title: string;
    /** `/api/admin/history/revert`, `/restore-week` or `/revert-actor`. */
    path: string;
    body: Record<string, unknown>;
    names: Names;
    ondone: (plan: RevertPlan) => void;
  } = $props();

  let plan = $state<RevertPlan | null>(null);
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
      acknowledged = false;
      void run({ preview: true }).then((p) => (plan = p));
    }
    if (!open) requested = null;
  });

  async function apply(force: boolean) {
    const result = await run({ force, request_id: crypto.randomUUID() });
    if (!result) return;
    if (result.outcome === 'conflicts') {
      plan = result;
      return;
    }
    open = false;
    ondone(result);
  }

  const lines = (rows: RowChange[]) =>
    describe({ format: 'kanade.change.v1', seq: 0, id: '', revision: 0, at: '', actor: { kind: 'admin', id: '' }, surface: 'rollback', request_id: null, weeks: [], rows, notices: [], refs: [], prev_hash: '', hash: '' }, names);
  const keyText = (key: RevertPlan['conflicts'][number]['key']) =>
    'id' in key ? `${key.table} ${key.id}` : `answer ${names(key.user_id)} on ${key.run_id}`;
</script>

<Modal bind:open {title} eyebrow="History" narrow>
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
    <h3 class="pane__section">Would change</h3>
    <ul class="plan">{#each lines(plan.rows) as line, i (i)}<li>{line}</li>{/each}</ul>
    {#if plan.skipped.length}
      <p class="note">Left alone ({plan.skipped[0]?.reason}): {plan.skipped.length} row{plan.skipped.length === 1 ? '' : 's'}.</p>
    {/if}
    {#if plan.conflicts.length}
      <div class="flash flash--error" role="alert">
        <strong>Changed again since.</strong> A strict revert is refused because {plan.conflicts.length}
        row{plan.conflicts.length === 1 ? ' no longer matches' : 's no longer match'} what the change left:
        <ul>
          {#each plan.conflicts as c, i (i)}<li>#{c.seq}: {keyText(c.key)}</li>{/each}
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
