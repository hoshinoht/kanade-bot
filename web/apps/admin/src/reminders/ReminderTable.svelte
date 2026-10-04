<!--
  One Reminders tab (B_Reminders): a single table grouped by day, a 44 px
  row card per reminder, every row on one line (the party cut to three chips
  and "+n").
-->
<script lang="ts">
  import type { ReminderRow } from '@kanade/api-types';
  import { BossTag } from '@kanade/ui';
  import { dayOf, daysUntil, span } from './when';

  let {
    rows,
    tab,
    caption,
    now,
    zone,
  }: { rows: ReminderRow[]; tab: 'queued' | 'sent' | 'stale'; caption: string; now: Date; zone: string | undefined } = $props();

  const SHOWN = 3;
  const upcoming = $derived(tab === 'queued');
  const groups = $derived.by(() => {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a lookup rebuilt by the derivation, never mutated after
    const byDay = new Map<string, ReminderRow[]>();
    for (const row of rows) byDay.set(dayOf(row.at), [...(byDay.get(dayOf(row.at)) ?? []), row]);
    return [...byDay].map(([day, list]) => ({ day, rows: list }));
  });
  const STALE = 'Retired without posting';
  const label = (day: string, first: ReminderRow) => (daysUntil(first.at, now, zone) === 0 ? `${day} · today` : day);
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
        {@const when = span(row.at, now, zone)}
        <tr class="reminder" data-fid="reminders-row" title={row.state === 'stale' ? `Stale: ${STALE.toLowerCase()}` : undefined}>
          <td class="mono reminders-table__at">{row.at}</td>
          <td class="mono reminders-table__in">{row.state === 'due' ? 'now' : upcoming || !when ? when : `${when} ago`}</td>
          <td class="mono reminders-table__kind">{row.kind}</td>
          <th scope="row"><span class="reminder__bosses">{#each row.bosses as boss (boss.token)}<BossTag {boss} short />{/each}</span></th>
          <td class="reminders-table__run"><a class="mono" href="/reminders?run={encodeURIComponent(row.run_id)}">#{row.run_short_id}</a></td>
          <td class="reminders-table__party">
            <span class="reminder__party">
              {#each row.party.slice(0, SHOWN) as name, i (i)}<span class="chip reminder__person">{name}</span>{/each}
              {#if rest.length}<span class="chip chip--mono reminder__person" title={rest.join(', ')}>+{rest.length}<span class="vh">: {rest.join(', ')}</span></span>{/if}
            </span>
          </td>
          <td class="reminders-table__state">
            {#if row.state === 'sent'}
              {#if row.url}<a class="tone tone--success" href={row.url} target="_blank" rel="noopener noreferrer">sent<span class="vh">: open in Discord</span></a>{:else}<span class="tone tone--success">sent</span>{/if}
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
