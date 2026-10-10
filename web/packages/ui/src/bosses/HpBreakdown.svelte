<!--
  HP breakdown for the selected difficulty: the stored total, then one row per
  phase in data order, a dark label bar (name, plus the phase's HP when it is
  split between targets) over one
  bar per target in the difficulty's pill colours. Values stay short as
  stored; each carries its spelled-out form in a title and for screen readers.
-->
<script lang="ts">
  import { LETTER, spellHp, type HpBreakdown } from './guide';

  let { hp, difficulty, fid }: { hp: HpBreakdown; difficulty: string; /** Fidelity tag (member portal). */ fid?: string } = $props();
  const tone = $derived(LETTER[difficulty] ?? difficulty.toLowerCase());
  const uid = $props.id();
  // A disclosure, closed by default (user, 2026-10-05); closed, the total stays in the head.
  let open = $state(false);
</script>

{#snippet amount(value: string)}
  <span class="mono" title={spellHp(value)}><span aria-hidden="true">{value}</span><span class="vh">{spellHp(value)}</span></span>
{/snippet}

<section class="guide-hp" class:guide-hp--closed={!open} aria-labelledby="guide-hp-heading" data-fid={fid}>
  <div class="guide-hp__head">
    <h3 class="cap" id="guide-hp-heading">
      <button type="button" class="guide-hp__toggle" aria-expanded={open} aria-controls="{uid}-hp-body" onclick={() => (open = !open)}
        ><span>HP</span><span class="guide-hp__chevron" aria-hidden="true"></span></button
      >
    </h3>
    {#if !open && hp.total}<p class="guide-hp__total guide-hp__total--head"><span>Total HP</span>{@render amount(hp.total)}</p>{/if}
  </div>
  <div class="guide-hp__body" id="{uid}-hp-body" hidden={!open}>
  {#if hp.total}<p class="guide-hp__total"><span>Total HP</span>{@render amount(hp.total)}</p>{/if}
  <ol class="guide-hp__phases">
    {#each hp.phases as phase, index (index)}
      <li class="guide-hp__phase">
        <p class="guide-hp__label"><span>{phase.name}</span>{#if phase.total}{@render amount(phase.total)}{/if}</p>
        {#if phase.bars.length > 1}
          <ul class="guide-hp__bars" aria-label="{phase.bars.length} targets, each">
            {#each phase.bars as bar, at (at)}<li class="guide-hp__bar guide-hp__bar--{tone}">{@render amount(bar)}</li>{/each}
          </ul>
        {:else}
          <p class="guide-hp__bars"><span class="guide-hp__bar guide-hp__bar--{tone}">{@render amount(phase.bars[0]!)}</span></p>
        {/if}
      </li>
    {/each}
  </ol>
  </div>
</section>
