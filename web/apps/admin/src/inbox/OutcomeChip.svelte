<!-- How a closed item ended: its word and mark on the tonal chip. -->
<script lang="ts">
  import type { PastOutcome } from '@kanade/api-types';
  import { Icon, StatusChip } from '@kanade/ui';
  import { OUTCOME } from './past';

  let { outcome }: { outcome: PastOutcome } = $props();
  const o = $derived(OUTCOME[outcome]);
</script>

<span class="past__outcome-chip" data-outcome={outcome}><StatusChip tone={o.tone}><Icon name={o.icon} />{o.label}</StatusChip></span>
