<!--
  `/extractions/:id` (old links, the Inbox's "Extraction log entry"): the
  call now opens in the Extractions list-detail, so this route hands over to
  `/extractions?call=<id>` without leaving a history entry.
-->
<script lang="ts">
  let { id }: { id: string } = $props();

  $effect(() => {
    history.replaceState(null, '', `/extractions?call=${encodeURIComponent(id)}`);
    // The router follows location on popstate.
    window.dispatchEvent(new PopStateEvent('popstate'));
  });
</script>
