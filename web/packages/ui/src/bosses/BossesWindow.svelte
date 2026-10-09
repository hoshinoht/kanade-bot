<!--
  The Bosses window (boards B_Bosses; portal Bosses, Guide*, PhoneBosses,
  PhoneGuide*), shared by the admin app and the member portal: the catalog
  (`BossGrid`) and event bosses on the left, the open boss's knowledge hero
  and guide on the right; below 900 px one pane at a time. Each app loads
  the data and supplies its page line, links, hero meta and aside extras.
-->
<script lang="ts">
  import type { BossRow, EventBoss, PublicKnowledge } from '@kanade/api-types';
  import { tick, untrack, type Snippet } from 'svelte';
  import BossArt from '../components/BossArt.svelte';
  import LoadError from '../components/LoadError.svelte';
  import LoadingState from '../components/LoadingState.svelte';
  import Portrait from '../components/Portrait.svelte';
  import RowContent from '../components/RowContent.svelte';
  import StatusChip from '../components/StatusChip.svelte';
  import { entryArt } from '../bossArt';
  import { SINGLE_PANE_QUERY } from '../media';
  import { enter } from '../motion/enter';
  import BossGrid from './BossGrid.svelte';
  import KnowledgeGuide from './KnowledgeGuide.svelte';
  import { LETTER, type BossFids } from './guide';
  import { eventAsBoss, seasonTag } from './event';
  import '@kanade/ui/styles/boss-knowledge.scss';

  /** The phone frame's top bar, where a page's own back step goes (admin `Chrome`). */
  interface BackBar {
    readonly phone: boolean;
    back(step: { readonly label: string; readonly name: string; go(): void } | null): void;
  }

  let {
    rows,
    rowsError = '',
    onretry,
    events,
    knowledge,
    knowledgeError = '',
    activeKey,
    selectedKey = '',
    difficulty = '',
    onselect,
    href = (key: string) => `/bosses/${key}/knowledge`,
    chrome,
    pageLine,
    heroCap,
    heroTitle,
    heroMeta,
    heroBadge,
    compactBar,
    pick,
    timings,
    asideLabel,
    fids = {},
  }: {
    /** The catalog; null while it loads. */
    rows: BossRow[] | null;
    rowsError?: string;
    onretry: () => void;
    events: EventBoss[];
    /** The open boss's knowledge; null while it loads or with none open. */
    knowledge: PublicKnowledge | null;
    knowledgeError?: string;
    /** The boss shown (and marked) in the catalog; empty: none. */
    activeKey: string;
    /** The boss in the address; empty: the catalog alone in one pane. */
    selectedKey?: string;
    difficulty?: string;
    /** Opens a boss (empty: the catalog); `open` pushes a history entry (single pane: Back returns to the catalog). */
    onselect?: (key: string, open: boolean) => void;
    /** A boss's address. */
    href?: (key: string) => string;
    chrome?: BackBar;
    /** Above the window, except where one pane shows an open boss in the phone frame. */
    pageLine?: Snippet;
    /** The hero's caption over the boss's name. */
    heroCap: string;
    /** The whole meta line, for its `title` (it is cut with an ellipsis on narrow frames). */
    heroTitle: string;
    heroMeta: Snippet;
    /** Beside the identity, after the season chip. */
    heroBadge?: Snippet;
    /** One pane with a boss open in the phone frame: a title bar for the guide (none: no bar). */
    compactBar?: Snippet;
    /** The detail with no boss open (none: the first boss opens instead). */
    pick?: Snippet;
    /** Below "On this page" in the aside. */
    timings?: Snippet;
    asideLabel?: string;
    /** Fidelity tags for regions only the member portal's boards draw. */
    fids?: BossFids;
  } = $props();

  const catalog = $derived(rows ?? []);
  const activeEvent = $derived(events.find((boss) => boss.key === activeKey));
  const activeBoss = $derived(catalog.find((boss) => boss.key === activeKey));
  const doc = $derived(knowledge?.doc);
  // The boss whose knowledge is on screen: its pane content enters when it changes.
  const shownKey = $derived(doc && knowledge ? knowledge.key : null);
  const facts = $derived(doc?.difficulties ?? []);
  /** Knowledge-only difficulties (never scheduled): no catalog letter, own tick colours. */
  const infoOnly = $derived(facts.filter((fact) => !LETTER[fact.name]).map((fact) => fact.name));
  const tickClass = (name: string) => LETTER[name] ?? name.toLowerCase();
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

  /** The catalog key a link opens, if it is a boss link. */
  function linkKey(link: HTMLAnchorElement): string | undefined {
    const keys = [...catalog.map((row) => row.key), ...events.map((boss) => boss.key)];
    return keys.find((key) => new URL(href(key), location.href).pathname === link.pathname);
  }

  /** Single pane: a catalog pick pushes a tagged entry instead of the router's plain one. */
  function tagPicks(nav: HTMLElement) {
    const onclick = (event: MouseEvent) => {
      if (!phone || !onselect || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href]') : null;
      const key = link ? linkKey(link) : undefined;
      if (!key) return;
      event.preventDefault();
      onselect(key, true);
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
    else if (!now && before) void tick().then(() => navEl?.querySelector<HTMLElement>(`a[href="${CSS.escape(href(before))}"]`)?.focus({ preventScroll: true }));
  });

  const asBoss = (row: BossRow) => ({ token: row.key, key: row.key, name: row.name, difficulty: 'n' as const, level: row.level, portrait: row.portrait, portrait_sm: row.portrait, art: null, animated: null, hue: row.hue });
  const activePortrait = $derived(
    knowledge
      ? { token: knowledge.key, key: knowledge.key, name: knowledge.name, difficulty: 'n' as const, level: knowledge.level, portrait: knowledge.portrait, portrait_sm: knowledge.portrait, art: null, animated: null, hue: knowledge.hue }
      : activeBoss
        ? asBoss(activeBoss)
        : activeEvent
          ? eventAsBoss(activeEvent)
          : null,
  );
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
    // Reattach for a new boss, not for scrolling or same-boss guide changes.
    void shownKey;
    heroCompact = false;
    panel.scrollTop = 0;
    const onscroll = () => {
      const room = panel.scrollHeight - panel.clientHeight;
      if (!heroCompact && panel.scrollTop > 96 && room > 160) heroCompact = true;
      else if (heroCompact && panel.scrollTop < 12) heroCompact = false;
    };
    panel.addEventListener('scroll', onscroll, { passive: true });
    return () => panel.removeEventListener('scroll', onscroll);
  }
</script>

{#if !compact}{@render pageLine?.()}{/if}

<section data-fid="window" class="card bosses-window window-fill" aria-labelledby="bosses-title" class:bosses-window--compact={compact}>
  {#if !compact}<div class="card__head" data-fid="window-bar"><h2 class="card__title" id="bosses-title">The in-game list</h2><span class="bosses-window__order" data-fid={fids.order}>level order</span></div>
  {:else}{@render compactBar?.()}{/if}
  <div class="bosses-window__body">
    <nav data-fid="boss-list" class="bosses-list" aria-label="Boss catalog" class:bosses-list--hidden={phone && Boolean(selectedKey)} bind:this={navEl} {@attach tagPicks}>
      {#if rowsError}<LoadError thing="the boss list" reason={rowsError} {onretry} level={3} />
      {:else if rows}<BossGrid rows={catalog} readonly active={activeKey} {infoOnly} {href} />
        {#if events.length}
          <h3 class="cap bosses-list__event-title">Event bosses</h3>
          <ul class="bosses-events" aria-label="Event bosses" data-fid={fids.events} {@attach forwardEventClicks}>
            {#each events as boss (boss.key)}
              <li class="expandable-row" data-fid="boss-row" class:bosses-events__active={boss.key === activeKey}><a href={href(boss.key)} aria-current={boss.key === activeKey ? 'true' : undefined}><Portrait boss={eventAsBoss(boss)} size="md" /><strong>{boss.key}</strong></a><RowContent expanded={boss.key === activeKey}>{#snippet compact()}<StatusChip>Seasonal boss · <abbr title={boss.event.name}>{seasonTag(boss.event)}</abbr></StatusChip>{/snippet}<span class="bossrow__difficulties" role="group" aria-label="{boss.key} difficulties">{#each boss.key === knowledge?.key ? facts : [] as fact (fact.name)}<span><span class="boss-tick boss-tick--{tickClass(fact.name)}">{fact.name.toUpperCase()}</span>{#if fact.boss_level ?? fact.entry_level}<span class="bossrow__tracked">{` Lv. ${fact.boss_level ?? fact.entry_level}`}</span>{/if}</span>{/each}</span></RowContent></li>
            {/each}
          </ul>
        {/if}
      {:else}<LoadingState text="Loading the boss list…" />{/if}
    </nav>
    <article data-fid="knowledge-detail" class="knowledge-detail" class:knowledge-detail--hidden={!selectedKey && phone} tabindex="-1" bind:this={detailEl} {@attach enter(shownKey)}>
      <!-- One pane below 900 px: the rail frame's way back (the phone frame's is in the top bar). -->
      {#if phone && selectedKey && !compact}<button type="button" class="btn btn--ghost knowledge-detail__back" onclick={leaveDetail}><span aria-hidden="true">←</span> Back to the catalog</button>{/if}
      {#if !activeKey && pick}{@render pick()}
      {:else if knowledgeError}<div class="empty" role="alert"><strong>No knowledge for “{activeKey}”.</strong>{knowledgeError}</div>
      {:else if doc && knowledge}
        <header data-fid="knowledge-head" class="knowledge-hero" class:knowledge-hero--compact={heroCompact}>
          <BossArt class="knowledge-hero__art" still={entryArt(activeKey)} animated={knowledge.animated} />
          <div class="knowledge-hero__identity">
            {#if activePortrait}<Portrait boss={activePortrait} size="md" />{/if}
            <div><p class="cap">{heroCap}</p><h2>{knowledge.name}{#if shownDifficulty}<span class="knowledge-hero__pill boss-tick boss-tick--{tickClass(shownDifficulty)}"><span class="vh">, </span>{shownDifficulty.toUpperCase()}</span>{/if}</h2><p class="knowledge-hero__meta" title={heroTitle}>{@render heroMeta()}</p></div>
            {#if doc.event}<StatusChip>Seasonal boss · <abbr title={doc.event.name}>{seasonTag(doc.event)}</abbr></StatusChip>{/if}{@render heroBadge?.()}
          </div>
        </header>
        <div data-fid="knowledge-body" class="knowledge-detail__body" {@attach collapseHero}>
          <!-- Keyed by boss: the difficulty and open steps start fresh, the tab is read again from the address. -->
          {#key knowledge.key}
            <KnowledgeGuide {knowledge} {difficulty} bind:shown={shownDifficulty} {timings} {asideLabel} {fids} />
          {/key}
        </div>
      {:else}<LoadingState text="Loading checked-in knowledge…" />{/if}
    </article>
  </div>
</section>
