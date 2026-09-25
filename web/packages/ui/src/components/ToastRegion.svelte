<script lang="ts">
  import type { Toaster } from './toaster.svelte';
  import ToastItem from './ToastItem.svelte';

  let { toaster }: { toaster: Toaster } = $props();
</script>

<!-- Present from first paint so screen readers register the live region before it changes. -->
<section class="toasts" aria-label="Notifications" aria-live="polite" aria-relevant="additions text">
  {#each toaster.items as toast (toast.id)}
    <ToastItem {toast} {toaster} />
  {/each}
</section>
