<!--
  The Party total as one block (boards B_PhonePartyTotal*, M_PartyTotal*):
  "Party total" with when the oldest figure was saved ("· older than n days"
  once stale), one row per boss (`PartyTotalFigure`), then the notes. The
  admin's phone Party list draws it first; the portal's run pane in place of
  the run-total tiles. With no figure yet, each boss says "no estimate yet"
  and the head says what fills it in.
-->
<script lang="ts">
  import type { RunPartyTotal } from '@kanade/api-types';
  import '@kanade/ui/styles/screens/share-chart.scss';
  import { olderThan } from './chardata';
  import { asOfTime, hasTotal, type TotalNotes } from './partyTotal';
  import PartyTotalFigure from './PartyTotalFigure.svelte';
  import PartyTotalNotes from './PartyTotalNotes.svelte';

  let {
    total,
    bosses,
    timeZone,
    notes,
    audience,
    tag = 'div',
    fids = {},
    ...rest
  }: {
    total: RunPartyTotal;
    /** The run's bosses in order: token and full name ("Karling Hard"). */
    bosses: { token: string; title: string }[];
    timeZone: string;
    notes: TotalNotes;
    /** What fills an empty total in: an admin's Look up party, or the next lookup. */
    audience: 'admin' | 'member';
    tag?: 'div' | 'li';
    fids?: { row?: string; asOf?: string; notes?: string };
    [data: `data-${string}`]: string | undefined;
  } = $props();

  const uid = $props.id();
  const any = $derived(hasTotal(total));
</script>

<svelte:element this={tag} class="party-total" aria-labelledby="{uid}-h" {...rest}>
  <div class="party-total__head">
    <span class="share__title" id="{uid}-h">Party total</span>
    {#if !any}
      <span class="share__sub">{audience === 'admin' ? 'Look up party fills it in.' : 'Fills in after the next lookup.'}</span>
    {:else if total.as_of}
      <span class="share__sub" data-fid={fids.asOf}
        >as of <span class="mono">{asOfTime(total.as_of, timeZone)}</span>{#if total.stale}
          · <span class="party-total__old">{olderThan(total.fresh_days).slice(3)}</span>{/if}</span
      >
    {/if}
  </div>
  <ul class="party-total__rows">
    {#each total.bosses as boss, i (boss.boss)}
      <li class="party-total__row" data-fid={i === 0 ? fids.row : undefined}>
        <abbr class="party-total__boss" title={bosses.find((b) => b.token === boss.boss)?.title ?? boss.boss}>{boss.boss}</abbr>
        <PartyTotalFigure {boss} />
      </li>
    {/each}
  </ul>
  {#if any}<PartyTotalNotes {notes} fid={fids.notes} />{/if}
</svelte:element>
