<!--
  Bosses (catalog C7/C8; boards Bosses, Guide, Guide-Phases, Guide-Destiny,
  Guide-Event, PhoneBosses, PhoneGuide*): the admin Bosses window
  (`BossesWindow`, @kanade/ui) over the member boss reads, read-only. `/bosses`
  is the catalog with "Pick a boss" beside it; `/bosses/{key}` opens a guide
  (`?difficulty=`, `?tab=`, `?phase=` as admin). No weekly-timings aside.
  In the phone frame an open guide puts "← {boss}" in the top bar.
-->
<script lang="ts">
  import type { PublicKnowledge } from '@kanade/api-types';
  import { BossesWindow, StateNote, StatusChip, type BossFids } from '@kanade/ui';
  import { untrack } from 'svelte';
  import type { Route } from '../route.svelte';
  import type { MemberBosses } from './bosses.svelte';
  import './bosses.scss';

  let {
    bosses,
    route,
    phone,
    onback,
  }: {
    bosses: MemberBosses;
    route: Route;
    /** The phone frame (top bar + drawer). */
    phone: boolean;
    /** The top bar's back step for an open guide; null clears it. */
    onback: (step: { label: string; name: string; go: () => void } | null) => void;
  } = $props();

  const href = (key: string) => `/bosses/${encodeURIComponent(key)}`;
  const selectedKey = $derived(route.path.startsWith('/bosses/') ? decodeURIComponent(route.path.slice('/bosses/'.length)) : '');
  const guide = $derived(bosses.guide?.key === selectedKey ? bosses.guide : null);
  // Every visit reads the list afresh (cheap, and art and timings change).
  $effect(() => untrack(() => void bosses.load()));
  $effect(() => {
    const key = selectedKey;
    if (key) untrack(() => void bosses.open(key));
  });

  function select(key: string, open: boolean) {
    route.go(key ? href(key) : '/bosses', {}, { state: key && open ? { bossDetail: true } : null, replace: !open });
  }

  // The phone frame's top bar goes back to the catalog, named by the open boss (board PhoneGuide).
  let step = $state<{ name: string; go: () => void } | null>(null);
  const chrome = {
    get phone() {
      return phone;
    },
    back: (next: { name: string; go: () => void } | null) => (step = next),
  };
  $effect(() => {
    onback(step ? { label: guide?.data?.name ?? selectedKey, name: step.name, go: step.go } : null);
  });
  $effect(() => () => onback(null));

  /** Wide frames: a catalog link opens the guide in place (a new entry), as admin's router does. */
  function followLinks(node: HTMLElement) {
    const onclick = (event: MouseEvent) => {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href^="/bosses"]') : null;
      if (!link) return;
      event.preventDefault();
      route.go(link.pathname);
    };
    node.addEventListener('click', onclick);
    return () => node.removeEventListener('click', onclick);
  }

  const partyWords = (knowledge: PublicKnowledge) => {
    const most = Math.max(0, ...(knowledge.doc.difficulties ?? []).map((fact) => fact.party_max ?? 0));
    return most === 1 ? 'solo' : most ? `up to ${most}` : '';
  };
  const metaParts = (knowledge: PublicKnowledge) => {
    const sources = knowledge.doc.sources.length;
    return [
      knowledge.level ? `Lv. ${knowledge.level}` : '',
      partyWords(knowledge),
      `researched ${knowledge.researched_as_of ?? 'undated'}`,
      `${sources} source${sources === 1 ? '' : 's'}`,
    ].filter(Boolean);
  };
  const FIDS: BossFids = {
    order: 'window-filters',
    events: 'event-bosses',
    difficulty: 'guide-difficulty',
    mission: 'guide-mission',
    event: 'guide-event',
    tiles: 'guide-tiles',
    hp: 'guide-hp',
    notes: 'guide-notes',
    tabs: 'guide-tabs',
    toc: 'guide-toc',
  };
</script>

<div class="member-bosses" {@attach followLinks}>
  <BossesWindow
    rows={bosses.rows}
    rowsError={bosses.error}
    onretry={() => void bosses.load()}
    events={bosses.events}
    knowledge={guide?.data ?? null}
    knowledgeError={guide?.error ?? ''}
    activeKey={selectedKey}
    {selectedKey}
    difficulty={route.params.get('difficulty') ?? ''}
    onselect={select}
    {href}
    {chrome}
    heroCap="Boss guide"
    heroTitle={guide?.data ? metaParts(guide.data).join(' · ') : ''}
    asideLabel="On this page"
    fids={FIDS}
  >
    {#snippet pageLine()}
      <div class="pageline" data-fid="page-line">
        <div class="pageline__head">
          <h1 class="pageline__title">Bosses</h1>
          <p class="pageline__context">· guides for every boss, read-only</p>
        </div>
      </div>
    {/snippet}
    {#snippet heroMeta()}{#if guide?.data}{metaParts(guide.data).join(' · ')}{/if}{/snippet}
    {#snippet heroBadge()}<StatusChip>read-only</StatusChip>{/snippet}
    {#snippet compactBar()}<div class="card__head" data-fid="window-bar"><h2 class="card__title" id="bosses-title">Boss guide</h2><span class="member-bosses__ro"><StatusChip>read-only</StatusChip></span></div>{/snippet}
    {#snippet pick()}
      <div class="state-pane member-bosses__pick" data-fid="state-pane">
        <StateNote icon="shield" title="Pick a boss to read its guide">
          Phases, strategies and HP for each difficulty. Champion and Destiny are shown for reference; the guild doesn't schedule them.
        </StateNote>
        <p class="note">Party size is the most the game allows in one run.</p>
      </div>
    {/snippet}
  </BossesWindow>
</div>
