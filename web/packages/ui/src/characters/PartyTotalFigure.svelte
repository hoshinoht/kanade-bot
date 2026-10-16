<!--
  One boss's Party total (board B_PartyTotal): "150.2% · ~19.6 min" (a `*`
  when a figure counted assumes a 100% critical rate; minutes only from a
  saved total of that boss's whole party), then "technically possible" or
  not, and "not complete" while a member has no estimate. Without a figure,
  a dashed "no estimate yet", never 0%.
-->
<script lang="ts">
  import type { PartyTotalBoss } from '@kanade/api-types';
  import Icon from '../components/Icon.svelte';
  import StatusChip from '../components/StatusChip.svelte';
  import { minutes } from './chardata';

  let { boss }: { boss: PartyTotalBoss } = $props();
</script>

{#if boss.combined_percentage === null}
  <span class="party-total__none">no estimate yet</span>
{:else}
  <span class="party-total__fig"
    ><span
      ><span class="party-total__num">{boss.combined_percentage.toFixed(1)}%{boss.critical_rate_assumed ? '*' : ''}</span>{#if boss.critical_rate_assumed}<span class="vh">
          (counts an assumed 100% critical rate)</span
        >{/if}{#if boss.estimated_minutes !== null}<span class="party-total__min"> · ~{minutes(boss.estimated_minutes)}</span>{/if}</span
    >{#if boss.technically_possible === true}<StatusChip tone="ok"><Icon name="check" />technically possible</StatusChip>{:else if boss.technically_possible === false}<StatusChip
        tone="warn"><Icon name="alert-triangle" />not technically possible</StatusChip
      >{/if}{#if !boss.complete}<StatusChip tone="warn"><Icon name="alert-triangle" />not complete</StatusChip>{/if}</span
  >
{/if}
