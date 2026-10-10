<!--
  Strategies as options, not orders: risk and damage need as three pips plus
  the word (pips are only a picture of it), When and Payoff, steps folded.
-->
<script lang="ts">
  import type { Strategy, StrategyLevel } from '@kanade/api-types';
  import { SvelteSet } from 'svelte/reactivity';
  import { LEVEL_PIPS, LEVEL_WORD } from './guide';

  let { strategies, id }: { strategies: Strategy[]; id: string } = $props();
  const open = new SvelteSet<number>();
  const toggle = (index: number) => (open.has(index) ? open.delete(index) : open.add(index));
</script>

{#snippet meter(label: string, level: StrategyLevel, kind: 'risk' | 'damage')}
  <div class="guide-meter">
    <dt class="cap">{label}</dt>
    <dd>
      <span class="guide-pips guide-pips--{kind}-{level}" aria-hidden="true">{#each [1, 2, 3] as pip (pip)}<i class:on={pip <= LEVEL_PIPS[level]}></i>{/each}</span>
      <strong>{LEVEL_WORD[level]}</strong>
    </dd>
  </div>
{/snippet}

<p class="note">Options, not orders: each trades risk against the damage it needs.</p>
<ul class="guide-strategies">
  {#each strategies as strategy, index (strategy.name)}
    {@const expanded = open.has(index)}
    <li class="guide-card guide-strategy">
      <h3>{strategy.name}</h3>
      <dl class="guide-strategy__meters">
        {@render meter('Risk', strategy.risk, 'risk')}
        {@render meter('Damage need', strategy.damage, 'damage')}
      </dl>
      <dl class="guide-strategy__when">
        <div><dt class="cap">When</dt><dd>{strategy.when}</dd></div>
        <div><dt class="cap">Payoff</dt><dd>{strategy.payoff}</dd></div>
      </dl>
      {#if strategy.steps.length}
        <ol class="guide-steps" id="{id}-steps-{index}" aria-label="Steps for {strategy.name}" hidden={!expanded}>
          {#each strategy.steps as step, at (at)}<li><span class="guide-steps__num mono" aria-hidden="true">{at + 1}</span><span>{step}</span></li>{/each}
        </ol>
        <button type="button" class="btn guide-strategy__toggle" aria-expanded={expanded} aria-controls="{id}-steps-{index}" onclick={() => toggle(index)}
          >{expanded ? 'Hide steps' : `Show ${strategy.steps.length} step${strategy.steps.length === 1 ? '' : 's'}`}</button
        >
      {/if}
    </li>
  {/each}
</ul>
