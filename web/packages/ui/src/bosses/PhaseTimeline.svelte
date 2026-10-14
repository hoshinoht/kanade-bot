<!--
  The Phases tab: one timeline bar of equal segments (name, tag, tone tint)
  that picks ONE phase; its items show below. Adjacent phases sharing a
  `group` sit under one bracket, with "↻ repeats" in words when the group
  cycles. A tablist: arrows, Home and End move and select. Narrow, the bar
  turns vertical (container query) so nothing is cut. A single phase shows
  only its card.
-->
<script lang="ts">
  import type { GuidePhase } from '@kanade/api-types';
  import { itemParts, phaseRuns, runLabel } from './guide';

  let { phases, selected, id, onselect }: { phases: GuidePhase[]; selected: number; id: string; onselect: (index: number) => void } = $props();

  const runs = $derived(phaseRuns(phases));
  const labelled = $derived(runs.some((run) => runLabel(run)));
  const current = $derived(phases[selected] ?? phases[0]!);
  const currentRun = $derived(runs.find((run) => run.phases.some((entry) => entry.index === selected)) ?? runs[0]!);
  const buttons: HTMLButtonElement[] = [];

  // A run spans as many equal columns as it has phases (CSSOM: no inline styles under the CSP).
  const span = (count: number) => (node: HTMLElement) => {
    node.style.setProperty('grid-column', `span ${count}`);
  };

  function onkeydown(event: KeyboardEvent) {
    const last = phases.length - 1;
    const moves: Record<string, number> = { ArrowRight: selected + 1, ArrowDown: selected + 1, ArrowLeft: selected - 1, ArrowUp: selected - 1, Home: 0, End: last };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = (target + phases.length) % phases.length;
    onselect(next);
    buttons[next]?.focus();
  }
</script>

<div class="guide-timeline guide-timeline--n{phases.length}" class:guide-timeline--labelled={labelled}>
  <!-- One phase: just its card, no bar and no tablist (user decision 2026-10-05). -->
  {#if phases.length > 1}
  <div class="guide-timeline__bar" role="tablist" aria-label="Phases">
    {#each runs as run, r (r)}
      {@const label = runLabel(run)}
      <div class="guide-timeline__run" class:guide-timeline__run--group={Boolean(label)} {@attach span(run.phases.length)}>
        <span class="guide-timeline__bracket" aria-hidden="true">{#if run.cycle}<span class="guide-timeline__loop">↻</span>{/if}{label}</span>
        {#each run.phases as { index, phase } (index)}
          <button
            type="button"
            role="tab"
            class="guide-timeline__seg {phase.tone ? `guide-tone--${phase.tone}` : 'guide-timeline__seg--plain'}"
            id="{id}-phase-{index}"
            aria-selected={selected === index}
            aria-controls="{id}-phase-panel"
            tabindex={selected === index ? 0 : -1}
            bind:this={buttons[index]}
            onclick={() => onselect(index)}
            {onkeydown}
            >{#if label}<span class="vh">{`${label}: `}</span>{/if}<strong>{phase.name}</strong>{#if phase.tag}<span class="vh">{phase.tag ? ', ' : ''}</span><span class="guide-timeline__tag">{phase.tag}</span>{/if}</button
          >
        {/each}
      </div>
    {/each}
  </div>
  {/if}
  <div
    class="guide-card guide-phase"
    role={phases.length > 1 ? 'tabpanel' : undefined}
    id="{id}-phase-panel"
    aria-labelledby={phases.length > 1 ? `${id}-phase-${selected}` : undefined}
  >
    <!-- No position number: names already carry it ("Phase 3"), and data may skip phases (Lotus 1, 3). -->
    <div class="guide-phase__head">
      {#if runLabel(currentRun)}<p class="cap">{#if currentRun.cycle}<span class="guide-timeline__loop" aria-hidden="true">↻</span>{/if}{runLabel(currentRun)}</p>{/if}
      <h3>{current.name}</h3>
      {#if current.tag}<p class="guide-phase__tag">{current.tag}</p>{/if}
    </div>
    <ul>
      {#each current.items as item, at (at)}
        {@const part = itemParts(item)}
        <li>{#if part.title}<strong>{part.title}</strong>{` ${part.text}`}{:else}{part.text}{/if}</li>
      {/each}
    </ul>
  </div>
</div>
