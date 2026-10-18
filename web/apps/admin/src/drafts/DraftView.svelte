<!--
  One draft (boards B_Drafts, B_DraftsConflict, B_PhoneDraftConflict): the
  head, the "Not live" note, the base-moved box, the staged changes with
  remove, then the side pane: what a merge would write (History's diff
  rows) or, once the base moved, the conflicts with Keep mine / Take theirs,
  and the decision card (Merge, or Rebase onto the head; Discard…). Phones
  stack it all in one scroller, conflicts first, the decision as a bar.
-->
<script lang="ts">
  import type { Boss, DraftConflict, DraftDetail, DraftPreviewView, DraftWritten, Run, RowChange, Week } from '@kanade/api-types';
  import { DecisionCard, Icon, PendingLabel, Portrait, StatusChip, Toaster, runTitle, type Outcome } from '@kanade/ui';
  import { tick } from 'svelte';
  import DiffRow from '../history/DiffRow.svelte';
  import { shortAt } from '../history/describe';
  import { Resource, send } from '../resource.svelte';
  import { answers, changeNo, changesLabel, closedLabel, draftPath, editedAt, flagsByOp, kindLabel, opFlag, removalLabel, settled, shortDate, type Answer } from './drafts';

  let {
    id,
    week,
    timeZone,
    bar,
    toaster,
    onchanged,
    onclosed,
    onadd,
    ondiscard,
  }: {
    id: string;
    /** The week on screen: run titles and portraits for the staged runs. */
    week: Week | null;
    timeZone: string;
    /** One pane (below 900 px): one scroller, conflicts first, the decision as a bar. */
    bar: boolean;
    toaster: Toaster;
    /** A write changed the draft: the list re-reads (staged count, base, edited). */
    onchanged: () => void;
    /** Merged or discarded: the page moves on to the next draft. */
    onclosed: (message: string) => void;
    /** "Add a change": stage into this draft from the Week board. */
    onadd: (draft: DraftDetail) => void;
    ondiscard: (draft: DraftDetail) => void;
  } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  const detail = new Resource<DraftDetail>(draftPath(id), { topics: ['inbox', 'schedule'] });
  // svelte-ignore state_referenced_locally
  const preview = new Resource<DraftPreviewView>(draftPath(id, '/preview'), { topics: ['inbox', 'schedule'] });
  $effect(() => detail.watch());
  $effect(() => preview.watch());

  const d = $derived(detail.data);
  const p = $derived(preview.data);
  const open = $derived(d?.status === 'open');
  const conflicts = $derived(p?.conflicts ?? []);
  const flags = $derived(flagsByOp(conflicts));
  const moved = $derived(Boolean(p && !p.up_to_date));
  const runOf = (runId: string | null): Run | undefined => (runId ? week?.runs.find((r) => r.id === runId) : undefined);
  const lead = $derived<Boss | null>(runOf(d?.ops.find((op) => op.run_id)?.run_id ?? null)?.bosses[0] ?? null);

  let busy = $state<'' | 'merge' | 'rebase' | 'remove' | 'rename'>('');
  let error = $state('');
  let picks = $state<Record<string, Answer>>({});
  const allSettled = $derived(settled(conflicts, picks));
  const settledCount = $derived(conflicts.filter((c) => picks[c.key]).length);

  /** One preview row per run: which staged changes made it. */
  const rowOps = (row: RowChange) => (d?.ops ?? []).filter((op) => 'id' in row.key && (op.run_id === row.key.id || op.fixed_id === row.key.id)).map((op) => op.ord);
  const rowTitle = (row: RowChange) => {
    if (!('id' in row.key)) return row.key.run_id;
    const run = runOf(row.key.id);
    const bosses = ((row.after ?? row.before)?.bosses as string[] | undefined) ?? [];
    return run ? runTitle(run) : bosses.join(' + ') || `${row.key.table}/${row.key.id}`;
  };
  const changedRuns = $derived(new Set((p?.rows ?? []).filter((r) => r.key.table === 'runs').map((r) => ('id' in r.key ? r.key.id : ''))).size);

  async function reload() {
    await Promise.all([detail.load(), preview.load()]);
  }

  /** A write's answer: re-read the draft, its preview and the list. */
  async function wrote(result: Outcome<DraftWritten>, verb: string): Promise<boolean> {
    if (!result.ok) {
      error = `Couldn't ${verb}: ${result.message}`;
      // Stale: someone else (another tab) changed it; show what is there now.
      if (result.status === 409) await reload();
      return false;
    }
    error = '';
    await reload();
    onchanged();
    return true;
  }

  let opList = $state<HTMLOListElement>();
  async function remove(ord: number) {
    if (!d || busy) return;
    busy = 'remove';
    const ok = await wrote(await send((c) => c.delete<DraftWritten>(draftPath(d.id, `/ops/${ord}?version=${d.version}`))), `remove change ${changeNo(ord)}`);
    busy = '';
    if (!ok) return;
    toaster.show({ message: `Removed change ${changeNo(ord)} from ${d.title}.`, tone: 'ok' });
    // Focus stays in the list: the row now in that place, else the last one, else the add button.
    await tick();
    const buttons = opList?.querySelectorAll<HTMLButtonElement>('.draftop__x') ?? [];
    (buttons[Math.min(ord, buttons.length - 1)] ?? addButton)?.focus({ preventScroll: true });
  }

  async function rebase() {
    if (!d || !p || busy || !allSettled) return;
    busy = 'rebase';
    const settle = Object.fromEntries(conflicts.map((c) => [c.key, picks[c.key]!]));
    const result = await send((c) => c.post<DraftWritten>(draftPath(d.id, '/rebase'), { version: d.version, settle }));
    const ok = await wrote(result, 'rebase');
    busy = '';
    if (ok && result.ok) {
      picks = {};
      toaster.show({ message: result.value.message, tone: 'ok' });
    }
  }

  async function merge() {
    if (!d || !p?.mergeable || busy) return;
    busy = 'merge';
    const result = await send((c) => c.post<DraftWritten>(draftPath(d.id, '/merge'), { version: d.version }));
    busy = '';
    if (!result.ok) {
      error = `Couldn't merge: ${result.message}`;
      if (result.status === 409) await reload();
      return;
    }
    onclosed(result.value.message);
  }

  // Rename: the title becomes a field in place; Enter saves, Escape keeps it.
  let renaming = $state(false);
  let title = $state('');
  let renameButton = $state<HTMLButtonElement>();
  let addButton = $state<HTMLButtonElement>();
  async function startRename() {
    if (!d) return;
    title = d.title;
    renaming = true;
    await tick();
    document.getElementById(`${uid}-title-field`)?.focus();
  }
  async function stopRename() {
    renaming = false;
    await tick();
    renameButton?.focus({ preventScroll: true });
  }
  async function rename(event: SubmitEvent) {
    event.preventDefault();
    if (!d || busy) return;
    if (!title.trim() || title.trim() === d.title) return void stopRename();
    busy = 'rename';
    const ok = await wrote(await send((c) => c.patch<DraftWritten>(draftPath(d.id), { version: d.version, title: title.trim() })), 'rename');
    busy = '';
    if (ok) await stopRename();
  }

  /** Base / Yours / Now as the run sheet words them: a slot instant reads "Tue 29 22:00". */
  const value = (text: string | null) => (text && /^\d{4}-\d{2}-\d{2}T/.test(text) ? shortAt(text, timeZone) : (text ?? '—'));
  const reason = $derived.by(() => {
    if (!p || !d) return '';
    if (!d.ops.length) return 'Stage a change before merging.';
    if (moved) return `the schedule moved since #${p.base}. Rebase to put your changes on #${p.head}.`;
    if (conflicts.length) return conflicts[0]!.message;
    return '';
  });
</script>

{#snippet conflictCard(c: DraftConflict)}
  {@const run = runOf(c.run_id)}
  {@const name = run ? runTitle(run) : (c.run_id ?? c.fixed_id ?? 'A change')}
  <div class="draftcfl" data-fid="drafts-conflict" role="group" aria-labelledby="{uid}-c-{c.key}">
    <div class="draftcfl__head">
      <b id="{uid}-c-{c.key}">{name}{c.kind === 'both_changed' && c.field ? ` · ${c.field}` : ''}</b>
      <StatusChip tone="risk">{c.kind === 'both_changed' ? 'Both changed' : c.kind === 'upstream_removed' ? removalLabel(c) : 'Can’t apply'}</StatusChip>
    </div>
    {#if c.kind === 'both_changed'}
      <dl class="draftcfl__grid">
        <div class="draftcfl__cell"><dt class="cap">Base #{p?.base}</dt><dd class="mono">{value(c.base)}</dd></div>
        <div class="draftcfl__cell draftcfl__cell--mine"><dt class="cap">Yours · {c.ops.map(changeNo).join(', ')}</dt><dd class="mono"><b>{value(c.yours)}</b></dd></div>
        <div class="draftcfl__cell"><dt class="cap" title="Now{c.record ? ` · #${c.record.seq} ${c.record.actor.name}` : ''}">Now{c.record ? ` · #${c.record.seq} ${c.record.actor.name}` : ''}</dt><dd class="mono">{value(c.now)}</dd></div>
      </dl>
    {:else if c.kind === 'upstream_removed'}
      <p class="draftcfl__text">
        {c.record ? `${c.record.actor.name} ${c.removal === 'cancelled' ? 'cancelled' : 'changed'} this run.` : c.message}
        {c.ops.length === 1 ? `Change ${changeNo(c.ops[0]!)} edits it, so it can’t apply.` : `Changes ${c.ops.map(changeNo).join(' and ')} edit it, so they can’t apply.`}
      </p>
    {:else}
      <p class="draftcfl__text">{c.message}</p>
    {/if}
    {#if c.settle.length}
      <div class="seg draftcfl__settle" role="group" aria-label="Settle {name}">
        {#each answers(c, bar) as a (a.value)}
          <button type="button" aria-pressed={picks[c.key] === a.value} disabled={Boolean(busy)} onclick={() => (picks = { ...picks, [c.key]: a.value })}>{a.label}</button>
        {/each}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet conflictsList()}
  {#each conflicts as c (c.key)}{@render conflictCard(c)}{/each}
{/snippet}

{#snippet rebaseKey()}
  <button
    class="btn btn--primary btn--key drafts__rebase"
    data-fid="decision-rebase"
    type="button"
    aria-disabled={!allSettled || Boolean(busy)}
    aria-describedby={allSettled ? undefined : `${uid}-settle`}
    onclick={() => void rebase()}
    ><Icon name="git-branch" /><PendingLabel pending={busy === 'rebase'} label="Rebasing…">Rebase onto #{p?.head}</PendingLabel></button
  >
  {#if !allSettled}<p class="vh" id="{uid}-settle">Settle each conflict first: Keep mine or Take theirs.</p>{/if}
{/snippet}

{#snippet decision()}
  <DecisionCard label="Merge this draft" overline={bar ? '' : 'Merge'}>
    {#if !bar}
      {#if p?.mergeable && p.consequence}<p class="proposal__consequence" data-fid="decision-consequence">One History change. Then live: {p.consequence}.</p>{/if}
      <button
        class="btn btn--primary btn--key drafts__merge"
        class:drafts__merge--small={!p?.mergeable}
        data-fid="decision-merge"
        type="button"
        aria-disabled={!p?.mergeable || Boolean(busy)}
        aria-describedby={reason ? `${uid}-why` : undefined}
        onclick={() => void merge()}
        ><Icon name="check" /><PendingLabel pending={busy === 'merge'} label="Merging…">Merge {d?.ops.length ?? 0} change{d?.ops.length === 1 ? '' : 's'}</PendingLabel></button
      >
    {/if}
    {#if reason}<p class="decision__why" data-fid="decision-note" id="{uid}-why"><strong>{bar ? 'Merge is off' : 'Can’t merge yet:'}</strong> {bar && moved ? `until you rebase onto #${p?.head}.` : reason}</p>{/if}
    {#if bar}
      <!-- B_PhoneDraftConflict: the reason over one row of Discard… and the key action. -->
      <div class="drafts__barrow">
        <button class="btn btn--danger drafts__discard" data-fid="decision-discard" type="button" disabled={Boolean(busy)} onclick={() => d && ondiscard(d)}>Discard…</button>
        {#if moved || conflicts.length}{@render rebaseKey()}{:else}
          <button class="btn btn--primary btn--key drafts__merge" data-fid="decision-merge" type="button" aria-disabled={!p?.mergeable || Boolean(busy)} aria-describedby={reason ? `${uid}-why` : undefined} onclick={() => void merge()}
            ><Icon name="check" /><PendingLabel pending={busy === 'merge'} label="Merging…">Merge {d?.ops.length ?? 0}</PendingLabel></button
          >
        {/if}
      </div>
    {:else if moved || conflicts.length}
      {@render rebaseKey()}
    {/if}
    {#if !bar}
      <span class="decision__spacer" aria-hidden="true"></span>
      <div class="decision__footrow drafts__foot" data-fid="decision-foot">
        <span class="decision__foot">{moved || conflicts.length ? 'Rebase keeps your edits and uses your choices.' : 'Until then it stays a draft.'}</span>
        <button class="btn btn--danger decision__reject" data-fid="decision-discard" type="button" disabled={Boolean(busy)} onclick={() => d && ondiscard(d)}>Discard…</button>
      </div>
    {/if}
    <p class="field__error" role="alert">{error}</p>
  </DecisionCard>
{/snippet}

{#if detail.error && !d}
  <p class="flash flash--error drafts__readerror" role="alert">Couldn't load this draft: {detail.error} <button class="btn" type="button" onclick={() => void reload()}>Try again</button></p>
{:else if d}
  <div class="drafts__view" class:drafts__view--bar={bar}>
    <div class="drafts__main" data-fid="drafts-detail">
      <header class="drafts__head" data-fid="drafts-head">
        <span class="proposal__art drafts__headart" aria-hidden="true">{#if lead}<Portrait boss={lead} size="md" />{/if}</span>
        <div class="drafts__headtext">
          {#if !bar}<p class="cap">Admin draft{d.week ? ` · boss week of ${shortDate(d.week)}` : d.scope === 'weekly' ? ' · weekly timings' : ''}</p>{/if}
          {#if renaming}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <form class="drafts__rename" onsubmit={rename} onkeydown={(event) => event.key === 'Escape' && (event.preventDefault(), void stopRename())}>
              <label class="vh" for="{uid}-title-field">Draft name</label>
              <input id="{uid}-title-field" class="drafts__titlefield" bind:value={title} maxlength="200" required />
              <button class="btn btn--primary" type="submit" disabled={Boolean(busy)}><PendingLabel pending={busy === 'rename'} label="Saving…">Save</PendingLabel></button>
              <button class="btn btn--ghost" type="button" onclick={() => void stopRename()}>Cancel</button>
            </form>
          {:else}
            <div class="drafts__titlerow" data-fid="drafts-title">
              <h2 class="drafts__title">{d.title}</h2>
              {#if open}<button class="btn btn--ghost drafts__renamebtn" type="button" bind:this={renameButton} aria-label="Rename {d.title}" title="Rename" onclick={() => void startRename()}><Icon name="edit" /></button>{/if}
            </div>
          {/if}
          <p class="drafts__meta" data-fid="drafts-meta">
            {#if bar}{d.week ? `Boss week of ${shortDate(d.week)}` : 'Draft'} · {d.staged} staged · on <span class="mono">#{d.base}</span>{#if moved}&nbsp;· head <span class="mono">#{p?.head}</span>{/if}
            {:else}{d.author.name} · edited {editedAt(d.updated_at, timeZone, week?.generated_at ?? '')} · on change <span class="mono">#{d.base}</span>{#if moved}&nbsp;· head <span class="mono">#{p?.head}</span>{/if}{/if}
          </p>
        </div>
      </header>
      {#if open}
        <p class="drafts__live" data-fid="drafts-live-note"><Icon name="eye-off" /><span><b>Not live.</b> No pings, cards or reminders until you merge.</span></p>
      {:else}
        <p class="drafts__live" data-fid="drafts-live-note"><Icon name="info" /><span><b>Closed:</b> {closedLabel(d)}{d.closed_by ? ` by ${d.closed_by.name}` : ''} · {shortAt(d.updated_at, timeZone)}. <a href="/history?tab=graph&draft={encodeURIComponent(d.id)}">See it in History</a></span></p>
      {/if}
      {#if open && moved && p}
        <div class="drafts__moved" data-fid="drafts-base-moved" role="note">
          <Icon name="alert-triangle" />
          {#if bar}
            <p><b>The schedule moved since you started.</b> <span class="mono">#{p.base + 1}</span>{#if p.head > p.base + 1}–<span class="mono">#{p.head}</span>{/if} landed after your base.{#if conflicts.length} {conflicts.length} conflict{conflicts.length === 1 ? '' : 's'}, settled below.{/if}</p>
          {:else}
            <p><b>The schedule moved since you started.</b> {p.head - p.base} change{p.head - p.base === 1 ? '' : 's'} landed after your base <span class="mono">#{p.base}</span> (now <span class="mono">#{p.head}</span>). <a href="/history?tab=graph&draft={encodeURIComponent(d.id)}">See them in History</a></p>
          {/if}
        </div>
      {/if}
      {#if bar && open}{@render conflictsList()}{/if}
      <section class="drafts__ops" data-fid="drafts-ops" aria-labelledby="{uid}-ops">
        <div class="drafts__opshead">
          <h3 class="cap" id="{uid}-ops">Staged changes · {d.ops.length}</h3>
          {#if open}<button class="btn drafts__add" data-fid="drafts-add" type="button" bind:this={addButton} onclick={() => onadd(d)}><Icon name="plus" />Add a change</button>{/if}
        </div>
        {#if d.ops.length}
          <ol class="draftops" aria-label="Staged changes" bind:this={opList}>
            {#each d.ops as op (op.ord)}
              {@const flag = flags.get(op.ord)}
              {@const run = runOf(op.run_id)}
              <li class="draftop" class:draftop--bad={Boolean(flag)} data-fid="drafts-op">
                <span class="draftop__ord mono">{changeNo(op.ord)}</span>
                {#if !bar}<span class="draftop__kind">{kindLabel(op.kind)}</span>{/if}
                <div class="draftop__text">
                  <span class="draftop__what">{#if bar}<span class="draftop__kind">{kindLabel(op.kind)}</span>{/if}<b>{run ? runTitle(run) : (op.run_id ?? op.fixed_id ?? 'New')}</b></span>
                  <span>{op.summary}</span>
                  {#if flag}<span class="draftop__flag"><Icon name="alert-triangle" />{opFlag(flag)}</span>{/if}
                </div>
                {#if !bar}<span class="draftop__at mono">{shortAt(op.added_at, timeZone).slice(-5)}</span>{/if}
                {#if open}
                  <button class="draftop__x" type="button" aria-label="Remove change {changeNo(op.ord)}: {kindLabel(op.kind)} {run ? runTitle(run) : ''}" disabled={Boolean(busy)} onclick={() => void remove(op.ord)}><Icon name="x" /></button>
                {/if}
              </li>
            {/each}
          </ol>
        {:else}
          <p class="note drafts__noops">Nothing staged yet. Add a change: open a run from the Week and use Stage in draft…</p>
        {/if}
      </section>
      {#if bar && open && !moved && p?.rows.length}
        <section class="drafts__preview drafts__preview--inline" aria-labelledby="{uid}-pv">
          <h3 class="cap" id="{uid}-pv">If merged now · against head #{p.head}</h3>
          {#each p.rows as row, i (i)}{@render diff(row)}{/each}
        </section>
      {/if}
    </div>

    {#if open && bar}
      {@render decision()}
    {:else if open}
      <aside class="side-pane drafts__pane" data-fid="drafts-pane" aria-label={moved || conflicts.length ? 'Conflicts and merge' : 'Preview and merge'}>
        <div class="drafts__panehead" data-fid="drafts-pane-head">
          {#if moved || conflicts.length}
            <p class="cap">Base moved · {p ? p.head - p.base : 0} change{p && p.head - p.base === 1 ? '' : 's'} since #{p?.base}</p>
            <h3 class="drafts__panetitle">{conflicts.length} conflict{conflicts.length === 1 ? '' : 's'} · {settledCount === conflicts.length ? (conflicts.length === 2 ? 'both settled' : 'all settled') : `${settledCount} settled`}</h3>
          {:else}
            <p class="cap">Preview · against head #{p?.head ?? d.base}</p>
            <h3 class="drafts__panetitle">If merged now: {changedRuns} run{changedRuns === 1 ? '' : 's'} change{changedRuns === 1 ? 's' : ''}</h3>
          {/if}
        </div>
        {#if moved || conflicts.length}
          <div class="drafts__panebody">{@render conflictsList()}</div>
        {:else}
          <div class="drafts__panebody" data-fid="drafts-preview">
            {#if preview.error && !p}<p class="flash flash--error" role="alert">Couldn't load the preview: {preview.error}</p>
            {:else if p && !p.rows.length}<p class="note">A merge would change nothing yet.</p>{/if}
            {#each p?.rows ?? [] as row, i (i)}{@render diff(row)}{/each}
          </div>
        {/if}
        {@render decision()}
      </aside>
    {/if}
  </div>
{/if}

{#snippet diff(row: RowChange)}
  {@const ords = rowOps(row)}
  <DiffRow {row} fid="drafts-diff">
    {#snippet head()}<p class="drafts__diffhead"><b>{rowTitle(row)}</b>{#if ords.length}<span>{changesLabel(ords)}</span>{/if}</p>{/snippet}
  </DiffRow>
{/snippet}
