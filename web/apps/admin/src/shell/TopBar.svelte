<!--
  The phone frame's 48 px top bar (M3E spec "Phone", gate G7): the menu that
  opens the navigation drawer, where you are, the Live chip, and the Inbox
  always one tap away. No bottom navigation bar.
-->
<script lang="ts">
  import { Freshness, Icon, pulse, type FreshState } from '@kanade/ui';
  import { arrival } from '../resource.svelte';
  import type { BackStep } from './chrome';
  import QuietMode from './QuietMode.svelte';

  let {
    title,
    open,
    inbox = 0,
    onInbox = false,
    fresh,
    updated = '',
    timezone = '',
    quiet = false,
    drawerId,
    back = null,
    menu = $bindable(),
    onmenu,
  }: {
    title: string;
    /** Whether the drawer is open (the menu button's expanded state). */
    open: boolean;
    inbox?: number;
    /** The Inbox is the current page. */
    onInbox?: boolean;
    fresh: FreshState;
    updated?: string;
    timezone?: string;
    /** "Quiet mode on" in place of the Live chip. */
    quiet?: boolean;
    drawerId: string;
    /** A page's own back step (an open Inbox item), in place of the menu and title. */
    back?: BackStep | null;
    /** The bar's leading control (the menu, or a page's back step): where the drawer returns focus. */
    menu?: HTMLButtonElement;
    onmenu: () => void;
  } = $props();
</script>

<header class="topbar" data-fid="topbar">
  {#if back}
    <!-- Bound like the menu: the drawer (still opened by the edge swipe) returns focus to whichever is here. -->
    <button bind:this={menu} type="button" class="topbar__back" aria-label={back.name} onclick={() => back?.go()}><Icon name="chevron-left" />{back.label}</button>
    <span class="topbar__spacer"></span>
  {:else}
  <button
    bind:this={menu}
    type="button"
    class="topbar__menu"
    data-fid="topbar-menu"
    aria-label="Open the navigation"
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-controls={drawerId}
    onclick={onmenu}
  >
    <Icon name="menu" />
  </button>
  <p class="topbar__title" data-fid="topbar-title">{title}</p>
  {/if}
  <span class="topbar__fresh" class:topbar__fresh--quiet={quiet} data-fid="topbar-fresh" title={timezone ? `Every time here is ${timezone}` : undefined}
    >{#if quiet}<QuietMode />{:else}<Freshness state={fresh} {updated} />{/if}</span
  >
  <a class="topbar__inbox" data-fid="topbar-inbox" href="/inbox" aria-current={onInbox ? 'page' : undefined} aria-label={inbox > 0 ? `Inbox ${inbox} waiting` : 'Inbox'}>
    <Icon name="inbox" />
    <span class="vh">Inbox</span>
    {#if inbox > 0}<span class="topbar__badge" {@attach pulse(inbox, arrival.seq)}>{inbox}<span class="vh"> waiting</span></span>{/if}
  </a>
</header>
