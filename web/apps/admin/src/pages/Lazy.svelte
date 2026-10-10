<script lang="ts">
  import type { Component } from 'svelte';

  // Pages take different props; the route table pairs each loader with its props.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  type Loader = () => Promise<{ default: Component<any> }>;
  let { loader, props = {} }: { loader: Loader; props?: Record<string, unknown> } = $props();

  // One import per page; Vite dedupes chunk loads anyway.
  const promise = $derived(loader());
</script>

<!-- Route-level code splitting: each page's JS/CSS loads on first visit. -->
{#await promise then page}
  <page.default {...props} />
{:catch}
  <div class="empty" role="alert">
    <strong>This page didn't load</strong>
    Kanade Admin has probably been updated since this tab opened.
    <br /><button type="button" class="btn btn--primary" onclick={() => location.reload()}>Reload</button>
  </div>
{/await}
