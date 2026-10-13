<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/screens/members.scss';
  import type { MemberRow, Persona, PingLevel, Week } from '@kanade/api-types';
  import { Avatar, LoadError, LoadingState, PHONE_QUERY, Presence, RowContent, Select, TWO_PANE_QUERY, watchMedia, type Toaster } from '@kanade/ui';
  import '@kanade/ui/styles/core/select.scss';
  import Pager from '../pages/Pager.svelte';
  import { paged } from '../pages/paging';
  import { memberLabel } from '../names/directory.svelte';
  import { Resource } from '../resource.svelte';
  import MemberSheet from './MemberSheet.svelte';
  import { provideDialogLayer } from './layer.svelte';
  import { memberAvatar } from '../shared/avatar';
  import { memberRuns, orderMembers, runCounts, type MemberOrder } from './runs';

  let {
    roster = false,
    toaster,
  }: {
    /** The character roster module is on (from the session): the sheet gains Characters. */
    roster?: boolean;
    toaster: Toaster;
  } = $props();
  // The open sheet reads its row from this list, so both follow a roster change.
  const members = new Resource<MemberRow[]>('/api/admin/members', { topics: ['members'] });
  const personas = new Resource<Persona[]>('/api/admin/personas');
  // The detail's "This week" list reads the runs the member is on.
  const week = new Resource<Week>('/api/admin/week?week=this', { topics: ['schedule'], version: (w) => w.version });
  $effect(() => {
    void personas.load();
    const unfollow = [members.watch(), week.watch()];
    return () => unfollow.forEach((stop) => stop());
  });
  const PING: Record<PingLevel, string> = { essential: 'Essential', all: 'All', off: 'Off' };

  let query = $state('');
  // `?open=<id>` (Account's "Open my member profile") opens that member's sheet.
  let openId = $state<string | null>(new URLSearchParams(location.search).get('open'));
  let wide = $state(false);
  let phone = $state(false);
  // The board's default: most runs this week first.
  let order = $state<MemberOrder>('runs');
  let restore = '';

  const q = $derived(query.trim().toLowerCase());
  // Counted from the week the run list already loads; the roster's own count until it arrives.
  const counts = $derived(week.data ? runCounts(week.data) : null);
  const rows = $derived(
    orderMembers(
      (members.data ?? []).filter((m) => !q || [m.name, m.nickname ?? '', ...m.aliases].some((t) => t.toLowerCase().includes(q))),
      order,
      (m) => memberLabel(members.data ?? [], m.id),
      (m) => (counts ? (counts.get(m.id) ?? 0) : m.runs_this_week),
    ),
  );
  let page = $state(1);
  // A new search starts at page one (v4 dropped `page` from the search form).
  $effect(() => {
    void q;
    void order;
    page = 1;
  });
  const shown = $derived(paged(rows, page));
  const bossers = $derived((members.data ?? []).filter((m) => m.bossing).length);
  const current = $derived(members.data?.find((m) => m.id === openId) ?? null);
  // The pane outlives the selection by its exit animation.
  const pane = new Presence<MemberRow>();
  $effect(() => pane.set(current));
  // While open the sheet reads the live member (the presence copy lags an effect behind).
  const paneMember = $derived(current ?? pane.shown);
  const paneRuns = $derived(paneMember && week.data ? memberRuns(week.data, paneMember.id) : null);
  const dialogs = provideDialogLayer();

  $effect(() => watchMedia(TWO_PANE_QUERY, (matches) => (wide = matches)));
  $effect(() => watchMedia(PHONE_QUERY, (matches) => (phone = matches)));

  function replace(row: MemberRow) {
    if (members.data) members.data = members.data.map((m) => (m.id === row.id ? row : m));
  }

  function close() {
    openId = null;
    requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(`[data-member="${restore}"]`)?.focus({ preventScroll: true }));
  }
</script>

<!-- The side pane belongs to the roster window; the phone's sheet is a dialog over the page. -->
{#snippet sheet(shown: MemberRow)}
  <MemberSheet
    {wide}
    {phone}
    member={paneMember ?? shown}
    members={members.data ?? []}
    runs={paneRuns}
    personas={personas.data ?? []}
    {roster}
    timeZone={week.data?.timezone ?? 'Asia/Kuala_Lumpur'}
    {toaster}
    onchange={replace}
    onclose={close}
    leaving={pane.leaving}
    onleft={(event) => pane.done(event)}
  />
{/snippet}
<PageLine title={members.data ? 'Members' : ''}>
  <h1>{#if members.data}<span class="pageline__num">{bossers}</span> bosser{bossers === 1 ? '' : 's'}{:else}Members{/if}</h1>
  <p class="pageline__context">synced from the bossing role</p>
</PageLine>
<section data-fid="window" class="card members-window window-fill" aria-labelledby="members-roster-title">
  <div class="card__head members-window__head" data-fid="window-bar">
    <h2 class="card__title" id="members-roster-title">Roster</h2>
    <div class="members-window__search" data-fid="window-search" role="search">
      <label class="vh" for="members-search">Search members</label>
      <input id="members-search" type="search" bind:value={query} placeholder="name, nickname, alias…" autocomplete="off" spellcheck="false" />
    </div>
    <div class="members-window__sort" data-fid="members-sort">
      <Select
        size="tbar"
        label="Sort"
        fullLabel="Sort members"
        plain
        options={[
          { value: 'runs', label: 'runs' },
          { value: 'name', label: 'A–Z' },
        ]}
        bind:value={() => order, (v) => (order = v as MemberOrder)}
      />
    </div>
  </div>
  <div class="members-window__body">
    <div class="members-roster" data-fid="members-list">
      {#if members.error}
        <LoadError thing="members" reason={members.error} onretry={() => void members.load()} />
      {:else if !members.data}
        <LoadingState text="Loading the roster…" />
      {:else if rows.length === 0}
        <div class="empty">
          <strong>Nothing matches “{query}”.</strong>The search reads the Discord name, the server nickname and the chat aliases.
        </div>
      {:else}
        <div class="memberlist__columns" data-fid="members-head" aria-hidden="true"><span>Member</span><span>This wk</span><span>@mentions</span><span>Reply style</span></div>
        <div class="memberlist" role="list" aria-label="Members">
          {#each shown.rows as member (member.id)}
            <div role="listitem">
              <button
                class="memberlist__row expandable-row"
                data-fid="members-row"
                class:memberlist__row--active={member.id === openId}
                type="button"
                aria-current={member.id === openId ? 'true' : undefined}
                onclick={() => {
                  restore = member.id;
                  openId = member.id;
                }}
                data-member={member.id}
              >
                <span class="memberlist__lead">
                <Avatar class="memberlist__av" src={memberAvatar(member.id)} name={member.name} />
                <RowContent expanded={member.id === openId}>
                  {#snippet compact()}<span class="memberlist__name">
                  <span class="memberlist__who"><strong>{memberLabel(members.data ?? [], member.id)}</strong>
                  {#if member.nickname}<span class="id">{member.nickname}</span>{:else if member.aliases.length}<span class="id">{member.aliases.join(' · ')}</span>{/if}</span>
                  {#if !member.bossing}<span class="chip chip--waiting">chat only</span>{/if}
                  </span>{/snippet}
                  <span class="memberlist__identity"><strong>{memberLabel(members.data ?? [], member.id)}</strong>{#if member.name !== memberLabel(members.data ?? [], member.id)}<span class="id">{member.name}</span>{/if}{#if member.nickname && member.nickname !== memberLabel(members.data ?? [], member.id)}<span class="id">{member.nickname}</span>{/if}{#if member.aliases.length}<span class="id">{member.aliases.join(' · ')}</span>{/if}{#if !member.bossing}<span class="chip chip--waiting">chat only</span>{/if}</span>
                </RowContent>
                </span>
                <span class="memberlist__stat mono"><span class="vh">, runs this week: </span>{member.runs_this_week}</span>
                <span class="memberlist__preference"><span class="vh">, @mentions: </span>{PING[member.ping_level]}</span>
                <span class="memberlist__style mono"><span class="vh">, reply style: </span>{member.persona ?? 'default'}</span>
                {#if member.id === openId}<span class="vh">, open</span>{/if}
              </button>
            </div>
          {/each}
        </div>
        <div class="members-roster__pager"><Pager bind:page pages={shown.pages} total={rows.length} noun="member" /></div>
      {/if}
    </div>
    {#if pane.shown && wide}{@render sheet(pane.shown)}{/if}
  </div>
</section>
{#if pane.shown && !wide}{@render sheet(pane.shown)}{/if}
{@render dialogs.content?.()}
