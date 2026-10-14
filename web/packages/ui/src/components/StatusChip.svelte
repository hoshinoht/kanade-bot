<script lang="ts">
  import type { Snippet } from 'svelte';

  // `rest`: data-* attributes from the call site (fidelity tags live there, never in here).
  let {
    tone = 'neutral',
    legacyTone = '',
    children,
    ...rest
  }: { tone?: 'neutral' | 'ok' | 'risk' | 'warn'; legacyTone?: string; children: Snippet; [data: `data-${string}`]: string | undefined } = $props();
</script>

<span class="status-chip status-chip--{tone} {legacyTone ? `tone tone--${legacyTone}` : ''}" {...rest}>{@render children()}</span>
