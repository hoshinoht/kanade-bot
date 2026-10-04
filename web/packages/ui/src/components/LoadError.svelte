<!--
  A pane whose read failed (M3E "Shared states", B_States "Error and retry"):
  centred in that pane, the reason in one line, Try again and Copy details.
  Filters and drafts live outside it, so a retry keeps them.
-->
<script lang="ts">
  import StateNote from './StateNote.svelte';

  let { thing, reason, onretry, level = 2 }: { thing: string; reason: string; onretry: () => void; level?: 2 | 3 } = $props();

  let copied = $state('');
  async function copy() {
    try {
      await navigator.clipboard.writeText(`Couldn't load ${thing}: ${reason}`);
      copied = 'Copied';
    } catch {
      copied = "Couldn't copy";
    }
  }
</script>

<div class="state-pane" data-fid="state-pane" role="alert">
  <StateNote tone="error" icon="alert-circle" title="Couldn’t load {thing}" {level}>
    {reason}
    {#snippet actions()}
      <button type="button" class="btn btn--primary" onclick={onretry}>Try again</button>
      <button type="button" class="btn" onclick={() => void copy()}>{copied || 'Copy details'}</button>
    {/snippet}
  </StateNote>
</div>
