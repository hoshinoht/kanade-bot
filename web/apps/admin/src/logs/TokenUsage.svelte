<script lang="ts">
  let {
    prompt,
    completion,
    cached,
    reasoning,
  }: { prompt?: number | null; completion?: number | null; cached?: number | null; reasoning?: number | null } = $props();
  const count = (n: number | null | undefined) => (n == null ? '—' : n.toLocaleString('en'));
</script>

<span title="Prompt → completion tokens">{count(prompt)}{#if cached != null}<span title="Prompt tokens served from the provider's prompt cache">{` (${count(cached)} cached)`}</span>{/if} → {count(completion)}</span>{#if reasoning != null}<span title="Reasoning tokens">{` · ${count(reasoning)} reasoning`}</span>{/if}
