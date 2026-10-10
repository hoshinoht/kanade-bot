<!--
  The navigation rail (M3E spec "Navigation rail", gate G1): the bot's tile,
  the grouped destinations and the account at the bottom. Collapsed (96 px)
  below 1440 px; expanded (240 px) from there unless the reader collapsed it,
  which this browser remembers.
-->
<script lang="ts">
  import { Icon, initial } from '@kanade/ui';
  import type { Snippet } from 'svelte';
  import NavList from './NavList.svelte';
  import { railCollapsed, rememberRail } from './chrome';

  let {
    name,
    avatar = null,
    active,
    inbox = 0,
    account,
  }: { name: string; avatar?: string | null; active: string; inbox?: number; account: Snippet } = $props();

  let collapsed = $state(railCollapsed());

  function toggle() {
    collapsed = !collapsed;
    rememberRail(collapsed);
  }
</script>

<header class="navrail" class:navrail--collapsed={collapsed}>
  <div class="navrail__head">
    <a class="navrail__brand" href="/">
      {#if avatar}
        <img class="brand__avatar navrail__tile" src={avatar} alt="" width="40" height="40" />
      {:else}
        <span class="brand__avatar navrail__tile" aria-hidden="true">{initial(name)}</span>
      {/if}
      <span class="brand__name">{name}</span>
    </a>
    <button
      type="button"
      class="navrail__toggle"
      aria-label={collapsed ? 'Expand the navigation' : 'Collapse the navigation'}
      title={collapsed ? 'Expand the navigation' : 'Collapse the navigation'}
      onclick={toggle}
    >
      <Icon name={collapsed ? 'chevrons-right' : 'chevrons-left'} />
    </button>
  </div>
  <NavList {active} {inbox} />
  <div class="navrail__foot">{@render account()}</div>
</header>
