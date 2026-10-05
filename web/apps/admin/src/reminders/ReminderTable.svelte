<!--
  One Reminders tab (B_Reminders): a single table grouped by day, a 44 px
  row card per reminder, every row on one line (the party cut to three chips
  and "+n"). A row opens its card preview: the bosses cell is the button, and
  a press anywhere else on the row (not a link) opens it too.
-->
<script lang="ts">
  import type { ReminderRow } from '@kanade/api-types';
  import { BossTag } from '@kanade/ui';
  import { dayOf, daysUntil, span } from './when';
  import { discordLink } from '../shared/discordLink.svelte';

  let {
    rows,
    tab,
    caption,
    now,
    zone,
    active = null,
    onopen,
  }: {
    rows: ReminderRow[];
    tab: 'queued' | 'sent' | 'stale';
    caption: string;
    /** The server's now (epoch ms); null until the list has loaded. */
    now: number | null;
    zone: string;
    /** The row whose preview is open. */
    active?: string | null;
    onopen: (row: ReminderRow) => void;
  } = $props();

  const SHOWN = 3;
  const upcoming = $derived(tab === 'queued');
  const groups = $derived.by(() => {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a lookup rebuilt by the derivation, never mutated after
    const byDay = new Map<string, ReminderRow[]>();
    for (const row of rows) byDay.set(dayOf(row.at), [...(byDay.get(dayOf(row.at)) ?? []), row]);
    return [...byDay].map(([day, list]) => ({ day, rows: list }));
  });
  const STALE = 'Retired without posting';
  const label = (day: string, first: ReminderRow) => (now !== null && daysUntil(first.fire_at, now, zone) === 0 ? `${day} · today` : day);
  const bossText = (row: ReminderRow) => row.bosses.map((b) => b.token).join(' + ');
  function rowPress(event: MouseEvent, row: ReminderRow) {
    if (event.target instanceof Element && event.target.closest('a, button')) return;
    onopen(row);
  }
</script>

<table class="reminders-table">
  <caption class="vh">{caption}</caption>
  <thead data-fid="reminders-head">
    <tr>
      <th scope="col" class="reminders-table__at">{upcoming ? 'Fires' : 'Fired'}</th>
      <th scope="col" class="reminders-table__in">{upcoming ? 'In' : 'Ago'}</th>
      <th scope="col" class="reminders-table__kind">Kind</th>
      <th scope="col">Bosses</th>
      <th scope="col" class="reminders-table__run">Run</th>
      <th scope="col" class="reminders-table__party">Party</th>
      <th scope="col" class="reminders-table__state">Status</th>
    </tr>
  </thead>
  {#each groups as group (group.day)}
    <tbody>
      <tr class="reminders-table__group" data-fid="reminders-group">
        <th scope="colgroup" colspan="7">{label(group.day, group.rows[0]!)} <span class="reminders-table__count">{group.rows.length}</span></th>
      </tr>
      {#each group.rows as row (row.id)}
        {@const rest = row.party.slice(SHOWN)}
        {@const when = now === null ? '' : span(row.fire_at, now, zone)}
        <tr
          class="reminder"
          class:reminder--active={row.id === active}
          data-fid="reminders-row"
          data-reminder={row.id}
          title={row.state === 'stale' ? `Stale: ${STALE.toLowerCase()}` : undefined}
          onclick={(event) => rowPress(event, row)}
        >
          <td class="mono reminders-table__at">{row.at}</td>
          <td class="mono reminders-table__in">{row.state === 'due' ? 'now' : upcoming || !when ? when : `${when} ago`}</td>
          <td class="mono reminders-table__kind">{row.kind}</td>
          <th scope="row">
            <button
              class="reminder__open"
              type="button"
              aria-label="{bossText(row)}, {row.kind}, {row.at}: preview the card"
              aria-current={row.id === active ? 'true' : undefined}
              onclick={() => onopen(row)}><span class="reminder__bosses">{#each row.bosses as boss (boss.token)}<BossTag {boss} short />{/each}</span></button
            >
          </th>
          <td class="reminders-table__run"><a class="mono" href="/reminders?run={encodeURIComponent(row.run_id)}">#{row.run_short_id}</a></td>
          <td class="reminders-table__party">
            <span class="reminder__party">
              {#each row.party.slice(0, SHOWN) as name, i (i)}<span class="chip reminder__person">{name}</span>{/each}
              {#if rest.length}<span class="chip chip--mono reminder__person" title={rest.join(', ')}>+{rest.length}<span class="vh">: {rest.join(', ')}</span></span>{/if}
            </span>
          </td>
          <td class="reminders-table__state">
            {#if row.state === 'sent'}
              {#if row.url}<a class="tone tone--success" {...discordLink(row.url)}>sent<span class="vh">: open in Discord</span></a>{:else}<span class="tone tone--success">sent</span>{/if}
            {:else if row.state === 'stale'}
              <span class="tone tone--danger" title={STALE}>stale<span class="vh">{` — ${STALE.toLowerCase()}`}</span></span>
            {:else if row.state === 'due'}
              <span class="tone tone--warning">due now</span>
            {:else}
              <span class="reminder__queued">queued</span>
            {/if}
          </td>
        </tr>
      {/each}
    </tbody>
  {/each}
</table>
