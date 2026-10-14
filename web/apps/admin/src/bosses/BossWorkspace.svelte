<!--
  Admin Bosses (B_Bosses): the shared `BossesWindow` over the admin reads,
  with the page line's counts, the knowledge file's path in the hero and the
  aside's weekly timings and this week's runs for the open boss.
-->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import type { BossRow, EventBoss, FixedRow, Knowledge, Run, Week } from '@kanade/api-types';
  import { BossesWindow, DIFFICULTY_WORDS, dayLabel } from '@kanade/ui';
  import { Resource } from '../resource.svelte';

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
  const fixed = new Resource<FixedRow[]>('/api/admin/fixed', { topics: ['schedule'] });
  const week = new Resource<Week>('/api/admin/week?week=this', { topics: ['schedule'], version: (w) => w.version });
  $effect(() => {
    void bosses.load();
    void events.load();
    const unfollow = [fixed.watch(), week.watch()];
    return () => unfollow.forEach((stop) => stop());
  });

  const catalog = $derived(bosses.data ?? []);
  const eventRows = $derived(events.data ?? []);
  const fallback = $derived(catalog[0]?.key ?? eventRows[0]?.key ?? '');
  const activeKey = $derived(selectedKey || fallback);
  const knowledge = $derived(new Resource<Knowledge>(`/api/admin/bosses/${encodeURIComponent(activeKey)}/knowledge`));
  $effect(() => {
    if (activeKey) void knowledge.load();
  });

  const total = $derived(catalog.reduce((n, row) => n + row.difficulties.length, 0));
  const inUse = $derived(catalog.reduce((count, row) => count + row.difficulties.filter((difficulty) => difficulty.in_use).length, 0));
  const relatedFixed = $derived((fixed.data ?? []).filter((row) => row.bosses.some((boss) => boss.key === activeKey)));
  const relatedRuns = $derived(
    [...(week.data?.runs ?? [])]
      .filter((run) => run.bosses.some((boss) => boss.key === activeKey))
      .sort((left, right) => left.day - right.day || (left.time ?? '99:99').localeCompare(right.time ?? '99:99')),
  );
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
  // The header is one line (cut with an ellipsis on narrow frames); the title holds it whole.
  const heroLine = $derived(knowledge.data ? `${knowledge.data.level ? `Lv. ${knowledge.data.level} · ` : ''}researched ${knowledge.data.researched_as_of ?? 'undated'} · ${knowledge.data.path}` : '');
  const runWhen = (run: Run) => `${dayLabel(week.data!, run.day)} ${run.time ?? 'own time'}`;
  const timingBoss = (timing: FixedRow) => timing.bosses.find((boss) => boss.key === activeKey) ?? null;
  const otherBosses = (timing: FixedRow) => timing.bosses.filter((boss) => boss.key !== activeKey).map((boss) => boss.name);
</script>

<BossesWindow
  rows={bosses.data}
  rowsError={bosses.error}
  onretry={() => void bosses.load()}
  events={eventRows}
  knowledge={knowledge.data}
  knowledgeError={knowledge.error}
  {activeKey}
  {selectedKey}
  {difficulty}
  {onselect}
  chrome={getChrome()}
  heroCap="Checked-in boss knowledge"
  heroTitle={heroLine}
>
  {#snippet pageLine()}
    <PageLine title={bosses.data ? 'Bosses' : ''}>
      <h1>{#if bosses.data}<span class="pageline__num">{catalog.length}</span> bosses, <span class="pageline__num">{total}</span> difficulties{:else}Bosses{/if}</h1>
      {#if bosses.data}<p class="pageline__context"><strong>{inUse}</strong> ticked with a weekly timing</p>{/if}
    </PageLine>
  {/snippet}
  {#snippet heroMeta()}{#if knowledge.data}{knowledge.data.level ? `Lv. ${knowledge.data.level} · ` : ''}researched {knowledge.data.researched_as_of ?? 'undated'} · <code>{knowledge.data.path}</code>{/if}{/snippet}
  {#snippet timings()}<h2 class="cap">Weekly timings</h2>{#if relatedFixed.length}<ul class="knowledge-aside__timings">{#each relatedFixed as timing (timing.id)}{@const boss = timingBoss(timing)}{@const others = otherBosses(timing)}<li><a href="/fixed?open={encodeURIComponent(timing.id)}"><strong>{timing.weekday_name.slice(0, 3)} {timing.time}</strong>{#if boss}<span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>{/if}{#if others.length}<span class="knowledge-aside__others" title={others.join(' · ')}>+ {others.join(' · ')}</span>{/if}</a></li>{/each}</ul>{:else}<p class="note">No weekly timing uses this boss.</p>{/if}<h2 class="cap">This week</h2><p class="knowledge-aside__count">{relatedRuns.length} run{relatedRuns.length === 1 ? '' : 's'}{nextRun ? ' · next ' : ''}{#if nextRun}<strong>{runWhen(nextRun)}</strong>{/if}</p>{/snippet}
</BossesWindow>
