<script lang="ts">
  import type { ChangeRecord } from '@kanade/api-types';
  import { enter, Modal } from '@kanade/ui';
  import type { Snippet } from 'svelte';
  import { SURFACE_LABELS, describe, localAt, type Names } from './describe';
  import DiffRow from './DiffRow.svelte';

  let {
    wide,
    member,
    extra,
    record,
    week,
    timezone,
    names,
    onclose,
    onrevert,
    onrestore,
    onraw,
    leaving = false,
    onleft,
  }: {
    wide: boolean;
    /** The "revert a member's changes" tool, at the wide pane's foot (B_History). */
    member?: Snippet;
    /** Between the raw link and the actions: the Graph's "Drafts at this change" (B_HistoryGraph). */
    extra?: Snippet;
    record: ChangeRecord;
    week: string;
    timezone: string;
    names: Names;
    onclose: () => void;
    onrevert: (record: ChangeRecord) => void;
    onrestore: (week: string, record: ChangeRecord) => void;
    onraw: (record: ChangeRecord) => void;
    /** Closed, playing its exit (HistoryPage's Presence): inert, and the phone dialog closes. */
    leaving?: boolean;
    onleft?: (event: AnimationEvent) => void;
  } = $props();
  // A primitive key: a reloaded copy of the same change must not replay the enter.
  const seq = $derived(record.seq);

  const label = (record: ChangeRecord) => `Change #${record.seq}`;
</script>

<svelte:window
  onkeydown={(event) => {
    // A confirmation dialog owns Escape while it is open; otherwise the wide
    // pane closes like the other M3E side panes.
    if (wide && !leaving && event.key === 'Escape' && !document.querySelector('dialog[open]')) {
      event.preventDefault();
      onclose();
    }
  }}
/>

{#snippet content()}
  <div class="history-detail__head" data-fid="history-pane-head">
    <!-- No "History" header row (mockup B_History): the close sits beside the change's overline. -->
    <div class="history-detail__top">
      <p class="cap">Change #{record.seq} · {SURFACE_LABELS[record.surface] ?? record.surface} · {localAt(record.at, timezone)}</p>
      {#if wide}<button class="btn btn--ghost history-detail__close" type="button" aria-label="Close change details" onclick={onclose}>×</button>{/if}
    </div>
    <h2>{describe(record, names, timezone)[0] ?? label(record)}</h2>
    <p class="history-detail__meta">revision <span class="mono">{record.revision}</span> · {record.rows.length} row{record.rows.length === 1 ? '' : 's'} · hash <span class="mono">{record.hash.slice(0, 12)}</span></p>
  </div>

  <div class="history-detail__body">
    <section class="history-detail__rows" aria-labelledby="history-rows-{record.seq}">
      <h3 class="cap" id="history-rows-{record.seq}">Rows · {record.rows.length}</h3>
      {#each record.rows as row, i (i)}<DiffRow {row} fid="history-diff" />{/each}
    </section>

    <button class="linklike history-detail__raw" data-fid="history-raw" type="button" onclick={() => onraw(record)}>Show raw JSON</button>

    {#if extra}{@render extra()}{/if}

    <div class="history-detail__actions" data-fid="history-actions">
      <button class="btn btn--danger" type="button" data-history-revert={record.seq} onclick={() => onrevert(record)}>Revert…</button>
      {#if week}<button class="btn btn--ghost" type="button" onclick={() => onrestore(week, record)}>Restore week to here…</button>{/if}
    </div>

    {#if wide && member}{@render member()}{/if}
  </div>
{/snippet}

{#if wide}
  <!-- A native aside (not SidePane) so the board's region name sits on the pane itself. -->
  <aside class="side-pane side-pane--history" class:is-leaving={leaving} inert={leaving} aria-label="Change details" data-fid="history-pane" onanimationend={onleft} {@attach enter(seq)}>
    {@render content()}
  </aside>
{:else}
  <Modal open={!leaving} title={label(record)} eyebrow="History" narrow className="history-detail" onclose={onclose}>
    {@render content()}
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Close</button>
    {/snippet}
  </Modal>
{/if}
