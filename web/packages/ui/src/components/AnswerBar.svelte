<!--
  A run's answers as one flat segmented bar: on, maybe, then waiting (hatched),
  each as wide as its share of the party; those out leave the track bare. The
  counts are its accessible name, so the bar never rests on colour or length.
  Widths go in through CSSOM (style-src 'self' blocks style attributes).
-->
<script lang="ts">
  import type { Attachment } from 'svelte/attachments';
  import { answerCounts, answerWords, type AnswerCounts } from '../answers';

  let {
    participants,
    label = 'Answers',
    class: extra = '',
  }: { participants: readonly { answer: string }[]; label?: string; class?: string } = $props();

  const counts = $derived<AnswerCounts>(answerCounts(participants));
  const SEGMENTS = ['yes', 'maybe', 'waiting'] as const;
  const grow =
    (n: number): Attachment<HTMLElement> =>
    (node) => {
      node.style.flexGrow = String(n);
    };
</script>

{#if counts.total}
  <span class="answerbar {extra}" role="img" aria-label="{label}: {answerWords(counts)}">
    {#each SEGMENTS as key (key)}
      {#if counts[key]}<span class="answerbar__seg answerbar__seg--{key}" {@attach grow(counts[key])}></span>{/if}
    {/each}
    {#if counts.no}<span class="answerbar__rest" {@attach grow(counts.no)}></span>{/if}
  </span>
{/if}
