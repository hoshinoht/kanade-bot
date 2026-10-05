<script lang="ts">
  import type { Toaster } from './toaster.svelte';
  import ToastItem from './ToastItem.svelte';

  let { toaster }: { toaster: Toaster } = $props();
  const isLeaving = (id: number) => toaster.leaving.some((t) => t.id === id);
</script>

<!-- Present from first paint so screen readers register the live region before it changes. -->
<section class="toasts" aria-label="Notifications" aria-live="polite" aria-relevant="additions text">
  <!-- Newest on top, in reading order too. -->
  {#each toaster.shown.slice().reverse() as toast (toast.id)}
    <ToastItem {toast} {toaster} leaving={isLeaving(toast.id)} />
  {/each}
</section>
