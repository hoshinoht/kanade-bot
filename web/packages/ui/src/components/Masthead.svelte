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
    <svelte:element this={href ? 'a' : 'p'} class="brand" {href}>
      {#if avatar}
        <img class="brand__avatar" src={avatar} alt="" width="26" height="26" />
      {:else}
        <span class="brand__avatar" aria-hidden="true">{initial(name)}</span>
      {/if}
      <span class="brand__name">{name}</span>
      {#if by}<span class="brand__by">{by}</span>{/if}
    </svelte:element>
    {#if meta}<div class="masthead__meta">{@render meta()}</div>{/if}
    {@render nav?.()}
  </div>
</header>
