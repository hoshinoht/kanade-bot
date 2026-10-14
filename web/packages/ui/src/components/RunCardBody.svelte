<script lang="ts">
  import type { PublicRun } from '@kanade/api-types';
  import type { Snippet } from 'svelte';
  import BossArt from './BossArt.svelte';
  import BossTag from './BossTag.svelte';
  import StatusMark from './StatusMark.svelte';
  import { tally } from '../format';

  let {
    run,
    bosses,
    places,
    badge,
    art = true,
  }: {
    run: PublicRun;
    bosses?: Snippet;
    /** "1 open" or "full" after the tally (admin board). */
    places?: string;
    /** Beside the clock (the member portal's YOU chip). */
    badge?: Snippet;
    /** The entry-art veil; the portal's view-only cards go without. */
    art?: boolean;
  } = $props();
  const count = $derived(tally(run));
  // The lead boss's entry art only (animated where it has a clip); the sheet has room for two (v4 board.html).
  const lead = $derived(run.bosses.find((b) => b.art) ?? null);
</script>

{#if lead && art}<BossArt class="runcard__art" still={lead.art} animated={lead.animated} loading="lazy" />{/if}
<span class="runcard__top">
  <span class="runcard__time">{run.status === 'otot' || run.time === null ? 'own time' : run.time}</span>
  {@render badge?.()}
  <span class="runcard__meta">
    <StatusMark status={run.status} />
    <span class="runcard__tally">{count.on}/{count.total}</span>{#if places}<span class="runcard__places">{places}</span>{/if}
  </span>
</span>
{#if bosses}{@render bosses()}{:else}<span class="runcard__bosses">
  {#each run.bosses as boss (boss.token)}<BossTag {boss} short />{/each}
</span>{/if}
