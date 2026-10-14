<!--
  The phone frame's 48 px top bar (boards PhoneGate … PhoneMyRuns), the
  admin top bar's look (`_topbar.scss`). Signed out: the bot's tile and name
  and the time zone. Signed in (`member`): the menu that opens the drawer,
  the page's name, the freshness chip and the member's portrait, which opens
  their Account. An open run (PhoneRun) swaps the menu and title for a
  "‹ Week" back step and a chip saying whose run it is; the request form
  (PhoneRequest) a back step to My requests and its own title.
-->
<script lang="ts">
  import type { PublicMember } from '@kanade/api-types';
  import { Avatar, Icon, initial } from '@kanade/ui';
  import type { Snippet } from 'svelte';

  let {
    name,
    avatar,
    zone,
    member = null,
    title = '',
    open = false,
    drawerId = '',
    menu = $bindable(),
    back = null,
    fresh,
    onmenu,
    onprofile,
  }: {
    /** The bot's name and avatar. */
    name: string;
    avatar: string | null;
    zone: string;
    member?: PublicMember | null;
    title?: string;
    /** Whether the drawer is open (the menu button's expanded state). */
    open?: boolean;
    drawerId?: string;
    /** Where the drawer returns focus. */
    menu?: HTMLButtonElement;
    /** In place of the menu and title: where back goes, its accessible name, what the chip says (no chip: an open guide) or the page's title (the request form). */
    back?: { label: string; name?: string; chip?: string; mine?: boolean; title?: string; onback: () => void } | null;
    fresh?: Snippet;
    onmenu?: () => void;
    onprofile?: () => void;
  } = $props();
</script>

<header class="topbar" class:topbar--visitor={!member} data-fid="topbar">
  {#if member && back}
    <button type="button" class="btn topbar__back" aria-label={back.name} onclick={back.onback}><Icon name="chevron-left" />{back.label}</button>
    {#if back.title}<p class="topbar__title" data-fid="topbar-title">{back.title}</p>{:else}<span class="topbar__spacer"></span>{/if}
    {#if back.chip}<span class={back.mine ? 'you-chip' : 'member-card__view'}>{back.chip}</span>{/if}
    <button type="button" class="topbar__me" aria-label="Account: {member.display}" onclick={onprofile}
      ><Avatar class="topbar__avatar" src={member.avatar} name={member.display} /></button
    >
  {:else if member}
    <button
      bind:this={menu}
      type="button"
      class="topbar__menu"
      aria-label="Open the navigation"
      aria-haspopup="dialog"
      aria-expanded={open}
      aria-controls={drawerId}
      onclick={onmenu}
      data-fid="topbar-menu"><Icon name="menu" /></button
    >
    <p class="topbar__title" data-fid="topbar-title">{title}</p>
    {#if fresh}<span class="topbar__fresh" data-fid="topbar-fresh">{@render fresh()}</span>{/if}
    <button type="button" class="topbar__me" aria-label="Account: {member.display}" onclick={onprofile}
      ><Avatar class="topbar__avatar" src={member.avatar} name={member.display} /></button
    >
  {:else}
    {#if avatar}
      <img class="masthead__tile" src={avatar} alt="" width="32" height="32" />
    {:else}
      <span class="masthead__tile" aria-hidden="true">{initial(name)}</span>
    {/if}
    <p class="topbar__title" data-fid="topbar-title">{name}</p>
    <span class="masthead__zone"><span class="vh">Times in </span>{zone}</span>
  {/if}
</header>
