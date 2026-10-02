<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import type { BossRow, EventBoss } from '@kanade/api-types';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { Resource } from '../resource.svelte';
  import BossGrid from './BossGrid.svelte';
  import { Portrait, StatusChip } from '@kanade/ui';
  import '@kanade/ui/styles/boss-grid.scss';
  import { eventAsBoss as asBoss, seasonal } from './event';

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
  <h1>{#if bosses.data}<span class="pageline__num">{rows.length}</span> bosses, <span class="pageline__num">{total}</span> difficulties{:else}Bosses{/if}</h1>
  {#if bosses.data}<p class="pageline__context"><strong>{inUse}</strong> ticked with a weekly timing</p>{/if}
</PageLine>
<PaneWindow title="The in-game list">
  {#if bosses.error}
    <p class="flash flash--error" role="alert">{bosses.error}</p>
  {:else if bosses.data}
    <BossGrid {rows} readonly />
    <p class="note">Edit <code>boss/bosses.yaml</code> and restart to change this list.</p>
    {#if events.data?.length}
      <h3 class="pane__section">Event bosses</h3>
      <ul class="grid-bosses events" aria-label="Event bosses">
        {#each events.data as boss (boss.key)}
          <li class="bossrow">
            <div class="bossrow__id">
              <Portrait boss={asBoss(boss)} size="md" />
              <a class="bossrow__name" href="/bosses/{boss.key}/knowledge">{boss.key}</a>
            </div>
            <div class="events__about">
              <StatusChip>{seasonal(boss.event)}</StatusChip>
              <p class="note">{boss.event.availability}</p>
            </div>
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
  }

  .events__about p {
    margin: 0.3rem 0 0;
  }
</style>
