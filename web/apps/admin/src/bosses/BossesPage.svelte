<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import type { BossRow, EventBoss } from '@kanade/api-types';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { Resource } from '../resource.svelte';
  import BossGrid from './BossGrid.svelte';

  const bosses = new Resource<BossRow[]>('/api/admin/bosses');
  const events = new Resource<EventBoss[]>('/api/admin/bosses/events');
  $effect(() => {
    void bosses.load();
    void events.load();
  });

  const rows = $derived(bosses.data ?? []);
  const total = $derived(rows.reduce((n, r) => n + r.difficulties.length, 0));
  const inUse = $derived(rows.filter((r) => r.difficulties.some((d) => d.in_use)).length);
</script>

<PageLine title={bosses.data ? 'Bosses' : ''}>
  <h1>{bosses.data ? `${rows.length} bosses, ${total} difficulties` : 'Bosses'}</h1>
  {#if bosses.data}<p class="pageline__context"><strong>{inUse}</strong> ticked with a weekly timing</p>{/if}
  {#snippet about()}
    <p>Every boss in <code>boss/bosses.yaml</code>, in level order; the ticked ones have a weekly timing.</p>
  {/snippet}
</PageLine>
<PaneWindow title="The in-game list">
  {#if bosses.error}
    <p class="flash flash--error" role="alert">{bosses.error}</p>
  {:else if bosses.data}
    <BossGrid {rows} readonly />
    <p class="note">Edit <code>boss/bosses.yaml</code> and restart to change this list.</p>
    {#if events.data?.length}
      <h3 class="pane__section">Event bosses</h3>
      <ul class="events">
        {#each events.data as boss (boss.key)}
          <li>
            <a class="bossrow__name" href="/bosses/{boss.key}/knowledge">{boss.key}</a>
            <span class="chip chip--maybe">Event · {boss.event.name}</span>
            <p class="note">{boss.event.availability}</p>
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <p class="note" aria-busy="true">Loading the boss list…</p>
  {/if}
</PaneWindow>

<style>
  .events {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.6rem;
  }

  .events p {
    margin: 0.2rem 0 0;
  }
</style>
