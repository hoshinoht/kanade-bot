<script lang="ts">
  import {
    Freshness,
    Icon,
    Masthead,
    NapWindow,
    registerServiceWorker,
    RunTable,
    Tabs,
    ThemePicker,
    ToastRegion,
    Toaster,
    longDate,
    type TabItem,
  } from '@kanade/ui';
  import Board from './Board.svelte';
  import { WeekFeed } from './feed.svelte';

  type TabId = 'board' | 'list' | 'appearance';

  const feed = new WeekFeed('/api/public/week');
  const toaster = new Toaster();
  let tab = $state<TabId>('board');

  const tabs = $derived<TabItem<TabId>[]>([
    { id: 'board', label: 'Week' },
    { id: 'list', label: 'List', count: feed.week?.runs.length ?? null },
    { id: 'appearance', label: 'Appearance' },
  ]);

  $effect(() => feed.start());

  $effect(() => {
    void registerServiceWorker({
      url: '/sw.js',
      onUpdateReady: (apply) =>
        toaster.show({ message: 'A new version of Kanade is ready.', timeoutMs: null, action: { label: 'Reload', run: apply } }),
      onControllerChange: () =>
        toaster.show({
          message: 'Kanade was updated in another tab.',
          timeoutMs: null,
          action: { label: 'Reload', run: () => location.reload() },
        }),
    });
  });
</script>

<div class="frame">
  <a class="skip" href="#main">Skip to the schedule</a>
  <Masthead name="Kanade" by="boss schedule">
    {#snippet meta()}
      {#if feed.week}<span class="masthead__tz">{feed.week.timezone}</span>{/if}
      <Freshness state={feed.fresh} updated={feed.updated} />
    {/snippet}
  </Masthead>
  <main class="shell" id="main" tabindex="-1">
    <div class="page-head">
      <div>
        <p class="eyebrow">{feed.week ? `Boss week of Thu ${longDate(feed.week.starts)}` : 'Boss week'}</p>
        <h1>{feed.week ? `${feed.week.runs.length} runs` : 'Schedule'}</h1>
      </div>
      <div class="page-head__side">
        <button type="button" class="btn" onclick={() => feed.refresh()}><Icon name="refresh-cw" /> Refresh</button>
      </div>
    </div>

    {#if feed.week}
      <Tabs items={tabs} bind:selected={tab} label="Schedule views">
        {#snippet panel(id)}
          {#if id === 'board'}
            <Board week={feed.week!} />
          {:else if id === 'list'}
            <RunTable week={feed.week!} />
          {:else}
            <h2 class="card__title panel-title">Appearance</h2>
            <ThemePicker />
          {/if}
        {/snippet}
      </Tabs>
    {:else if feed.fresh === 'loading'}
      <section class="card window-fill" aria-busy="true" aria-labelledby="loading-title">
        <div class="card__head"><h2 class="card__title" id="loading-title">Loading the week…</h2></div>
      </section>
    {:else if feed.closed}
      <NapWindow title="The schedule isn't public right now">
        <p>The guild's admins have closed the public schedule. Reminders and answers still work in Discord.</p>
        {#snippet actions()}
          <button type="button" class="btn btn--primary" onclick={() => feed.refresh()}>Check again</button>
        {/snippet}
      </NapWindow>
    {:else}
      <NapWindow title={feed.fresh === 'offline' ? "You're offline" : "Kanade can't be reached"}>
        <p>
          {feed.fresh === 'offline'
            ? 'Reconnect to see this week. Nothing is kept on this device.'
            : 'The schedule did not load. It will try again on its own.'}
        </p>
        {#snippet actions()}
          <button type="button" class="btn btn--primary" onclick={() => feed.refresh()}>Try again</button>
        {/snippet}
      </NapWindow>
    {/if}

    <p class="footnote">
      <span>All times {feed.week?.timezone ?? 'Asia/Kuala_Lumpur'}.</span>
      <span>Boss week starts {feed.week?.reset ?? 'Thu 00:00'}.</span>
      <span>Read-only schedule.</span>
    </p>
  </main>
  <ToastRegion {toaster} />
</div>

<style>
  .panel-title {
    margin: 0.2rem 0 0.8rem;
  }
</style>
