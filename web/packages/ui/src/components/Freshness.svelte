<script lang="ts" module>
  export type FreshState = 'loading' | 'live' | 'stale' | 'offline' | 'error' | 'closed';
</script>

<script lang="ts">
  import Icon from './Icon.svelte';

  let { state, updated = '' }: { state: FreshState; updated?: string } = $props();
</script>

<span class="fresh fresh--{state}" data-fresh={state}>
  {#if state === 'loading'}
    <Icon name="refresh-cw" /> Loading…
  {:else if state === 'live'}
    <Icon name="check" /> <span><span class="fresh__state">Live</span>{#if updated}<span class="fresh__words">&nbsp;· updated</span>&nbsp;<span class="fresh__time">{updated}</span>{/if}</span>
  {:else if state === 'closed'}
    <Icon name="clock" /> Closed{#if updated}&nbsp;· checked {updated}{/if}
  {:else if state === 'stale'}
    <Icon name="clock" /> Retrying{#if updated}&nbsp;· last updated {updated}{/if}
  {:else if state === 'offline'}
    <Icon name="wifi-off" /> Offline{#if updated}&nbsp;·<span class="fresh__since"> last updated</span> {updated}{/if}
  {:else}
    <Icon name="alert-circle" /> Can't reach Kanade
  {/if}
</span>
