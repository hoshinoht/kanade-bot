<script lang="ts">
  import type { Summary } from '@kanade/api-types';

  let {
    summary,
    onopen,
    inline = false,
  }: { summary: Summary; onopen: (runId: string) => void; /** Only the summary line, as one item in the week header. */ inline?: boolean } =
    $props();
</script>

{#if !inline}
<!-- v4 partials/now.html: what is next, who still owes an answer, what waits
     for approval, whether the model is free. -->
<div class="now" role="group" aria-label="Right now">
  {#if summary.next}
    <button type="button" class="now__tile now__tile--next" onclick={() => onopen(summary.next!.run_id)}>
      <span class="now__label">Next</span>
      <span class="now__big mono">{summary.next.countdown}</span>
      <span class="now__sub"
        >{summary.next.bosses} · {summary.next.when} · <span class="mono">{summary.next.on}/{summary.next.total} on</span></span
      >
    </button>
  {:else}
    <div class="now__tile">
      <span class="now__label">Next</span>
      <span class="now__big">nothing ahead</span>
      <span class="now__sub">Every run this week has been and gone.</span>
    </div>
  {/if}
  <div class="now__tile" class:now__tile--warn={summary.unanswered > 0}>
    <span class="now__label">Unanswered</span>
    <span class="now__big mono">{summary.unanswered}</span>
    <span class="now__sub"
      >{summary.unanswered
        ? `answer${summary.unanswered === 1 ? '' : 's'} still owed on the runs ahead`
        : 'everybody has said, on every run ahead'}</span
    >
  </div>
  <a class="now__tile" class:now__tile--warn={summary.inbox > 0} href="/inbox">
    <span class="now__label">Inbox</span>
    <span class="now__big mono">{summary.inbox}</span>
    <span class="now__sub">{summary.inbox ? 'waiting on you' : 'nothing proposed'}</span>
  </a>
  <a class="now__tile" class:now__tile--warn={summary.model.busy} href="/limits">
    <span class="now__label">Model</span>
    <span class="now__big">{summary.model.busy ? 'busy' : 'free'}</span>
    <span class="now__sub">{summary.model.holder ?? 'nothing is holding it'}</span>
  </a>
</div>
{/if}

<!-- Short frames and phones: the same four facts as one summary line
  (docs/v5/pwa-design-guidelines.md, "Area follows importance"). Exactly one
  of the two is displayed. -->
<p class="now-line" class:now-line--inline={inline} aria-label="Right now, in brief">
  {#if summary.next}
    <button type="button" class="now-line__item now-line__next" onclick={() => onopen(summary.next!.run_id)}
      >Next <span class="mono">{summary.next.countdown}</span> · {summary.next.bosses}</button
    >
  {:else}<span class="now-line__item">Nothing ahead</span>{/if}
  <span class="now-line__item" class:now-line__item--warn={summary.unanswered > 0}
    ><span class="mono">{summary.unanswered}</span> unanswered</span
  >
  <a class="now-line__item" class:now-line__item--warn={summary.inbox > 0} href="/inbox">Inbox <span class="mono">{summary.inbox}</span></a>
  <a class="now-line__item" class:now-line__item--warn={summary.model.busy} href="/limits">Model {summary.model.busy ? 'busy' : 'free'}</a>
</p>
