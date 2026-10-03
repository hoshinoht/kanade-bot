<script lang="ts">
  import type { Strategy, StrategyLevel } from '@kanade/api-types';
  import { StatusChip } from '@kanade/ui';

  let { strategies }: { strategies: Strategy[] } = $props();
  // Risk gets a tone as a second cue; the chip text always names the level.
  const RISK_TONE: Record<StrategyLevel, 'neutral' | 'warn' | 'risk'> = { low: 'neutral', medium: 'warn', high: 'risk' };
</script>

<section class="knowledge-notes knowledge-strategies" aria-labelledby="strategies-heading">
  <h2 class="cap" id="strategies-heading">Strategies</h2>
  <ul>
    {#each strategies as strategy (strategy.name)}
      <li class="strategy">
        <div class="strategy__head">
          <h3>{strategy.name}</h3>
          <span class="strategy__chips"><StatusChip tone={RISK_TONE[strategy.risk]}>Risk: {strategy.risk}</StatusChip><StatusChip>Damage needed: {strategy.damage}</StatusChip></span>
        </div>
        <dl>
          <div><dt class="cap">When</dt><dd>{strategy.when}</dd></div>
          <div><dt class="cap">Payoff</dt><dd>{strategy.payoff}</dd></div>
        </dl>
        <ol aria-label="Steps for {strategy.name}">
          {#each strategy.steps as step, index (index)}<li>{step}</li>{/each}
        </ol>
      </li>
    {/each}
  </ul>
</section>
