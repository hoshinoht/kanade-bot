<!--
  The knowledge detail's guide column: difficulty switch, mission card, lead,
  fact tiles, HP by phase and the difficulty's notes, then the Overview /
  Phases / Strategies / Notes / Sources pill tabs. The open tab is in the URL
  (`?tab=`, Overview by default); a tab with nothing in it is not shown.
-->
<script lang="ts">
  import type { GuideItem, Knowledge } from '@kanade/api-types';
  import FactTiles from './FactTiles.svelte';
  import GuideRows from './GuideRows.svelte';
  import GuideTabs from './GuideTabs.svelte';
  import HpBreakdown from './HpBreakdown.svelte';
  import MechanicBlock from './MechanicBlock.svelte';
  import MissionCard from './MissionCard.svelte';
  import PhaseTimeline from './PhaseTimeline.svelte';
  import SourceList from './SourceList.svelte';
  import StrategyCards from './StrategyCards.svelte';
  import { LETTER, difficultyNotes, factTiles, guideTabs, hpBreakdown, phaseIndex, type GuideTab } from './guide';

  let { knowledge, difficulty = '' }: { knowledge: Knowledge; difficulty?: string } = $props();
  const uid = $props.id();

  const doc = $derived(knowledge.doc);
  const facts = $derived(doc.difficulties ?? []);
  let chosen = $state<string | null>(null);
  const selected = $derived.by(() => {
    const wanted = chosen ?? difficulty;
    return facts.find((fact) => fact.name === wanted || LETTER[fact.name] === wanted) ?? facts.find((fact) => knowledge.in_use.includes(LETTER[fact.name]!)) ?? facts[0] ?? null;
  });
  const tiles = $derived(selected ? factTiles(selected) : []);
  const hp = $derived(selected ? hpBreakdown(selected.hp) : null);
  const notes = $derived(selected ? difficultyNotes(doc, selected) : []);
  // Overview rows: the old documents' core (no phases) first, then danger (risk mark) and tips (ok mark).
  const lists = $derived<{ key: string; title: string; items: GuideItem[]; mark?: 'risk' | 'ok' }[]>([
    { key: 'core', title: 'Core', items: doc.core ?? [] },
    { key: 'danger', title: 'Danger', items: doc.danger, mark: 'risk' },
    { key: 'tips', title: 'Tips', items: doc.tips, mark: 'ok' },
  ]);

  // The router only follows pushes and popstate: a tab or phase change
  // replaces the entry (keeping its state, which tags a single-pane pick) and says so.
  const read = (name: string) => new URLSearchParams(location.search).get(name) ?? '';
  let urlTab = $state(read('tab'));
  let urlPhase = $state(read('phase'));
  $effect(() => {
    const sync = () => {
      urlTab = read('tab');
      urlPhase = read('phase');
    };
    window.addEventListener('popstate', sync);
    return () => window.removeEventListener('popstate', sync);
  });
  function write(params: Record<string, string | null>) {
    const url = new URL(location.href);
    for (const [name, value] of Object.entries(params)) {
      if (value === null) url.searchParams.delete(name);
      else url.searchParams.set(name, value);
    }
    history.replaceState(history.state, '', url.pathname + url.search + url.hash);
    window.dispatchEvent(new PopStateEvent('popstate', { state: history.state }));
  }
  const tabs = $derived(guideTabs(doc));
  const tab = $derived<GuideTab>(tabs.find((item) => item.id === urlTab)?.id ?? 'overview');
  function pick(next: GuideTab) {
    urlTab = next;
    // `phase` belongs to the Phases tab only.
    write({ tab: next === 'overview' ? null : next, phase: null });
  }
  // `?phase=` is 1-based; the first phase is the default and leaves no parameter.
  const phase = $derived(phaseIndex(urlPhase, doc.phases?.length ?? 0));
  function pickPhase(index: number) {
    urlPhase = String(index + 1);
    write({ phase: index === 0 ? null : String(index + 1) });
  }
</script>

{#if facts.length}
  <div class="knowledge__switch">
    <span class="cap" id="difficulty-label">Difficulty</span>
    <div class="seg" role="group" aria-labelledby="difficulty-label">
      {#each facts as fact (fact.name)}
        <button type="button" class:seg__info={!LETTER[fact.name]} aria-pressed={selected?.name === fact.name} onclick={() => (chosen = fact.name)}
          >{fact.name}{#if knowledge.in_use.includes(LETTER[fact.name]!)}<span class="vh"> (the guild runs it)</span> ✓{/if}</button
        >
      {/each}
    </div>
  </div>
{/if}
{#if selected?.mission}<MissionCard mission={selected.mission} stops={knowledge.missions} difficulty={selected.name} />{/if}
<p class="guide-lead">{doc.lead ?? doc.summary}</p>
{#if doc.event}<p class="flash flash--ok"><strong>Event boss.</strong> {doc.event.availability}</p>{/if}
{#if selected}
  <section class="guide-facts" aria-labelledby="facts-heading">
    <h2 class="vh" id="facts-heading">{selected.name} facts</h2>
    {#if tiles.length}<FactTiles {tiles} label="{selected.name} figures" />{/if}
    {#if hp}<HpBreakdown {hp} difficulty={selected.name} />{/if}
    {#if notes.length}
      <section class="guide-notes" aria-labelledby="difficulty-notes-heading">
        <h3 class="cap" id="difficulty-notes-heading">{selected.name} notes</h3>
        <GuideRows items={notes} />
      </section>
    {/if}
  </section>
{/if}

<GuideTabs {tabs} selected={tab} id={uid} onselect={pick} />
<div class="guide-panel" role="tabpanel" id="{uid}-panel" aria-labelledby="{uid}-tab-{tab}" tabindex="0" data-tab={tab}>
  {#if tab === 'overview'}
    {#if doc.mechanics?.length}<div class="guide-mechanics">{#each doc.mechanics as mechanic, index (index)}<MechanicBlock {mechanic} />{/each}</div>{/if}
    {#each lists as list (list.key)}
      {#if list.items.length}
        <section class="guide-list" aria-labelledby="{uid}-{list.key}">
          <h3 class="cap" id="{uid}-{list.key}">{list.title}</h3>
          <GuideRows items={list.items} mark={list.mark} />
        </section>
      {/if}
    {/each}
  {:else if tab === 'phases'}
    <PhaseTimeline phases={doc.phases ?? []} selected={phase} id={uid} onselect={pickPhase} />
  {:else if tab === 'strategies'}
    <StrategyCards strategies={doc.strategies ?? []} id={uid} />
  {:else if tab === 'notes'}
    <GuideRows items={doc.notes ?? []} label="Notes" />
  {:else}
    <SourceList sources={doc.sources} />
  {/if}
</div>
