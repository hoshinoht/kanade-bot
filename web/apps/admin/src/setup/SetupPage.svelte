<!--
  Setup: what this deployment has set up, read from `GET /api/admin/setup`
  (facts only; every word comes from `copy.ts`). One window, Required then
  Optional in the API's order; each row is a worded status mark, the item's
  name and source, one line, and the one action that fixes it: a Config
  section, Discord in a new tab, or the deploy runbook. Read on each visit
  and on Refresh, never polled. Narrow windows stack each row.
-->
<script lang="ts">
  import type { Channel } from '@kanade/api-types';
  import { Icon, LiveRegion, LoadError, LoadingState, StatusChip } from '@kanade/ui';
  import { onMount } from 'svelte';
  import { localAt } from '../history/describe';
  import { Resource } from '../resource.svelte';
  import PageLine from '../shell/PageLine.svelte';
  import { describe, SOURCE, STATE, summary } from './copy';
  import { requiredLeft, setupStatus } from './status.svelte';

  let { timeZone = 'Asia/Kuala_Lumpur' }: { timeZone?: string } = $props();

  const uid = $props.id();
  // The digest row names its channel.
  const channels = new Resource<Channel[]>('/api/admin/channels');
  let message = $state('');

  async function refresh(announce: boolean) {
    if (setupStatus.loading) return;
    // Cleared first so an unchanged answer is announced again.
    message = '';
    await setupStatus.load();
    if (announce && !setupStatus.error) message = `Checked again. ${summary(requiredLeft(setupStatus.data))}.`;
  }

  // Once per visit; `onMount`, so the read's own loading flag never re-runs it.
  onMount(() => {
    void refresh(false);
    void channels.load();
  });

  const view = $derived(setupStatus.data);
  const left = $derived(requiredLeft(view));
  const invite = $derived(view?.items.find((item) => item.id === 'bot_access')?.invite_url ?? null);
  const context = $derived({
    channelName: (id: string) => channels.data?.find((c) => c.id === id)?.name ?? null,
    when: (iso: string) => localAt(iso, timeZone),
    inviteUrl: invite,
  });
  const groups = $derived(
    view
      ? [
          { key: 'required', label: 'Required', items: view.items.filter((item) => item.required) },
          { key: 'optional', label: 'Optional', items: view.items.filter((item) => !item.required) },
        ]
      : [],
  );
</script>

<PageLine title="Setup">
  <h1>{view ? summary(left) : 'Setup'}</h1>
  <p class="pageline__context">what this deployment has set up</p>
</PageLine>

<section class="card setup window-fill" data-fid="window" aria-labelledby="{uid}-w" aria-busy={setupStatus.loading}>
  <div class="card__head setup__head" data-fid="window-bar">
    <h2 class="card__title" id="{uid}-w">Checklist</h2>
    <button class="btn setup__refresh" type="button" aria-disabled={setupStatus.loading} onclick={() => void refresh(true)}
      ><Icon name="refresh-cw" />{setupStatus.loading ? 'Checking…' : 'Refresh'}</button
    >
  </div>
  {#if view}
    <!-- A failed re-read keeps the last answer and says so. -->
    {#if setupStatus.error}<p class="flash flash--error setup__error" role="alert">Couldn't check again: {setupStatus.error}</p>{/if}
    <!-- Focusable: the list scrolls (axe scrollable-region-focusable). -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="setup__panel" role="region" aria-label="Setup items" tabindex="0">
      {#each groups as group (group.key)}
        <section class="setup__group" aria-labelledby="{uid}-{group.key}">
          <h3 class="setup__grouphead" id="{uid}-{group.key}">
            {group.label} <span class="setup__count">{group.items.filter((item) => item.state === 'ok').length} of {group.items.length} ready</span>
          </h3>
          <ul class="setup__list">
            {#each group.items as item (item.id)}
              {@const row = describe(item, context)}
              {@const state = STATE[item.state]}
              <li class="setup-row" data-item={item.id}>
                <span class="setup-row__state"><StatusChip tone={state.tone} data-state={item.state}><Icon name={state.icon} />{state.label}</StatusChip></span>
                <div class="setup-row__text">
                  <p class="setup-row__name">
                    <b>{row.name}</b>{#if item.source}<span class="setup-row__source"><span class="vh">, source:&nbsp;</span>{SOURCE[item.source]}</span>{/if}
                  </p>
                  <p class="setup-row__line">{row.line}</p>
                </div>
                {#if row.action}
                  <div class="setup-row__action">
                    {#if row.action.kind === 'config'}
                      <a class="btn setup-row__go" href="/config?section={row.action.section}">{row.action.label}<Icon name="chevron-right" /></a>
                    {:else if row.action.kind === 'external'}
                      <a class="btn setup-row__go" href={row.action.href} target="_blank" rel="noopener"
                        >{row.action.label}<Icon name="external-link" /><span class="vh">&nbsp;(opens Discord in a new tab)</span></a
                      >
                    {:else}
                      <span class="setup-row__runbook"><Icon name="book-open" />{row.action.label}</span>
                    {/if}
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>
  {:else if setupStatus.error}
    <LoadError thing="the setup checklist" reason={setupStatus.error} onretry={() => void refresh(false)} level={3} />
  {:else}
    <LoadingState text="Checking the setup…" />
  {/if}
</section>
<LiveRegion {message} />

<style>
  .setup {
    display: flex;
    flex-direction: column;
    overflow: clip;
    container-type: inline-size;
  }

  .setup__head {
    flex-wrap: nowrap;
    align-items: center;
  }

  .setup__refresh {
    flex: none;
    gap: 6px;
    margin-left: auto;
  }

  .setup__error {
    flex: none;
    margin: 8px 12px 0;
  }

  .setup__panel {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    gap: 18px;
    min-height: 0;
    overflow: auto;
    overscroll-behavior: contain;
    padding: 14px;
    background: var(--pane);
    color: var(--ink);
  }

  .setup__panel:focus-visible {
    outline-offset: -3px;
  }

  .setup__group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .setup__grouphead {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin: 0;
    font-size: var(--fs-body);
    font-weight: 700;
  }

  .setup__count {
    color: var(--dim-text);
    font-size: var(--fs-small);
    font-weight: 400;
  }

  .setup__list {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    border: 1.5px solid var(--line);
    border-radius: 14px;
    background: var(--surface);
    list-style: none;
  }

  .setup-row {
    display: grid;
    grid-template-columns: 6.5rem minmax(0, 1fr) auto;
    align-items: center;
    gap: 4px 14px;
    min-height: 56px;
    padding: 8px 12px;
  }

  .setup-row + .setup-row {
    border-top: 1px solid var(--line);
  }

  .setup-row__text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .setup-row__name,
  .setup-row__line {
    margin: 0;
  }

  .setup-row__name {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px 8px;
  }

  .setup-row__source {
    color: var(--dim-text);
    font-size: var(--fs-mini);
  }

  .setup-row__line {
    color: var(--dim-text);
    font-size: var(--fs-body-sm);
  }

  .setup-row__action {
    display: flex;
    justify-content: flex-end;
  }

  .setup-row__go {
    gap: 4px;
    min-height: 40px;
    text-decoration: none;
    white-space: nowrap;
  }

  .setup-row__runbook {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 16rem;
    color: var(--dim-text);
    font-size: var(--fs-small);
  }

  .setup-row__runbook :global(.icon) {
    flex: none;
    width: 16px;
    height: 16px;
  }

  /* A narrow window stacks each row: the mark and name, the line, then the action. */
  @container (max-width: 620px) {
    .setup__panel {
      gap: 14px;
      padding: 10px;
    }

    .setup-row {
      grid-template-columns: auto minmax(0, 1fr);
      align-items: start;
      gap: 6px 10px;
    }

    .setup-row__action {
      grid-column: 1 / -1;
      justify-content: flex-start;
    }

    .setup-row__go {
      min-height: 44px;
    }

    .setup-row__runbook {
      max-width: none;
    }
  }
</style>
