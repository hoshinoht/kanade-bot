<script lang="ts">
  import type { ReminderRow, Reminders } from '@kanade/api-types';
  import { BossTag } from '@kanade/ui';
  import Pager from '../pages/Pager.svelte';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { paged } from '../pages/paging';
  import { Resource } from '../resource.svelte';

  let { run = '' }: { run?: string } = $props();

  const reminders = new Resource<Reminders>('/api/admin/reminders');
  $effect(() => void reminders.load());

  let query = $state('');
  const q = $derived(query.trim().toLowerCase());
  const keep = (row: ReminderRow) =>
    (!run || row.run_id === run) &&
    (!q ||
      [row.kind, row.run_short_id, ...row.party, ...row.bosses.flatMap((b) => [b.token, b.name])].some((t) => t.toLowerCase().includes(q)));
  const upcoming = $derived((reminders.data?.upcoming ?? []).filter(keep));
  const sent = $derived((reminders.data?.sent ?? []).filter(keep));
  let page = $state(1);
  $effect(() => {
    void q;
    void run;
    page = 1;
  });
  const sentPage = $derived(paged(sent, page));
  const runLabel = $derived(run ? (reminders.data?.upcoming.concat(reminders.data.sent).find((r) => r.run_id === run)?.run_short_id ?? run) : '');
  const STATE_WORDS = { queued: 'queued', due: 'due now', sent: 'sent', stale: 'stale — retired without posting' } as const;
</script>

<div class="page-head">
  <div>
    <p class="eyebrow">The scheduler, as rows</p>
    <h1>{reminders.data ? `${reminders.data.upcoming.length} queued, ${reminders.data.sent.length} sent` : 'Reminders'}</h1>
    <p class="note">Every message the bot will post, or already posted.</p>
  </div>
  {#if run}
    <div class="page-head__side">
      <span class="chip chip--mono">run #{runLabel}</span>
      <a class="btn" href="/reminders">Show every run</a>
    </div>
  {/if}
</div>

{#snippet table(rows: ReminderRow[], fired: boolean, caption: string)}
  <div class="table-wrap">
    <table>
      <caption class="vh">{caption}</caption>
      <thead>
        <tr>
          <th scope="col">{fired ? 'Fired' : 'Fires'}</th>
          <th scope="col">Kind</th>
          <th scope="col">Bosses</th>
          <th scope="col">Run</th>
          <th scope="col">Party</th>
          <th scope="col">{fired ? 'Message' : 'Status'}</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row (row.id)}
          <tr>
            <td class="mono">{row.at}</td>
            <td class="mono">{row.kind}</td>
            <th scope="row"><ul class="bosslist">{#each row.bosses as boss (boss.token)}<li><BossTag {boss} short /></li>{/each}</ul></th>
            <td><a class="id" href="/reminders?run={encodeURIComponent(row.run_id)}">#{row.run_short_id}</a></td>
            <td><span class="chips">{#each row.party as name, i (i)}<span class="chip">{name}</span>{/each}</span></td>
            <td>
              {#if row.state === 'sent' && row.url}
                <a href={row.url} target="_blank" rel="noopener noreferrer">open in Discord</a>
              {:else}
                <span class="tone tone--{row.state === 'stale' ? 'danger' : row.state === 'due' ? 'warning' : row.state === 'sent' ? 'success' : 'neutral'}">{STATE_WORDS[row.state]}</span>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/snippet}

<PaneWindow title="Reminders" bind:query searchLabel="Search reminders" placeholder="boss, member, run id…">
  {#if reminders.error}
    <p class="flash flash--error" role="alert">{reminders.error}</p>
  {:else if reminders.data}
    <h3 class="pane__section">Queued <span class="id">{upcoming.length}</span></h3>
    {#if upcoming.length}{@render table(upcoming, false, 'Queued reminders')}{:else}<p class="note">Nothing pending{q ? ' that matches' : ''}.</p>{/if}
    <h3 class="pane__section">Sent <span class="id">{sent.length}</span></h3>
    {#if sent.length}{@render table(sentPage.rows, true, 'Sent reminders')}
      <Pager bind:page pages={sentPage.pages} total={sent.length} noun="sent card" />{:else}<p class="note">Nothing posted yet{q ? ' that matches' : ''}.</p>{/if}
  {/if}
</PaneWindow>
