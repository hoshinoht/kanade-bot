<script lang="ts">
  import type { Toast, Toaster } from './toaster.svelte';
  import Icon from './Icon.svelte';

  let { toast, toaster }: { toast: Toast; toaster: Toaster } = $props();

  // Plain, not $state: only the timer reads it.
  let remaining = 0;
  let paused = $state(false);

  // Timer pauses while pointer or focus is on the toast (WCAG 2.2.1 timing adjustable).
  $effect(() => {
    if (toast.timeoutMs === null || paused) return;
    remaining ||= toast.timeoutMs;
    const started = Date.now();
    const timer = setTimeout(() => toaster.dismiss(toast.id), remaining);
    return () => {
      clearTimeout(timer);
      remaining = Math.max(1000, remaining - (Date.now() - started));
    };
  });
</script>

<div
  class="toast toast--{toast.tone}"
  role="group"
  aria-label="Notification"
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
