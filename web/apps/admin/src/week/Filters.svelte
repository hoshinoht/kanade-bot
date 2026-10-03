<!--
  v4's filter bar as the Week window's "Filters (n)" title-bar button (M3E
  spec, Week): the fields open in a popover under it, filtering as you
  choose; the active filters show as removable chips in the window's footer.
  On phones the button is an icon with the same name.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { Channel, Member } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import { filtering, NO_FILTER, type WeekFilter } from './filters';
  import { directory, memberLabel } from '../names/directory.svelte';

  let {
    filter = $bindable(),
    channels,
    members,
    count,
    icon = false,
  }: { filter: WeekFilter; channels: Channel[]; members: Member[]; count: number; icon?: boolean } = $props();
  const uid = $props.id();
  let open = $state(false);
  let button = $state<HTMLButtonElement>();
  let panel = $state<HTMLDivElement>();

  async function toggle() {
    open = !open;
    if (!open) return;
    await tick();
    panel?.querySelector<HTMLElement>('select, input')?.focus({ preventScroll: true });
  }

  function close(refocus: boolean) {
    open = false;
    if (refocus) button?.focus({ preventScroll: true });
  }

  // A press outside the button and the panel folds it away.
  $effect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!panel?.contains(target) && !button?.contains(target)) close(false);
    };
    document.addEventListener('pointerdown', away, true);
    return () => document.removeEventListener('pointerdown', away, true);
  });
</script>

<div class="week-filters">
  <button
    type="button"
    class="btn week-filters__toggle"
    class:week-filters__toggle--icon={icon}
    aria-expanded={open}
    aria-controls="{uid}-panel"
    aria-label={icon ? `Filters (${count})` : undefined}
    bind:this={button}
    onclick={() => void toggle()}><Icon name="filter" />{#if !icon}<span>Filters ({count})</span>{/if}</button
  >
  {#if open}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="filters filters--panel week-filters__panel"
      role="search"
      aria-label="Filter the week"
      id="{uid}-panel"
      bind:this={panel}
      onkeydown={(event) => {
        if (event.key === 'Escape') {
          event.stopPropagation();
          close(true);
        }
      }}
    >
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
  {/if}
</div>
