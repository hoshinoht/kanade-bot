<script lang="ts">
  import '@kanade/ui/styles/panes.scss';
  import type { Snippet } from 'svelte';

  let {
    title,
    query = $bindable(''),
    searchLabel = '',
    placeholder = '',
    children,
  }: { title: string; query?: string; searchLabel?: string; placeholder?: string; children: Snippet } = $props();
  const uid = $props.id();
</script>

<!-- v4 "card pane": a window whose body scrolls, with its search on the title bar. -->
<section class="card pane window-fill" aria-labelledby="{uid}-title">
  <div class="card__head">
    <h2 class="card__title" id="{uid}-title">{title}</h2>
    {#if searchLabel}
      <div class="pane__search" role="search">
        <label class="vh" for="{uid}-q">{searchLabel}</label>
        <input id="{uid}-q" type="search" bind:value={query} {placeholder} autocomplete="off" spellcheck="false" />
      </div>
    {/if}
  </div>
  <div class="pane__body">{@render children()}</div>
</section>
