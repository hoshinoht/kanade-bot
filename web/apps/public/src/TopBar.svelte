<!--
  The phone frame's 48 px top bar (boards PhoneGate … PhoneBrowser), the
  admin top bar's look (`_topbar.scss`). Signed out: the bot's tile and name
  and the time zone. Signed in (`member`): the menu that opens the drawer,
  the page's name and the member's portrait, which opens their Profile. The
  Week boards' freshness chip comes with the Week screen.
-->
<script lang="ts">
  import type { PublicMember } from '@kanade/api-types';
  import { Avatar, Icon, initial } from '@kanade/ui';

  let {
    name,
    avatar,
    zone,
    member = null,
    title = '',
    open = false,
    drawerId = '',
    menu = $bindable(),
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
    onmenu?: () => void;
    onprofile?: () => void;
  } = $props();
</script>

<header class="topbar" class:topbar--visitor={!member} data-fid="topbar">
  {#if member}
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
