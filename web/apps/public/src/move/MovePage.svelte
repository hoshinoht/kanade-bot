<!--
  A run's Move page, `/runs/{id}` (boards Move, Move-NotIn, Move-WeekOver,
  PhoneMove, PhoneMove-NotIn, PhoneMove-WeekOver): where a Discord link
  (`?move_to=<RFC 3339>`) and "Move this week…" land. It reads the run as
  the member may see it now (`GET /api/public/runs/{id}`), then either the
  Move form over this boss week (MemberMove: the link's slot picked, the
  checks, Move it / Cancel; on a phone the foot stays at the bottom), or why
  the link went stale — the week is over, the member left the party, the run
  started, closed or sits in a later week — with the run as it is now and
  the way on ("Ask for a change…", "Ask to join…", open the run). A move
  says "Moved … to …" and lands on the Week with the run open.
-->
<script lang="ts">
  import type { MemberMoveResult, MemberRun, PublicSession, PublicSessionRow } from '@kanade/api-types';
  import { BossArt, BossTag, dayLabel, Icon, LoadingState, runTitle, StateNote, STATUS_WORDS, weekStartLabel, type IconName, type Toaster } from '@kanade/ui';
  import { untrack } from 'svelte';
  import { countdownWords, yours } from '../member';
  import { weekly } from '../requests/form';
  import type { Route } from '../route.svelte';
  import type { MemberWeeks } from '../weeks.svelte';
  import ConfirmRun from '../writes/ConfirmRun.svelte';
  import { RunFlow } from '../writes/flow.svelte';
  import { follow, followed } from '../writes/follow';
  import type { RunWrites } from '../writes/runWrites.svelte';
  import { askedWords, askPath, linkState, slotOf, wallWords, weekEnds, weeklyAskPath, type LinkState } from '../writes/runs';
  import MemberMove from './MemberMove.svelte';
  import './move.scss';

  let {
    route,
    runs: writes,
    weeks,
    session,
    current,
    toaster,
    phone,
    zone,
  }: {
    route: Route;
    /** The member's answers and moves (`Portal.runs`). */
    runs: RunWrites;
    weeks: MemberWeeks;
    session: PublicSession;
    current: PublicSessionRow | null;
    toaster: Toaster;
    phone: boolean;
    /** The guild's zone as an offset ("GMT+8"), for the picker's legend. */
    zone: string;
  } = $props();

  const memberId = $derived(session.member.id);
  const flow = untrack(() => new RunFlow(writes, toaster));
  const id = $derived(route.runId);
  const moveTo = $derived(route.params.get('move_to') ?? '');

  let read = $state<Exclude<Awaited<ReturnType<RunWrites['link']>>, null> | 'loading'>('loading');
  async function load(runId: string) {
    const got = await writes.link(runId);
    // A slow answer for a run the page has left is dropped; null: the portal replaced the screen.
    if (got && runId === id) read = got;
  }
  $effect(() => {
    const runId = id;
    untrack(() => {
      read = 'loading';
      void load(runId);
    });
  });

  const link = $derived(read !== 'loading' && read.kind === 'ok' ? read.link : null);
  const timeZone = $derived(weeks.this?.timezone ?? Intl.DateTimeFormat().resolvedOptions().timeZone);
  // The run on this week as the weeks hold it now (answers and moves land there first).
  const live = $derived(link ? weeks.find(link.run.id) : null);
  const shape: LinkState | 'gone' | 'waiting' = $derived.by(() => {
    if (!link) return 'waiting';
    const said = linkState(link, memberId);
    if (said !== 'move') return said;
    if (!weeks.this) return 'waiting';
    return live?.which === 'this' ? 'move' : 'gone';
  });
  const run = $derived(shape === 'move' && live ? live.run : (link?.run ?? null));
  const title = $derived(run ? runTitle(run) : '');
  const asked = $derived(moveTo ? askedWords(moveTo, timeZone) : '');
  const initial = $derived(moveTo && weeks.this ? (slotOf(moveTo, weeks.this) ?? undefined) : undefined);
  const stale = $derived(shape !== 'move' && shape !== 'waiting');
  // Where the run opens on the Week; the past week has none.
  const openPath = $derived(link && link.week !== 'past' ? `/?${new URLSearchParams({ ...(link.week === 'next' ? { week: 'next' } : {}), run: link.run.id })}` : '/');

  const NOTICE_ICON: Record<Exclude<LinkState, 'move'> | 'gone', IconName> = {
    week_over: 'clock',
    not_in: 'users',
    closed: 'alert-circle',
    started: 'clock',
    not_yet: 'calendar',
    gone: 'alert-circle',
  };

  /** The Week with the run open (after a move, or Cancel from a Discord link). */
  function toWeek() {
    route.go('/', link?.run.id ? { run: link.run.id } : {});
  }

  function cancel() {
    if (followed(history.state)) history.back();
    else toWeek();
  }

  function onmoved(result: MemberMoveResult) {
    if (weeks.this) flow.moved(result, weeks.this);
    toWeek();
  }
</script>

{#snippet go(href: string, label: string, primary: boolean)}
  <a class="btn btn--key" class:btn--primary={primary} class:btn--full={phone} {href} onclick={(event) => follow(route, event, href)}>{label}</a>
{/snippet}

{#snippet runRow(label: string, shown: MemberRun | null)}
  {#if shown}
    {@const w = weeks.find(shown.id)?.week ?? null}
    {@const soon = w ? countdownWords(shown, w) : ''}
    <!-- The Move form names it inside the row ("NOW 22:00 …", board Move); a stale link labels it above (Move-NotIn). -->
    <div class="field move-page__now">
      {#if shape !== 'move'}<span class="label">{label}</span>{/if}
      <div class="move-page__run" data-fid="move-run">
        {#if shape === 'move'}<span class="cap">{label}</span>{/if}
        <span class="mono move-page__clock">{shown.time ?? 'own time'}</span>
        {#if w}<span class="cap">{dayLabel(w, shown.day)}{soon ? ` · ${soon}` : ''}</span>{/if}
        <span class="move-page__bosses">{#each shown.bosses as boss (boss.token)}<BossTag {boss} short />{/each}</span>
        {#if shape === 'not_in'}
          <span class="member-card__view move-page__chip">{phone ? 'view only' : 'not in this run · view only'}</span>
        {:else}
          <span class="status-chip move-page__chip" class:status-chip--risk={shown.status === 'at_risk'} class:status-chip--ok={shown.status === 'done' || shown.status === 'confirmed'}
            >{yours(shown, memberId) && shape !== 'move' ? "you're in · " : ''}{STATUS_WORDS[shown.status]} · {shown.tally.on} of {shown.tally.total} on</span
          >
        {/if}
      </div>
      {#if shape === 'not_in'}
        <p class="field__hint">Party: {shown.participants.map((p) => p.name).join(' · ')} · {shown.tally.on} of {shown.tally.total}</p>
      {:else if shape === 'week_over' && link?.timing}
        <p class="field__hint">Your weekly timing is {weekly(link.timing.day, link.timing.time)}.</p>
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet notice()}
  {#if link && shape !== 'move' && shape !== 'waiting'}
    {@const ended = wallWords(Date.parse(link.week_ends_at) - 60_000, timeZone)}
    {@const removedOn = link.removed ? wallWords(link.removed.at, timeZone) : null}
    <p class="flash flash--warn move-page__notice" role="status" data-fid="move-notice">
      <Icon name={NOTICE_ICON[shape]} />
      <span>
        {#if shape === 'week_over'}
          <b>That boss week is over.</b> The link was for the week of {weekStartLabel(link.week_starts)}, which ended at reset{ended ? ` on ${ended.date}` : ''}, so nothing was
          moved.
        {:else if shape === 'not_in'}
          {#if link.removed}
            <b>You're no longer in this party.</b> {link.removed.by} removed you from {title}{removedOn ? ` on ${removedOn.date}` : ''}, so you can't move it. The run is unchanged.
          {:else}
            <b>You're not in this party.</b> Only its party can move {title}, so nothing changed.
          {/if}
        {:else if shape === 'started'}
          <b>That run has started.</b> {title} can't move once it is under way, so nothing was moved.
        {:else if shape === 'closed'}
          <b>That run is {link.run.status === 'cancelled' ? 'cancelled' : 'done'}.</b> Nothing was moved.
        {:else if shape === 'not_yet'}
          <b>That run is in {link.week === 'next' ? 'next' : 'a later'} boss week.</b> Moves open once its week starts ({weekStartLabel(link.week_starts)}); until then, ask the admins.
        {:else}
          <b>That run is no longer on this week.</b> Nothing was moved.
        {/if}
      </span>
    </p>
  {/if}
{/snippet}

{#snippet staleActions()}
  {#if link}
    {#if shape === 'week_over'}
      {@render go(link.this_week ? `/?run=${encodeURIComponent(link.this_week.id)}` : '/', "Open this week's runs", true)}
      {@render go(link.run.fixed_id ? weeklyAskPath(link.run.fixed_id) : askPath(link.this_week ?? link.run), 'Ask for a change…', false)}
    {:else if shape === 'not_in'}
      {@render go(askPath(link.run), 'Ask to join…', true)}
      {@render go(openPath, 'Open the run', false)}
    {:else}
      {@render go(openPath, 'Open the run', true)}
      {@render go(askPath(link.run), 'Ask for a change…', false)}
    {/if}
  {/if}
{/snippet}

<section class="card move-page" class:move-page--phone={phone} class:window-fill={phone} aria-label="Move your run" data-fid="window">
  <div class="card__head" data-fid="window-bar">
    <span class="card__title">Move your run</span>
    {#if moveTo}<span class="card__head-end move-page__from">{phone ? 'from Discord' : 'from a Discord link'}</span>{/if}
  </div>
  {#if read === 'loading' || (shape === 'waiting' && read.kind === 'ok')}
    <div class="move-page__state"><LoadingState text="Loading the run…" /></div>
  {:else if read.kind === 'missing'}
    <div class="move-page__state">
      <StateNote icon="alert-circle" level={1} title="Kanade can't find that run">
        It isn't on this boss week or the next, and you weren't in it.
        {#snippet actions()}{@render go('/', 'Open the week', true)}{/snippet}
      </StateNote>
    </div>
  {:else if read.kind === 'failed'}
    <div class="move-page__state">
      <StateNote tone="error" icon="alert-circle" level={1} title="The run didn't load">
        {read.words}
        {#snippet actions()}<button type="button" class="btn btn--primary" onclick={() => void load(id)}>Try again</button>{/snippet}
      </StateNote>
    </div>
  {:else if run}
    {@const art = run.bosses.find((b) => b.art)}
    <header class="move-page__head" class:move-page__head--stale={stale}>
      {#if art}<BossArt class="move-page__art" still={art.art} animated={art.animated} />{/if}
      <span class="eyebrow">{moveTo ? (stale ? 'The link asked' : 'From your Discord link') : stale ? 'This run' : 'Your run'}</span>
      <h1 class="move-page__title">
        {#if stale}<s>{asked ? `Move ${title} to ${asked}?` : `Move ${title}`}</s>{:else}{asked ? `Move ${title} to ${asked}?` : `Move ${title} this week`}{/if}
      </h1>
      <span class="move-page__sub">{run.bosses.map((b) => b.name).join(' + ')} · {run.channel}{yours(run, memberId) && !stale ? " · you're in" : ''}</span>
    </header>
    {#if shape === 'move' && live}
      <div class="move-page__form">
        {#if !phone}{@render runRow('Now', live.run)}{/if}
        <MemberMove
          run={live.run}
          week={live.week}
          {memberId}
          {flow}
          variant={phone ? 'phone' : 'page'}
          {initial}
          ends={link ? weekEnds(link.week_ends_at, timeZone) : ''}
          step={15}
          legend={phone ? 'Day' : `New time · guild time (${zone})`}
          stepHint={phone ? '' : 'Type a day and time, or step 15 minutes'}
          note={phone
            ? 'This week only. The party is told on Discord and admins can undo it.'
            : `Only this week moves${live.run.fixed_id ? '; your weekly timing stays the same' : ''}. The party is told in ${live.run.channel}, and admins can undo.`}
          submitLabel={phone ? 'Move' : 'Move it'}
          fids={{ picker: 'move-picker', time: 'move-time', type: 'move-type', suggest: 'move-suggest' }}
          {onmoved}
          onrefused={() => void load(id)}
          oncancel={cancel}
        />
      </div>
    {:else}
      <div class="move-page__body">
        {@render notice()}
        {@render runRow(shape === 'week_over' ? "This week's run" : 'The run now', shape === 'week_over' ? (link?.this_week ?? null) : run)}
        {#if !phone}<div class="move-page__acts">{@render staleActions()}</div>{/if}
        {#if moveTo}<p class="field__hint">Links from Discord carry only the run and the time; the portal checks everything again when you open them.</p>{/if}
      </div>
      {#if phone}<div class="move-page__foot" data-fid="move-foot">{@render staleActions()}</div>{/if}
    {/if}
  {/if}
</section>
<ConfirmRun {flow} {session} {current} {phone} />
