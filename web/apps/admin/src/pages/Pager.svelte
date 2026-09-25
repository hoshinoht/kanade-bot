<script lang="ts">
  let {
    page = $bindable(1),
    pages,
    total,
    noun = 'row',
  }: { page: number; pages: number; total: number; noun?: string } = $props();
  const uid = $props.id();
</script>

<!-- v4 macros.pager: newer/older on the outside edges, "page N of M" between. -->
{#if pages > 1}
  <nav class="pager" aria-label="Pages" aria-describedby="{uid}-at">
    <button class="btn" type="button" disabled={page <= 1} onclick={() => (page -= 1)}>← Newer</button>
    <span class="pager__at mono" id="{uid}-at" aria-live="polite">
      Page {page} of {pages} · {total.toLocaleString('en')} {noun}{total === 1 ? '' : 's'}
    </span>
    <button class="btn" type="button" disabled={page >= pages} onclick={() => (page += 1)}>Older →</button>
  </nav>
{/if}
