<script lang="ts">
  let { text, tokens, round }: { text?: string | null; tokens?: number | null; round?: number } = $props();
  const label = $derived(`Reasoning${tokens != null ? ` · ${tokens.toLocaleString('en')} tokens` : ''}${round != null ? ` · Round ${round}` : ''}`);
</script>

{#if text || tokens != null}
  <details class="reasoning">
    <summary>{label}</summary>
    {#if text}<pre>{text}</pre>{:else}<p class="note">No reasoning text reported.</p>{/if}
  </details>
{/if}

<style>
  .reasoning {
    margin-block: 0.75rem;
  }

  summary {
    min-height: 2rem;
    cursor: pointer;
    font-weight: 600;
  }
</style>
