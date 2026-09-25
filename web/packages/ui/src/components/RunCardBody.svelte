<script lang="ts">
  import type { PublicRun } from '@kanade/api-types';
  import BossTag from './BossTag.svelte';
  import StatusMark from './StatusMark.svelte';
  import { tally } from '../format';

  let { run }: { run: PublicRun } = $props();
  const count = $derived(tally(run));
  // The lead boss's entry art only; the sheet has room for two (v4 board.html).
  const art = $derived(run.bosses.find((b) => b.art)?.art ?? null);
</script>

{#if art}<img class="runcard__art" src={art} alt="" loading="lazy" decoding="async" />{/if}
<span class="runcard__top">
  <span class="runcard__time">{run.status === 'otot' || run.time === null ? 'own time' : run.time}</span>
  <span class="runcard__meta">
    <StatusMark status={run.status} />
    <span class="runcard__tally">{count.on}/{count.total}</span>
  </span>
</span>
<span class="runcard__bosses">
  {#each run.bosses as boss (boss.token)}<BossTag {boss} short />{/each}
</span>
