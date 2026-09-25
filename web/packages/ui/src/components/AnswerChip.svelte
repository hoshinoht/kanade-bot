<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Participant } from '@kanade/api-types';
  import { ANSWER_MARKS } from '../format';

  let {
    participant,
    disambiguate = false,
    children,
  }: { participant: Participant; disambiguate?: boolean; children?: Snippet } = $props();
  const answer = $derived(ANSWER_MARKS[participant.answer]);
</script>

<!-- Two members can share a display name; then the member id tells them apart (HPK-9). -->
<span class="chip chip--{participant.answer}" title="{participant.name}: {answer.word}">
  <span class="chip__mark" aria-hidden="true">{answer.mark}</span>
  {participant.name}{#if disambiguate}<span class="chip__id">#{participant.id}</span>{/if}
  <span class="vh">({answer.word})</span>
  {@render children?.()}
</span>
