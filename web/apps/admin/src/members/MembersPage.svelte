<script lang="ts">
  import '@kanade/ui/styles/members.scss';
  import type { MemberRow, Persona } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import Pager from '../pages/Pager.svelte';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { paged } from '../pages/paging';
  import { Resource } from '../resource.svelte';
  import MemberSheet from './MemberSheet.svelte';

  const members = new Resource<MemberRow[]>('/api/admin/members');
  const personas = new Resource<Persona[]>('/api/admin/personas');
  $effect(() => {
    void members.load();
    void personas.load();
  });

  let query = $state('');
  let open = $state(false);
  let openId = $state<string | null>(null);

  const q = $derived(query.trim().toLowerCase());
  const rows = $derived(
    (members.data ?? []).filter((m) => !q || [m.name, m.nickname ?? '', ...m.aliases].some((t) => t.toLowerCase().includes(q))),
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
  const duplicate = (m: MemberRow) => (members.data ?? []).filter((o) => o.name === m.name).length > 1;

  function replace(row: MemberRow) {
    if (members.data) members.data = members.data.map((m) => (m.id === row.id ? row : m));
  }
</script>

<div class="page-head">
  <div>
    <p class="eyebrow">Synced from the bossing role</p>
    <h1>{members.data ? `${bossers} bosser${bossers === 1 ? '' : 's'}` : 'Members'}</h1>
    <p class="note">Aliases are what the extractor matches names against in chat.</p>
  </div>
</div>
<PaneWindow title="Roster" bind:query searchLabel="Search members" placeholder="name, nickname, alias…">
  {#if members.error}
    <p class="flash flash--error" role="alert">{members.error}</p>
  {:else if members.data && rows.length === 0}
    <div class="empty">
      <strong>Nothing matches “{query}”.</strong>The search reads the Discord name, the server nickname and the chat aliases.
    </div>
  {:else}
    <ul class="memberlist" aria-label="Members">
      {#each shown.rows as member (member.id)}
        <li>
          <button
            class="memberlist__row"
            type="button"
            onclick={() => {
              openId = member.id;
              open = true;
            }}
          >
            <span class="memberlist__name">
              <strong>{member.name}</strong>
              {#if duplicate(member)}<span class="id">#{member.id}</span>{/if}
              {#if member.nickname}<span class="id">{member.nickname}</span>{/if}
              {#if !member.bossing}<span class="chip chip--waiting">chat only</span>{/if}
            </span>
            <span class="memberlist__stat mono">{member.runs_this_week} run{member.runs_this_week === 1 ? '' : 's'}</span>
            {#if member.ping_level !== 'essential'}<span class="chip chip--mono">pings: {member.ping_level}</span>{/if}
            {#if member.persona}<span class="chip chip--mono">{member.persona}</span>{/if}
            <span class="memberlist__chevron" aria-hidden="true"><Icon name="chevron-right" /></span>
          </button>
        </li>
      {/each}
    </ul>
    <Pager bind:page pages={shown.pages} total={rows.length} noun="member" />
  {/if}
</PaneWindow>
<MemberSheet bind:open member={current} personas={personas.data ?? []} onchange={replace} />

<style>
  .memberlist {
    list-style: none;
    margin: 0;
    padding: 0;
  }
</style>
