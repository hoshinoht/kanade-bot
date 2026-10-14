<script lang="ts">
  import type { Snippet } from 'svelte';
  import NapArt from './NapArt.svelte';

  let { title, children, actions }: { title: string; children: Snippet; actions?: Snippet } = $props();
  const uid = $props.id();
</script>

<!-- Loading failed or offline: the same window chrome, the nap, and a way out. -->
<section class="card window-fill napwin" aria-labelledby={uid}>
  <div class="card__head"><h2 class="card__title" id={uid}>{title}</h2></div>
  <div class="napwin__body">
    <NapArt />
    <div class="napwin__text">{@render children()}</div>
    {#if actions}<div class="napwin__actions">{@render actions()}</div>{/if}
  </div>
</section>

<style>
  .napwin {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .napwin__body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    justify-items: center;
    align-content: start;
    gap: 0.8rem;
    text-align: center;
  }

  .napwin__text {
    max-width: 46ch;
    color: var(--dim);
  }

  .napwin__text :global(p) {
    margin: 0 0 0.4rem;
  }

  .napwin__actions {
    display: flex;
    gap: 0.4rem;
  }
</style>
