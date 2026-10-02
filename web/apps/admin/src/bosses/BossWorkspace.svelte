<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import type { Boss, BossRow, Difficulty, DifficultyFacts, EventBoss, FixedRow, Knowledge, Run, Week } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, Portrait, StatusChip, dayLabel } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import BossGrid from './BossGrid.svelte';
  import { eventAsBoss, seasonTag } from './event';
  import '@kanade/ui/styles/boss-knowledge.scss';

  let { selectedKey = '', difficulty = '' }: { selectedKey?: string; difficulty?: string } = $props();
  const bosses = new Resource<BossRow[]>('/api/admin/bosses');
  const events = new Resource<EventBoss[]>('/api/admin/bosses/events');
  const fixed = new Resource<FixedRow[]>('/api/admin/fixed');
  const week = new Resource<Week>('/api/admin/week?week=this');
  $effect(() => {
    void bosses.load();
    void events.load();
    void fixed.load();
    void week.load();
  });

  const catalog = $derived(bosses.data ?? []);
  const eventRows = $derived(events.data ?? []);
  const fallback = $derived(catalog[0]?.key ?? eventRows[0]?.key ?? '');
  const activeKey = $derived(selectedKey || fallback);
  const activeEvent = $derived(eventRows.find((boss) => boss.key === activeKey));
  const activeBoss = $derived(catalog.find((boss) => boss.key === activeKey));
  const knowledge = $derived(new Resource<Knowledge>(`/api/admin/bosses/${encodeURIComponent(activeKey)}/knowledge`));
  $effect(() => {
    if (activeKey) void knowledge.load();
  });

  const total = $derived(catalog.reduce((n, row) => n + row.difficulties.length, 0));
  const inUse = $derived(catalog.reduce((count, row) => count + row.difficulties.filter((difficulty) => difficulty.in_use).length, 0));
  const doc = $derived(knowledge.data?.doc);
  const facts = $derived(doc?.difficulties ?? []);
  const lists = $derived<[string, string[]][]>(doc ? [['Core', doc.core], ['Danger', doc.danger], ['Tips', doc.tips]] : []);
  const LETTER: Record<string, Difficulty> = { Easy: 'e', Normal: 'n', Hard: 'h', Chaos: 'c', Extreme: 'x' };
  let chosen = $state<string | null>(null);
  const selected = $derived.by(() => {
    const inUse = knowledge.data?.in_use ?? [];
    const wanted = chosen ?? difficulty;
    return facts.find((fact) => fact.name === wanted || LETTER[fact.name] === wanted) ?? facts.find((fact) => inUse.includes(LETTER[fact.name]!)) ?? facts[0] ?? null;
  });
  const letter = $derived(selected ? LETTER[selected.name] : undefined);
  const note = $derived(letter ? doc?.difficulty_notes?.[letter] : undefined);
  const relatedFixed = $derived((fixed.data ?? []).filter((row) => row.bosses.some((boss) => boss.key === activeKey)));
  const relatedRuns = $derived(
    [...(week.data?.runs ?? [])]
      .filter((run) => run.bosses.some((boss) => boss.key === activeKey))
      .sort((left, right) => left.day - right.day || (left.time ?? '99:99').localeCompare(right.time ?? '99:99')),
  );
  const chrome = getChrome();
  let phone = $state(false);
  $effect(() => {
    const query = window.matchMedia('(max-width: 839px)');
    const update = () => (phone = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });
  const compact = $derived(phone && Boolean(selectedKey) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Bosses', name: 'Back to the catalog (Bosses)', go: () => history.back() });
    return () => chrome.back(null);
  });

  let artFailed = $state(false);
  $effect(() => {
    void activeKey;
    artFailed = false;
    chosen = null;
  });
  const art = $derived(artFailed ? null : `/art/entry/${encodeURIComponent(activeKey)}`);
  const asBoss = (row: BossRow): Boss => ({ token: row.key, key: row.key, name: row.name, difficulty: 'n', level: row.level, portrait: row.portrait, portrait_sm: row.portrait, art: null, hue: row.hue });
  const activePortrait = $derived(knowledge.data ? { token: knowledge.data.key, key: knowledge.data.key, name: knowledge.data.name, difficulty: 'n' as const, level: knowledge.data.level, portrait: knowledge.data.portrait, portrait_sm: knowledge.data.portrait, art: null, hue: knowledge.data.hue } : activeBoss ? asBoss(activeBoss) : activeEvent ? eventAsBoss(activeEvent) : null);
  function rows(fact: DifficultyFacts): [string, string][] {
    const out: [string, string][] = [];
    if (fact.entry_level) out.push(['Entry level', String(fact.entry_level)]);
    if (fact.boss_level) out.push(['Boss level', String(fact.boss_level)]);
    if (fact.force) out.push([fact.force.kind === 'sacred' ? 'Sacred force' : 'Arcane force', String(fact.force.value)]);
    if (fact.pdr_percent !== undefined) out.push(['Defence (PDR)', `${fact.pdr_percent}%`]);
    if (fact.party_max) out.push(['Party', fact.party_max === 1 ? 'Solo only' : `Up to ${fact.party_max}`]);
    return out;
  }
  const factRows = $derived(selected ? [...rows(selected), ...(selected.hp?.filter((hp) => hp.phase === 'total').map((hp) => ['HP (total)', hp.value] as [string, string]) ?? [])] : []);
  const phaseHp = $derived(selected?.hp?.filter((hp) => hp.phase !== 'total') ?? []);
  const nextRun = $derived.by(() => {
    const current = week.data;
    if (!current) return null;
    const today = current.days.find((day) => day.is_today)?.index ?? 0;
    const now = new Intl.DateTimeFormat('en-GB', { timeZone: current.timezone, hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
      .formatToParts(new Date(current.generated_at))
      .filter((part) => part.type === 'hour' || part.type === 'minute')
      .map((part) => part.value)
      .join(':');
    return relatedRuns.find((run) => run.day > today || (run.day === today && run.time !== null && run.time >= now)) ?? null;
  });
  /** The whole event row follows its link, like the catalog rows. */
  function forwardEventClicks(list: HTMLElement) {
    const onclick = (event: MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (!target || target.closest('a, button')) return;
      target.closest('li')?.querySelector('a')?.click();
    };
    list.addEventListener('click', onclick);
    return () => list.removeEventListener('click', onclick);
  }
  const runWhen = (run: Run) => `${dayLabel(week.data!, run.day)} ${run.time ?? 'own time'}`;
  const timingBoss = (timing: FixedRow) => timing.bosses.find((boss) => boss.key === activeKey) ?? null;
  const otherBosses = (timing: FixedRow) => timing.bosses.filter((boss) => boss.key !== activeKey).map((boss) => boss.name);
</script>

{#if !compact}
  <PageLine title={bosses.data ? 'Bosses' : ''}>
    <h1>{#if bosses.data}<span class="pageline__num">{catalog.length}</span> bosses, <span class="pageline__num">{total}</span> difficulties{:else}Bosses{/if}</h1>
    {#if bosses.data}<p class="pageline__context"><strong>{inUse}</strong> ticked with a weekly timing</p>{/if}
  </PageLine>
{/if}

<section data-fid="window" class="card bosses-window window-fill" aria-labelledby="bosses-title" class:bosses-window--compact={compact}>
  {#if !compact}<div class="card__head" data-fid="window-bar"><h2 class="card__title" id="bosses-title">The in-game list</h2><span class="bosses-window__order">level order</span></div>{/if}
  <div class="bosses-window__body">
    <nav data-fid="boss-list" class="bosses-list" aria-label="Boss catalog" class:bosses-list--hidden={compact}>
      {#if bosses.error}<p class="flash flash--error" role="alert">{bosses.error}</p>
      {:else if bosses.data}<BossGrid rows={catalog} readonly active={activeKey} />
        {#if eventRows.length}
          <h3 class="cap bosses-list__event-title">Event bosses</h3>
          <ul class="bosses-events" aria-label="Event bosses" {@attach forwardEventClicks}>
            {#each eventRows as boss (boss.key)}
              <li data-fid="boss-row" class:bosses-events__active={boss.key === activeKey}><a href="/bosses/{boss.key}/knowledge"><Portrait boss={eventAsBoss(boss)} size="md" /><strong>{boss.key}</strong></a><StatusChip>Seasonal boss · <abbr title={boss.event.name}>{seasonTag(boss.event)}</abbr></StatusChip></li>
            {/each}
          </ul>
        {/if}
      {:else}<p class="note" aria-busy="true">Loading the boss list…</p>{/if}
    </nav>
    <article data-fid="knowledge-detail" class="knowledge-detail" class:knowledge-detail--hidden={!selectedKey && phone} tabindex="-1">
      {#if knowledge.error}<div class="empty" role="alert"><strong>No knowledge for “{activeKey}”.</strong>{knowledge.error}</div>
      {:else if doc && knowledge.data}
        <header data-fid="knowledge-head" class="knowledge-hero">
          {#if art}<img class="knowledge-hero__art" src={art} alt="" onerror={() => (artFailed = true)} />{/if}
          <div class="knowledge-hero__identity">
            {#if activePortrait}<Portrait boss={activePortrait} size="md" />{/if}
            <div><p class="cap">Checked-in boss knowledge</p><h2>{knowledge.data.name}</h2><p class="knowledge-hero__meta">{knowledge.data.level ? `Lv. ${knowledge.data.level} · ` : ''}researched {knowledge.data.researched_as_of ?? 'undated'} · <code>{knowledge.data.path}</code></p></div>
            {#if doc.event}<StatusChip>Seasonal boss · <abbr title={doc.event.name}>{seasonTag(doc.event)}</abbr></StatusChip>{/if}
          </div>
        </header>
        <div data-fid="knowledge-body" class="knowledge-detail__body">
          <div class="knowledge-detail__main">
            {#if facts.length}<div class="knowledge__switch"><span class="cap" id="difficulty-label">Difficulty</span><div class="seg" role="group" aria-labelledby="difficulty-label">{#each facts as fact (fact.name)}<button type="button" aria-pressed={selected?.name === fact.name} onclick={() => (chosen = fact.name)}>{fact.name}{#if knowledge.data.in_use.includes(LETTER[fact.name]!)}<span class="vh"> (the guild runs it)</span> ✓{/if}</button>{/each}</div></div>{/if}
            <p class="knowledge__summary">{doc.summary}</p>
            {#if doc.event}<p class="flash flash--ok"><strong>Event boss.</strong> {doc.event.availability}</p>{/if}
            {#if selected}<section aria-labelledby="facts-heading"><h2 class="vh" id="facts-heading">{DIFFICULTY_WORDS[letter!]} facts</h2><dl class="knowledge-facts">{#each factRows as [label, value] (label)}<div><dt class="cap">{label}</dt><dd>{value}</dd></div>{/each}</dl>{#if selected.recommended_spec}<section class="knowledge-recommended"><h3 class="cap">Recommended · hexa-converted stat</h3><p>{selected.recommended_spec.text}</p></section>{/if}{#each phaseHp as hp (hp.phase)}<p class="knowledge__detail"><strong>HP phase {hp.phase}:</strong> {hp.value}</p>{/each}{#if note || selected.notes?.length}<section class="knowledge-callouts" aria-labelledby="difficulty-notes-heading"><h3 class="cap" id="difficulty-notes-heading">{selected.name} notes</h3><ul>{#if note}<li>{note}</li>{/if}{#each selected.notes ?? [] as item (item)}<li>{item}</li>{/each}</ul></section>{/if}</section>{/if}
            {#each lists as [title, items] (title)}{#if items.length}<section class="knowledge-notes"><h2 class="cap">{title}</h2><ul>{#each items as item (item)}<li>{item}</li>{/each}</ul></section>{/if}{/each}
            {#if doc.notes?.length}<section class="knowledge-notes"><h2 class="cap">Notes</h2><ul>{#each doc.notes as item (item)}<li>{item}</li>{/each}</ul></section>{/if}
            <section class="knowledge-notes"><h2 class="cap">Sources</h2><ul class="knowledge-sources">{#each doc.sources as source (source.url)}<li><a href={source.url} rel="noopener noreferrer" target="_blank">{source.title}</a><span>by {source.author} · {source.kind} · fetched {source.fetched}{#if source.updated} · updated {source.updated}{/if}</span></li>{/each}</ul><p class="note">Our own paraphrase of these sources; the authors are credited above.</p></section>
          </div>
          <aside data-fid="knowledge-aside" aria-label="Weekly timings"><h2 class="cap">Weekly timings</h2>{#if relatedFixed.length}<ul>{#each relatedFixed as timing (timing.id)}{@const boss = timingBoss(timing)}{@const others = otherBosses(timing)}<li><a href="/fixed?open={encodeURIComponent(timing.id)}"><strong>{timing.weekday_name.slice(0, 3)} {timing.time}</strong>{#if boss}<span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>{/if}{#if others.length}<span class="knowledge-aside__others">+ {others.join(' · ')}</span>{/if}</a></li>{/each}</ul>{:else}<p class="note">No weekly timing uses this boss.</p>{/if}<h2 class="cap">This week</h2><p class="knowledge-aside__count">{relatedRuns.length} run{relatedRuns.length === 1 ? '' : 's'}{#if nextRun} · next <strong>{runWhen(nextRun)}</strong>{/if}</p></aside>
        </div>
      {:else}<p class="note" aria-busy="true">Loading checked-in knowledge…</p>{/if}
    </article>
  </div>
</section>
