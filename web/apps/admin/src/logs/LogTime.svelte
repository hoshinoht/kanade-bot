<!-- A log row's time in the guild's zone; the tooltip carries the full timestamp. -->
<script lang="ts">
  import { logTime } from './format';

  let { at, timeZone }: { at: string; timeZone: string } = $props();
  const t = $derived(logTime(at, timeZone));
</script>

{#if t.iso}<time class="logtime" datetime={t.iso} title={t.title}>{t.text}</time>{:else}<span class="logtime">{t.text}</span>{/if}

<style>
  .logtime {
    white-space: nowrap;
  }
</style>
