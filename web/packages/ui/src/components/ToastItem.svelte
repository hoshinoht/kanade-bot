<script lang="ts">
  import type { Toast, Toaster } from './toaster.svelte';
  import Icon from './Icon.svelte';

  let { toast, toaster, leaving = false }: { toast: Toast; toaster: Toaster; leaving?: boolean } = $props();

  // Plain, not $state: only the timer reads it.
  let remaining = 0;
  let paused = $state(false);

  // Timer pauses while pointer or focus is on the toast (WCAG 2.2.1 timing adjustable).
  $effect(() => {
    if (toast.timeoutMs === null || paused || leaving) return;
    remaining ||= toast.timeoutMs;
    const started = Date.now();
    const timer = setTimeout(() => toaster.dismiss(toast.id), remaining);
    return () => {
      clearTimeout(timer);
      remaining = Math.max(1000, remaining - (Date.now() - started));
    };
  });
</script>

<!-- A dismissed toast stays for its exit animation only: inert and hidden from assistive tech. -->
<div
  class="toast toast--{toast.tone}"
  class:is-leaving={leaving}
  role="group"
  aria-label="Notification"
  aria-hidden={leaving ? 'true' : undefined}
  inert={leaving}
  onanimationend={(event) => {
    if (leaving && event.target === event.currentTarget) toaster.gone(toast.id);
  }}
  onpointerenter={() => (paused = true)}
  onpointerleave={() => (paused = false)}
  onfocusin={() => (paused = true)}
  onfocusout={() => (paused = false)}
>
  <p class="toast__msg">{toast.message}</p>
  <div class="toast__actions">
    {#if toast.action}
      <button
        type="button"
        class="btn btn--primary"
        onclick={() => {
          toaster.dismiss(toast.id);
          toast.action?.run();
        }}>{toast.action.label}</button
      >
    {/if}
    <button type="button" class="btn btn--ghost" onclick={() => toaster.dismiss(toast.id)}>
      <Icon name="x" label="Dismiss" />
    </button>
  </div>
</div>
