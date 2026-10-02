<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/panes.scss';
  import '@kanade/ui/styles/boss-grid.scss';
  import type { Difficulty, DifficultyFacts, Knowledge } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, Portrait, StatusChip } from '@kanade/ui';
  import { seasonal } from './event';
  import { Resource } from '../resource.svelte';

  let { key, difficulty = '' }: { key: string; difficulty?: string } = $props();

  const knowledge = $derived(new Resource<Knowledge>(`/api/admin/bosses/${encodeURIComponent(key)}/knowledge`));
  $effect(() => void knowledge.load());

  const LETTER: Record<string, Difficulty> = { Easy: 'e', Normal: 'n', Hard: 'h', Chaos: 'c', Extreme: 'x' };
  const doc = $derived(knowledge.data?.doc);
  const facts = $derived(doc?.difficulties ?? []);
  // Open on ?difficulty=, else the difficulty the guild runs, else the first listed.
  let chosen = $state<string | null>(null);
  const selected = $derived.by(() => {
    const want = chosen ?? difficulty;
    const inUse = knowledge.data?.in_use ?? [];
    return (
      facts.find((f) => LETTER[f.name] === want || f.name === want) ??
      facts.find((f) => inUse.includes(LETTER[f.name]!)) ??
      facts[0] ??
      null
    );
  });
  const letter = $derived(selected ? LETTER[selected.name] : undefined);
  const note = $derived(letter ? doc?.difficulty_notes?.[letter] : undefined);
  const uid = $props.id();
  const lists = $derived<[string, string[]][]>(doc ? [['Core', doc.core], ['Danger', doc.danger], ['Tips', doc.tips]] : []);

  function rows(f: DifficultyFacts): [string, string][] {
    const out: [string, string][] = [];
    if (f.entry_level) out.push(['Entry level', String(f.entry_level)]);
    if (f.boss_level) out.push(['Boss level', String(f.boss_level)]);
    if (f.force) out.push([f.force.kind === 'sacred' ? 'Sacred force' : 'Arcane force', String(f.force.value)]);
    if (f.pdr_percent !== undefined) out.push(['Defence (PDR)', `${f.pdr_percent}%`]);
    if (f.party_max) out.push(['Party', f.party_max === 1 ? 'Solo only' : `Up to ${f.party_max}`]);
    return out;
  }
</script>

<p class="backlink"><a class="btn" href="/bosses">← Boss catalog</a></p>
<PageLine>
  <p class="eyebrow"><a href="/bosses">Bosses</a> · knowledge</p>
  <h1 class="knowledge__title">
      {#if knowledge.data}<Portrait
          boss={{ token: knowledge.data.key, key: knowledge.data.key, name: knowledge.data.name, difficulty: 'n', level: knowledge.data.level, portrait: knowledge.data.portrait, portrait_sm: knowledge.data.portrait, art: null, hue: knowledge.data.hue }}
          size="md"
        />{/if}
      {knowledge.data?.name ?? key}
      {#if doc?.event}<StatusChip>{seasonal(doc.event)}</StatusChip>{/if}
    </h1>
  {#if knowledge.data}
    <p class="pageline__context">
      {knowledge.data.level ? `Lv. ${knowledge.data.level} · ` : ''}researched {knowledge.data.researched_as_of ?? 'undated'}
    </p>
  {/if}
</PageLine>

<section class="card pane window-fill" aria-labelledby="{uid}-title">
  <div class="card__head">
    <h2 class="card__title" id="{uid}-title">{doc?.event ? doc.event.name : 'Strategy'}</h2>
    {#if knowledge.data}<span class="id">{knowledge.data.path}</span>{/if}
  </div>
  <div class="pane__body">
    {#if knowledge.error}
      <div class="empty" role="alert"><strong>No knowledge for “{key}”.</strong>{knowledge.error}</div>
    {:else if doc}
      {#if doc.event}<p class="flash flash--ok"><strong>Event boss.</strong> {doc.event.availability}</p>{/if}
      <p class="knowledge__summary">{doc.summary}</p>

      {#if facts.length}
        <div class="knowledge__switch">
          <span class="label" id="{uid}-diff">Difficulty</span>
          <div class="seg" role="group" aria-labelledby="{uid}-diff">
            {#each facts as f (f.name)}
              <button
                type="button"
                class="seg__btn"
                aria-pressed={selected?.name === f.name}
                onclick={() => (chosen = f.name)}
              >
                {f.name}{#if knowledge.data?.in_use.includes(LETTER[f.name]!)}<span class="vh"> (the guild runs it)</span>&nbsp;✓{/if}
              </button>
            {/each}
          </div>
        </div>
        {#if selected}
          <section class="knowledge__facts" aria-labelledby="{uid}-facts">
            <h3 class="pane__section" id="{uid}-facts">
              <span class="pill pill--{letter}">{DIFFICULTY_WORDS[letter!].toUpperCase()}</span> facts
            </h3>
            <table>
              <caption class="vh">{selected.name} facts</caption>
              <tbody>
                {#each rows(selected) as [label, value] (label)}
                  <tr><th scope="row">{label}</th><td class="mono">{value}</td></tr>
                {/each}
                {#each selected.hp ?? [] as hp (hp.phase)}
                  <tr>
                    <th scope="row">HP {hp.phase === 'total' ? '(total)' : `phase ${hp.phase}`}</th>
                    <td class="mono">{hp.value}</td>
                  </tr>
                {/each}
                {#if selected.recommended_spec}
                  <tr>
                    <th scope="row">Recommended ({selected.recommended_spec.kind})</th>
                    <td>{selected.recommended_spec.text}</td>
                  </tr>
                {/if}
              </tbody>
            </table>
            {#if note}<p class="note"><strong>{selected.name}:</strong> {note}</p>{/if}
            {#each selected.notes ?? [] as n (n)}<p class="note">{n}</p>{/each}
          </section>
        {/if}
      {/if}

      {#each lists as [title, items] (title)}
        <h3 class="pane__section">{title}</h3>
        <ul class="knowledge__list">{#each items as item (item)}<li>{item}</li>{/each}</ul>
      {/each}
      {#if doc.notes?.length}
        <h3 class="pane__section">Notes</h3>
        <ul class="knowledge__list">{#each doc.notes as item (item)}<li>{item}</li>{/each}</ul>
      {/if}

      <h3 class="pane__section">Sources</h3>
      <ul class="knowledge-sources">
        {#each doc.sources as source (source.url)}
          <li>
            <a href={source.url} rel="noopener noreferrer" target="_blank">{source.title}</a>
            <span class="knowledge__credit">
              by {source.author} · {source.kind} · fetched {source.fetched}{#if source.updated} · updated {source.updated}{/if}
            </span>
          </li>
        {/each}
      </ul>
      <p class="note">Our own paraphrase of these sources; the authors are credited above.</p>
    {:else}
      <p class="note" aria-busy="true">Loading…</p>
    {/if}
  </div>
</section>

<style>
  .backlink {
    margin: 0 0 0.5rem;
  }

  .knowledge__title {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .knowledge__summary {
    max-width: 72ch;
    font-size: var(--fs-body);
  }

  .knowledge__switch {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem 0.8rem;
    margin: 0.6rem 0;
  }

  .knowledge__facts table {
    max-width: 44rem;
  }

  .knowledge__list {
    margin: 0;
    padding-left: 1.25rem;
    max-width: 72ch;
  }

  .knowledge__list li + li {
    margin-top: 0.3rem;
  }

  .knowledge__credit {
    display: block;
    font-size: var(--fs-small);
    color: var(--dim-text);
  }
</style>
