<script lang="ts">
  import { clampPage, rangeLabel } from './paging';

  let {
    page = $bindable(1),
    pages,
    total,
    noun = 'row',
    size = 0,
    back = '← Newer',
    forward = 'Older →',
  }: {
    page: number;
    pages: number;
    total: number;
    noun?: string;
    /** Rows per page: when set, the count reads "1–15 of 33". */
    size?: number;
    back?: string;
    forward?: string;
  } = $props();
  const uid = $props.id();
  const at = $derived(clampPage(page, pages));
  const range = $derived(size ? rangeLabel(at, size, total) : '');
</script>

<!-- v4 macros.pager: newer/older on the outside edges, "page N of M" between. -->
{#if pages > 1}
  <nav class="pager" aria-label="Pages" aria-describedby="{uid}-at">
    <button class="btn" type="button" disabled={at <= 1} onclick={() => (page = at - 1)}>{back}</button>
    <span class="pager__at mono" id="{uid}-at" aria-live="polite">
      {#if range}{range} {noun}s{:else}Page {at} of {pages} · {total.toLocaleString('en')} {noun}{total === 1 ? '' : 's'}{/if}
    </span>
    <button class="btn" type="button" disabled={at >= pages} onclick={() => (page = at + 1)}>{forward}</button>
  </nav>
{/if}
