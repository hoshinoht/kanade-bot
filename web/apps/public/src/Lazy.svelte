<!--
  Screen-level code splitting (as the admin's `pages/Lazy.svelte`): a screen's
  JS and CSS load on its first visit. `children` renders the loaded component,
  so each screen's props stay type-checked where App passes them. A failed load
  is the portal's own notice window (a direct child of `.shell`).
-->
<script lang="ts" generics="C">
  import { StateNote } from '@kanade/ui';
  import type { Snippet } from 'svelte';

  let { loader, children }: { loader: () => Promise<{ default: C }>; children: Snippet<[C]> } = $props();

  const promise = $derived(loader());
</script>

{#await promise then screen}
  {@render children(screen.default)}
{:catch}
  <div class="gate">
    <section class="card notice" role="alert" aria-labelledby="lazy-bar">
      <div class="card__head"><span class="card__title" id="lazy-bar">Not loaded</span></div>
      <StateNote tone="error" icon="alert-circle" level={1} title="This page didn't load">
        Kanade has probably been updated since this tab opened.
        {#snippet actions()}
          <button type="button" class="btn btn--primary btn--key" onclick={() => location.reload()}>Reload</button>
        {/snippet}
      </StateNote>
    </section>
  </div>
{/await}
