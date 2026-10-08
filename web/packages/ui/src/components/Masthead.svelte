<!--
  The member portal's masthead (P_ boards "Shell"): the bot's identity tile
  (its avatar, or the initial) with its name over a mono by-line, then `nav`,
  then `meta` at the end (signed out: the time zone; signed in: the account
  button). 60 px; the phone frame's 52 px bar drops the by-line.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { initial } from '../initial';

  let {
    name,
    by = '',
    avatar = null,
    href = null,
    meta,
    nav,
  }: { name: string; by?: string; avatar?: string | null; href?: string | null; meta?: Snippet; nav?: Snippet } = $props();
</script>

<header class="masthead">
  <div class="masthead__inner">
    <svelte:element this={href ? 'a' : 'p'} class="masthead__id" {href}>
      {#if avatar}
        <img class="masthead__tile" src={avatar} alt="" width="32" height="32" />
      {:else}
        <span class="masthead__tile" aria-hidden="true">{initial(name)}</span>
      {/if}
      <span class="masthead__words">
        <span class="masthead__name">{name}</span>
        {#if by}<span class="masthead__by">{by}</span>{/if}
      </span>
    </svelte:element>
    {@render nav?.()}
    {#if meta}<div class="masthead__meta">{@render meta()}</div>{/if}
  </div>
</header>
