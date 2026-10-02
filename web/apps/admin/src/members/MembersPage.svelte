<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/members.scss';
  import type { MemberRow, Persona } from '@kanade/api-types';
  import Pager from '../pages/Pager.svelte';
  import { paged } from '../pages/paging';
  import { memberLabel } from '../names/directory.svelte';
  import { Resource } from '../resource.svelte';
  import MemberSheet from './MemberSheet.svelte';

  const members = new Resource<MemberRow[]>('/api/admin/members');
  const personas = new Resource<Persona[]>('/api/admin/personas');
  $effect(() => {
    void members.load();
    void personas.load();
  });

  let query = $state('');
  let openId = $state<string | null>(null);
  let wide = $state(false);
  let ascending = $state(true);
  let restore = '';

  const q = $derived(query.trim().toLowerCase());
  const rows = $derived(
    (members.data ?? [])
      .filter((m) => !q || [m.name, m.nickname ?? '', ...m.aliases].some((t) => t.toLowerCase().includes(q)))
      .toSorted((a, b) => ascending ? memberLabel(members.data ?? [], a.id).localeCompare(memberLabel(members.data ?? [], b.id)) : memberLabel(members.data ?? [], b.id).localeCompare(memberLabel(members.data ?? [], a.id))),
  );
  let page = $state(1);
  // A new search starts at page one (v4 dropped `page` from the search form).
  $effect(() => {
    void q;
    page = 1;
  });
  const shown = $derived(paged(rows, page));
  const bossers = $derived((members.data ?? []).filter((m) => m.bossing).length);
  const current = $derived(members.data?.find((m) => m.id === openId) ?? null);

  $effect(() => {
    const query = window.matchMedia('(min-width: 840px)');
    const update = () => (wide = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });

  function replace(row: MemberRow) {
    if (members.data) members.data = members.data.map((m) => (m.id === row.id ? row : m));
  }

  function close() {
    openId = null;
    requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(`[data-member="${restore}"]`)?.focus());
  }
</script>

<PageLine title={members.data ? 'Members' : ''}>
  <h1>{#if members.data}<span class="pageline__num">{bossers}</span> bosser{bossers === 1 ? '' : 's'}{:else}Members{/if}</h1>
  <p class="pageline__context">synced from the bossing role</p>
</PageLine>
<section class="card members-window window-fill" aria-labelledby="members-roster-title">
  <div class="card__head members-window__head">
    <h2 class="card__title" id="members-roster-title">Roster</h2>
    <div class="members-window__search" role="search">
      <label class="vh" for="members-search">Search members</label>
      <input id="members-search" type="search" bind:value={query} placeholder="name, nickname, alias…" autocomplete="off" spellcheck="false" />
    </div>
    <button class="btn members-window__sort" type="button" aria-pressed={!ascending} onclick={() => (ascending = !ascending)}>
      Sort {ascending ? 'A–Z' : 'Z–A'}
    </button>
  </div>
  <div class="members-window__body">
    <div class="members-roster">
      {#if members.error}
        <p class="flash flash--error" role="alert">{members.error}</p>
      {:else if members.data && rows.length === 0}
        <div class="empty">
          <strong>Nothing matches “{query}”.</strong>The search reads the Discord name, the server nickname and the chat aliases.
        </div>
      {:else}
        <div class="memberlist" role="list" aria-label="Members">
          <div class="memberlist__columns" role="presentation" aria-hidden="true"><span>Member</span><span>This wk</span><span>@mentions</span><span>Reply style</span></div>
          {#each shown.rows as member (member.id)}
            <div role="listitem">
              <button
                class="memberlist__row"
                class:memberlist__row--active={member.id === openId}
                type="button"
                aria-current={member.id === openId ? 'true' : undefined}
                onclick={() => {
                  restore = member.id;
                  openId = member.id;
                }}
                data-member={member.id}
              >
                <span class="memberlist__name">
                  <strong>{memberLabel(members.data ?? [], member.id)}</strong>
                  {#if member.nickname}<span class="id">{member.nickname}</span>{:else if member.aliases.length}<span class="id">{member.aliases.join(', ')}</span>{/if}
                  {#if !member.bossing}<span class="chip chip--waiting">chat only</span>{/if}
                </span>
                <span class="memberlist__stat mono">{member.runs_this_week} run{member.runs_this_week === 1 ? '' : 's'}</span>
                <span class="memberlist__preference">{member.ping_level}</span>
                <span class="memberlist__style mono">{member.persona ?? 'default'}</span>
                {#if member.id === openId}<span class="memberlist__open cap">open</span>{/if}
              </button>
            </div>
          {/each}
        </div>
        <div class="members-roster__pager"><Pager bind:page pages={shown.pages} total={rows.length} noun="member" /></div>
      {/if}
    </div>
    {#if current}
      <MemberSheet wide={wide} member={current} personas={personas.data ?? []} onchange={replace} onclose={close} />
    {/if}
  </div>
</section>
