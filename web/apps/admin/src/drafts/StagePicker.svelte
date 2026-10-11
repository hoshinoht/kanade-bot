<!--
  "Stage in draft…" (B_RunSheetStage, B_RunSheetStagePhone): pick one of the
  admin's open drafts for this boss week, or name a new one, then Start
  staging. Drafts that cannot hold a run change (weekly timings only,
  another boss week) are listed dashed and disabled with the reason.
-->
<script lang="ts">
  import type { DraftList } from '@kanade/api-types';
  import { Icon, PendingLabel, StatusChip } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import { DRAFTS_PATH, stageable } from './drafts';
  import type { StagingDraft } from './staging.svelte';

  let {
    week,
    busy = false,
    error = '',
    onstart,
    oncancel,
  }: {
    /** `Week.starts` of the run's boss week. */
    week: string;
    busy?: boolean;
    error?: string;
    onstart: (pick: { draft: StagingDraft } | { title: string }) => void;
    oncancel: () => void;
  } = $props();
  const uid = $props.id();

  const drafts = new Resource<DraftList>(`${DRAFTS_PATH}?state=open`, { topics: ['inbox'] });
  $effect(() => drafts.watch());
  const rows = $derived((drafts.data?.drafts ?? []).map((d) => ({ d, can: stageable(d, week) })));

  // The first draft that can take the change is picked; with none, New draft.
  let picked = $state('');
  let title = $state('');
  const choice = $derived(picked || (rows.find((r) => r.can.ok)?.d.id ?? 'new'));
  let titleField = $state<HTMLInputElement>();

  function start(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    if (choice === 'new') return onstart({ title });
    const row = rows.find((r) => r.d.id === choice);
    if (row?.can.ok) onstart({ draft: { id: row.d.id, title: row.d.title, version: row.d.version } });
  }
</script>

<form class="stagepick" onsubmit={start} aria-labelledby="{uid}-h">
  <div class="stagepick__head">
    <p class="cap">Stage edits on this run</p>
    <h3 class="stagepick__title" id="{uid}-h">Choose a draft</h3>
  </div>
  <div class="stagepick__options" role="radiogroup" aria-labelledby="{uid}-h">
    {#each rows as { d, can } (d.id)}
      <label class="stagepick__option" class:stagepick__option--on={choice === d.id} class:stagepick__option--off={!can.ok}>
        <input type="radio" name="{uid}-draft" value={d.id} checked={choice === d.id} disabled={!can.ok} onchange={() => (picked = d.id)} />
        <span class="stagepick__lines">
          <b>{d.title}</b>
          <span class="stagepick__meta">{#if can.ok}This boss week · {d.staged} staged{:else}{can.reason}{/if}</span>
          {#if can.ok && !d.up_to_date}<StatusChip tone="warn">Base moved</StatusChip>{/if}
        </span>
      </label>
    {/each}
    <label class="stagepick__option" class:stagepick__option--on={choice === 'new'}>
      <input
        type="radio"
        name="{uid}-draft"
        value="new"
        checked={choice === 'new'}
        onchange={() => {
          picked = 'new';
          titleField?.focus();
        }}
      />
      <span class="stagepick__lines stagepick__lines--grow">
        <b>New draft</b>
        <input
          class="stagepick__name"
          bind:this={titleField}
          bind:value={title}
          maxlength="200"
          aria-label="New draft name"
          placeholder="Name, e.g. Wednesday shuffle"
          onfocus={() => (picked = 'new')}
        />
      </span>
    </label>
  </div>
  {#if drafts.error && !drafts.data}<p class="field__error" role="alert">Couldn't load your drafts: {drafts.error}</p>{/if}
  <p class="stagepick__note">While staging, Move, Swap, party and status on this run go into the draft. Nothing is live until you merge it.</p>
  <p class="field__error" role="alert">{error}</p>
  <div class="stagepick__acts">
    <button class="btn" type="button" onclick={oncancel}>Cancel</button>
    <button class="btn btn--primary btn--key stagepick__go" type="submit" data-fid="sheet-stage-start" disabled={busy}
      ><Icon name="layers" /><PendingLabel pending={busy} label="Starting…">Start staging</PendingLabel></button
    >
  </div>
</form>
