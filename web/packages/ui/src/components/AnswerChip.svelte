<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Participant } from '@kanade/api-types';
  import { ANSWER_MARKS } from '../format';

  let {
    participant,
    label,
    children,
  }: {
    participant: Participant;
    /** The name to show when it differs from `participant.name` (a twin's "Ren (2)"); never the id. */
    label?: string;
    children?: Snippet;
  } = $props();
  const answer = $derived(ANSWER_MARKS[participant.answer]);
  const name = $derived(label ?? participant.name);
</script>

<span class="chip chip--{participant.answer}" title="{name}: {answer.word}">
  <span class="chip__mark" aria-hidden="true">{answer.mark}</span>
  {name}
  <span class="vh">({answer.word})</span>
  {@render children?.()}
</span>
