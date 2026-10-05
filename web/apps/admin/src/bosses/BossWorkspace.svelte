<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import type { Boss, BossRow, EventBoss, FixedRow, Knowledge, Run, Week } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, LoadError, LoadingState, Portrait, RowContent, SINGLE_PANE_QUERY, StatusChip, dayLabel, enter } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import BossGrid from './BossGrid.svelte';
  import KnowledgeGuide from './KnowledgeGuide.svelte';
  import { LETTER } from './guide';
  import { eventAsBoss, seasonTag } from './event';
  import { heroArt } from './heroArt';
  import { prefersReducedMotion } from 'svelte/motion';
  import { tick, untrack } from 'svelte';
  import '@kanade/ui/styles/boss-knowledge.scss';

  let {
    selectedKey = '',
    difficulty = '',
    onselect,
  }: {
    selectedKey?: string;
    difficulty?: string;
    /** Opens a boss (empty: the catalog); `open` pushes a history entry (single pane: Back returns to the catalog). */
    onselect?: (key: string, open: boolean) => void;
  } = $props();
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
  // The boss whose knowledge is on screen: its pane content enters when it changes.
  const shownKey = $derived(doc && knowledge.data ? knowledge.data.key : null);
  const facts = $derived(doc?.difficulties ?? []);
  /** Knowledge-only difficulties (never scheduled): no catalog letter, own tick colours. */
  const infoOnly = $derived(facts.filter((fact) => !LETTER[fact.name]).map((fact) => fact.name));
  const tickClass = (name: string) => LETTER[name] ?? name.toLowerCase();
  const relatedFixed = $derived((fixed.data ?? []).filter((row) => row.bosses.some((boss) => boss.key === activeKey)));
  const relatedRuns = $derived(
    [...(week.data?.runs ?? [])]
      .filter((run) => run.bosses.some((boss) => boss.key === activeKey))
      .sort((left, right) => left.day - right.day || (left.time ?? '99:99').localeCompare(right.time ?? '99:99')),
  );
  const chrome = getChrome();
  let phone = $state(false);
  $effect(() => {
    const query = window.matchMedia(SINGLE_PANE_QUERY);
    const update = () => (phone = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });
  const compact = $derived(phone && Boolean(selectedKey) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Bosses', name: 'Back to the catalog (Bosses)', go: () => leaveDetail() });
    return () => chrome.back(null);
  });

  // Whether the open boss's entry came from a pick here (so leaving pops it) or a deep link.
  const pushedHere = () => (history.state as { bossDetail?: boolean } | null)?.bossDetail === true;
  function leaveDetail() {
    if (pushedHere()) history.back();
    else onselect?.('', false);
  }

  /** Single pane: a catalog pick pushes a tagged entry instead of the router's plain one. */
  function tagPicks(nav: HTMLElement) {
    const onclick = (event: MouseEvent) => {
      if (!phone || !onselect || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href]') : null;
      const key = link ? /^\/bosses\/([^/]+)\/knowledge$/.exec(link.pathname)?.[1] : undefined;
      if (!key) return;
      event.preventDefault();
      onselect(decodeURIComponent(key), true);
    };
    nav.addEventListener('click', onclick);
    return () => nav.removeEventListener('click', onclick);
  }

  // Single pane swaps catalog and detail: focus follows into the detail after
  // a pick, and back to the opened boss's link on return.
  let detailEl = $state<HTMLElement>();
  let navEl = $state<HTMLElement>();
  let was = untrack(() => selectedKey);
  $effect(() => {
    const now = selectedKey;
    const before = was;
    was = now;
    if (!phone || now === before) return;
    if (now && !before) void tick().then(() => detailEl?.focus({ preventScroll: true }));
    else if (!now && before)
      void tick().then(() => navEl?.querySelector<HTMLElement>(`a[href="/bosses/${CSS.escape(before)}/knowledge"]`)?.focus({ preventScroll: true }));
  });

  let artFailed = $state(false);
  let videoFailed = $state(false);
  $effect(() => {
    void activeKey;
    artFailed = false;
    videoFailed = false;
  });
  const art = $derived(heroArt({ key: activeKey, animated: knowledge.data?.animated ?? null, reducedMotion: prefersReducedMotion.current, videoFailed, stillFailed: artFailed }));
  const asBoss = (row: BossRow): Boss => ({ token: row.key, key: row.key, name: row.name, difficulty: 'n', level: row.level, portrait: row.portrait, portrait_sm: row.portrait, art: null, hue: row.hue });
  const activePortrait = $derived(knowledge.data ? { token: knowledge.data.key, key: knowledge.data.key, name: knowledge.data.name, difficulty: 'n' as const, level: knowledge.data.level, portrait: knowledge.data.portrait, portrait_sm: knowledge.data.portrait, art: null, hue: knowledge.data.hue } : activeBoss ? asBoss(activeBoss) : activeEvent ? eventAsBoss(activeEvent) : null);
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
  // The selected difficulty (from the guide), for the compact header's pill.
  let shownDifficulty = $state<string | null>(null);
  // The hero collapses to a one-line bar once the guide is scrolled down and
  // comes back at the top. Two thresholds, so the hero's own change in height
  // (the panel grows) cannot flip it back; never for content that barely scrolls.
  let heroCompact = $state(false);
  function collapseHero(panel: HTMLElement) {
    const onscroll = () => {
      const room = panel.scrollHeight - panel.clientHeight;
      if (!heroCompact && panel.scrollTop > 96 && room > 160) heroCompact = true;
      else if (heroCompact && panel.scrollTop < 12) heroCompact = false;
    };
    panel.addEventListener('scroll', onscroll, { passive: true });
    return () => panel.removeEventListener('scroll', onscroll);
  }
  // The header is one line (cut with an ellipsis on narrow frames); the title holds it whole.
  const heroMeta = $derived(knowledge.data ? `${knowledge.data.level ? `Lv. ${knowledge.data.level} · ` : ''}researched ${knowledge.data.researched_as_of ?? 'undated'} · ${knowledge.data.path}` : '');
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
    <nav data-fid="boss-list" class="bosses-list" aria-label="Boss catalog" class:bosses-list--hidden={phone && Boolean(selectedKey)} bind:this={navEl} {@attach tagPicks}>
      {#if bosses.error}<LoadError thing="the boss list" reason={bosses.error} onretry={() => void bosses.load()} level={3} />
      {:else if bosses.data}<BossGrid rows={catalog} readonly active={activeKey} {infoOnly} />
        {#if eventRows.length}
          <h3 class="cap bosses-list__event-title">Event bosses</h3>
          <ul class="bosses-events" aria-label="Event bosses" {@attach forwardEventClicks}>
            {#each eventRows as boss (boss.key)}
              <li class="expandable-row" data-fid="boss-row" class:bosses-events__active={boss.key === activeKey}><a href="/bosses/{boss.key}/knowledge" aria-current={boss.key === activeKey ? 'true' : undefined}><Portrait boss={eventAsBoss(boss)} size="md" /><strong>{boss.key}</strong></a><RowContent expanded={boss.key === activeKey}>{#snippet compact()}<StatusChip>Seasonal boss · <abbr title={boss.event.name}>{seasonTag(boss.event)}</abbr></StatusChip>{/snippet}<span class="bossrow__difficulties" role="group" aria-label="{boss.key} difficulties">{#each boss.key === knowledge.data?.key ? facts : [] as fact (fact.name)}<span><span class="boss-tick boss-tick--{tickClass(fact.name)}">{fact.name.toUpperCase()}</span>{#if fact.boss_level ?? fact.entry_level}<span class="bossrow__tracked">{` Lv. ${fact.boss_level ?? fact.entry_level}`}</span>{/if}</span>{/each}</span></RowContent></li>
            {/each}
          </ul>
        {/if}
      {:else}<LoadingState text="Loading the boss list…" />{/if}
    </nav>
    <article data-fid="knowledge-detail" class="knowledge-detail" class:knowledge-detail--hidden={!selectedKey && phone} tabindex="-1" bind:this={detailEl} {@attach enter(shownKey)}>
      <!-- One pane below 900 px: the rail frame's way back (the phone frame's is in the top bar). -->
      {#if phone && selectedKey && !compact}<button type="button" class="btn btn--ghost knowledge-detail__back" onclick={leaveDetail}><span aria-hidden="true">←</span> Back to the catalog</button>{/if}
      {#if knowledge.error}<div class="empty" role="alert"><strong>No knowledge for “{activeKey}”.</strong>{knowledge.error}</div>
      {:else if doc && knowledge.data}
        <header data-fid="knowledge-head" class="knowledge-hero" class:knowledge-hero--compact={heroCompact}>
          <!-- Keyed by source: a boss switch builds a fresh element, never showing the previous boss's frame. -->
          {#key art?.src}
            {#if art?.kind === 'video'}<video class="knowledge-hero__art" src={art.src} poster={art.poster} muted autoplay loop playsinline preload="metadata" disablepictureinpicture disableremoteplayback aria-hidden="true" onerror={() => (videoFailed = true)}></video>
            {:else if art}<img class="knowledge-hero__art" src={art.src} alt="" onerror={() => (artFailed = true)} />{/if}
          {/key}
          <div class="knowledge-hero__identity">
            {#if activePortrait}<Portrait boss={activePortrait} size="md" />{/if}
            <div><p class="cap">Checked-in boss knowledge</p><h2>{knowledge.data.name}{#if shownDifficulty}<span class="knowledge-hero__pill boss-tick boss-tick--{tickClass(shownDifficulty)}"><span class="vh">, </span>{shownDifficulty.toUpperCase()}</span>{/if}</h2><p class="knowledge-hero__meta" title={heroMeta}>{knowledge.data.level ? `Lv. ${knowledge.data.level} · ` : ''}researched {knowledge.data.researched_as_of ?? 'undated'} · <code>{knowledge.data.path}</code></p></div>
            {#if doc.event}<StatusChip>Seasonal boss · <abbr title={doc.event.name}>{seasonTag(doc.event)}</abbr></StatusChip>{/if}
          </div>
        </header>
        <div data-fid="knowledge-body" class="knowledge-detail__body" {@attach collapseHero}>
          <!-- Keyed by boss: the difficulty and open steps start fresh, the tab is read again from the address. -->
          {#key knowledge.data.key}
            <KnowledgeGuide knowledge={knowledge.data} {difficulty} bind:shown={shownDifficulty}>
              {#snippet timings()}<h2 class="cap">Weekly timings</h2>{#if relatedFixed.length}<ul class="knowledge-aside__timings">{#each relatedFixed as timing (timing.id)}{@const boss = timingBoss(timing)}{@const others = otherBosses(timing)}<li><a href="/fixed?open={encodeURIComponent(timing.id)}"><strong>{timing.weekday_name.slice(0, 3)} {timing.time}</strong>{#if boss}<span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>{/if}{#if others.length}<span class="knowledge-aside__others" title={others.join(' · ')}>+ {others.join(' · ')}</span>{/if}</a></li>{/each}</ul>{:else}<p class="note">No weekly timing uses this boss.</p>{/if}<h2 class="cap">This week</h2><p class="knowledge-aside__count">{relatedRuns.length} run{relatedRuns.length === 1 ? '' : 's'}{nextRun ? ' · next ' : ''}{#if nextRun}<strong>{runWhen(nextRun)}</strong>{/if}</p>{/snippet}
            </KnowledgeGuide>
          {/key}
        </div>
      {:else}<LoadingState text="Loading checked-in knowledge…" />{/if}
    </article>
  </div>
</section>
