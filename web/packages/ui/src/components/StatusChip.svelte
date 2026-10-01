<script lang="ts">
  import type { Snippet } from 'svelte';

  let { tone = 'neutral', legacyTone = '', children }: { tone?: 'neutral' | 'ok' | 'risk' | 'warn'; legacyTone?: string; children: Snippet } = $props();
</script>

<span class="status-chip status-chip--{tone} {legacyTone ? `tone tone--${legacyTone}` : ''}">{@render children()}</span>
