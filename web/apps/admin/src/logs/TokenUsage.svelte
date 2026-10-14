<script lang="ts">
  let { prompt, completion, reasoning }: { prompt?: number | null; completion?: number | null; reasoning?: number | null } = $props();
  const count = (n: number | null | undefined) => (n == null ? '—' : n.toLocaleString('en'));
</script>

<span title="Prompt → completion tokens">{count(prompt)} → {count(completion)}</span>{#if reasoning != null}<span title="Reasoning tokens">{` · ${count(reasoning)} reasoning`}</span>{/if}
