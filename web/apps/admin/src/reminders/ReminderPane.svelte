<!--
  A reminder's card preview (O9), list-detail like Members and History: the
  side pane from 900 px, a sheet below. It reads
  `GET /api/admin/reminders/{id}/preview`, which renders the card with the
  delivery tick's own builder; nothing is posted or saved.
-->
<script lang="ts">
  import type { ReminderPreview, ReminderRow } from '@kanade/api-types';
  import { enter, Icon, LoadError, LoadingState, Modal } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import CardPreview from './CardPreview.svelte';

  let {
    wide,
    row,
    bot,
    avatar,
    onclose,
    leaving = false,
    onleft,
  }: {
    wide: boolean;
    row: ReminderRow;
    /** The bot's display name and avatar URL, as the card's author. */
    bot: string;
    avatar: string | null;
    onclose: () => void;
    leaving?: boolean;
    onleft?: (event: AnimationEvent) => void;
  } = $props();

  const uid = $props.id();
  const STATE: Record<ReminderRow['state'], string> = { queued: 'Queued', due: 'Due now', sent: 'Sent', stale: 'Stale' };
  let preview = $state<Resource<ReminderPreview> | null>(null);
  $effect(() => {
    const next = new Resource<ReminderPreview>(`/api/admin/reminders/${encodeURIComponent(row.id)}/preview`);
    preview = next;
    void next.load();
  });
  const title = $derived(row.bosses.map((b) => b.token).join(' + ') || `#${row.run_short_id}`);
  const card = $derived(preview?.data?.card ?? null);
  const time = $derived(row.at.replace(/^.*\s(\d{1,2}:\d{2})$/, '$1'));
</script>

<svelte:window
  onkeydown={(event) => {
    if (wide && !leaving && event.key === 'Escape' && !document.querySelector('dialog[open]')) {
      event.preventDefault();
      onclose();
    }
  }}
/>

{#snippet content()}
  <div class="reminder-pane__body">
    {#if preview?.error}
      <LoadError thing="the card preview" reason={preview.error} onretry={() => void preview?.load()} />
    {:else if !preview?.data}
      <LoadingState text="Rendering the card…" />
    {:else if card}
      <CardPreview {card} {bot} {avatar} at={time} />
      <p class="reminder-pane__note" data-fid="reminder-note">
        <Icon name="info" />
        <span>
          {#if row.state === 'sent' && preview?.data?.run_started}As the bot would render it now; Discord keeps the last edit from before the run started.
          {:else if row.state === 'sent'}As it reads in Discord now: posted cards are edited when the run changes.
          {:else}What the bot posts at {row.at} if nothing changes before then.{/if}
          {#if !card.heading_final}The heading line is the default; the bot may reword it when it posts.{/if}
          Mentions are shown as names here; nobody is pinged.
        </span>
      </p>
    {:else if row.state === 'stale'}
      <p class="empty"><strong>No card</strong>Retired without posting: no card was sent for this reminder.</p>
    {:else if row.state === 'sent'}
      <p class="empty"><strong>No preview</strong>This card was posted before the bot kept card records, so it cannot be redrawn here.</p>
    {:else}
      <p class="empty"><strong>No card</strong>This reminder posts nothing: its run is cancelled or its kind is unknown.</p>
    {/if}
    {#if row.url}<a class="btn reminder-pane__link" href={row.url} target="_blank" rel="noopener noreferrer">Open in Discord</a>{/if}
  </div>
{/snippet}

{#if wide}
  <aside class="side-pane side-pane--reminder" class:is-leaving={leaving} inert={leaving} aria-labelledby="{uid}-title" data-fid="reminders-pane" onanimationend={onleft} {@attach enter(row.id)}>
    <header class="reminder-pane__head" data-fid="reminders-pane-head">
      <div class="reminder-pane__who">
        <p class="cap">{row.kind} · {STATE[row.state]}</p>
        <h2 id="{uid}-title">{title}</h2>
        <p class="reminder-pane__when mono">{row.at} · run #{row.run_short_id}</p>
      </div>
      <button class="btn btn--ghost reminder-pane__close" type="button" aria-label="Close the card preview" onclick={onclose}><Icon name="x" /></button>
    </header>
    {@render content()}
  </aside>
{:else}
  <Modal open={!leaving} title={title} eyebrow="{row.kind} · {STATE[row.state]} · {row.at}" narrow className="reminder-sheet" onclose={onclose}>
    {@render content()}
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Close</button>
    {/snippet}
  </Modal>
{/if}
