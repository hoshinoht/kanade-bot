<!--
  v4's filter bar, filtering as you choose. Full: its own card under the week
  header (v4 padding and control heights). Compact (phones, short frames):
  a "Filters (n)" button in the header, active filters as removable chips,
  and the same fields in a panel only while open.
-->
<script lang="ts">
  import type { Channel, Member } from '@kanade/api-types';
  import { filtering, NO_FILTER, type WeekFilter } from './filters';
  import { directory, memberLabel } from '../names/directory.svelte';

  let {
    filter = $bindable(),
    channels,
    members,
    compact = false,
  }: { filter: WeekFilter; channels: Channel[]; members: Member[]; compact?: boolean } = $props();
  const uid = $props.id();
  let open = $state(false);

  const active = $derived(
    [
      filter.channel && { key: 'channel' as const, label: `Channel: ${directory.label('channel', filter.channel, channels.find((c) => c.id === filter.channel)?.name ?? '')}` },
      filter.member && { key: 'member' as const, label: `Member: ${memberLabel(members, filter.member)}` },
      filter.boss.trim() && { key: 'boss' as const, label: `Boss: ${filter.boss.trim()}` },
    ].filter((x): x is { key: 'channel' | 'member' | 'boss'; label: string } => Boolean(x)),
  );

  function clearOne(key: 'channel' | 'member' | 'boss') {
    filter = { ...filter, [key]: '' };
  }
</script>

{#snippet fields()}
  <div class="filters filters--panel" class:filters--popout={compact} role="search" aria-label="Filter the week" id="{uid}-panel">
    <label class="field"
      ><span>Channel</span>
      <select bind:value={filter.channel}>
        <option value="">every party</option>
        {#each channels as channel (channel.id)}<option value={channel.id}>{directory.label('channel', channel.id, channel.name)}</option>{/each}
      </select>
    </label>
    <label class="field"
      ><span>Member</span>
      <select bind:value={filter.member}>
        <option value="">everyone</option>
        {#each members as member (member.id)}<option value={member.id}>{memberLabel(members, member.id)}</option>{/each}
      </select>
    </label>
    <label class="field"><span>Boss</span><input bind:value={filter.boss} placeholder="hstar" size="10" /></label>
    {#if filtering(filter)}
      <button class="btn btn--ghost" type="button" onclick={() => (filter = { ...NO_FILTER })}>Clear</button>
    {/if}
  </div>
{/snippet}

{#if compact}
  <div class="filterchips">
    <button type="button" class="btn" aria-expanded={open} aria-controls="{uid}-panel" onclick={() => (open = !open)}>
      Filters ({active.length})
    </button>
    {#each active as chip (chip.key)}
      <button type="button" class="chip filterchips__chip" onclick={() => clearOne(chip.key)}
        >{chip.label}<span aria-hidden="true"> ×</span><span class="vh"> — remove</span></button
      >
    {/each}
    {#if open}{@render fields()}{/if}
  </div>
{:else}
  {@render fields()}
{/if}
