<!-- v4's one-button forms: the label says what pressing it does, not the state. -->
<script lang="ts">
  let {
    on,
    onLabel,
    offLabel,
    apply,
    disabled = false,
  }: { on: boolean; onLabel: string; offLabel: string; apply: (next: boolean) => Promise<string>; disabled?: boolean } = $props();

  let busy = $state(false);
  let error = $state('');

  // Busy is aria-disabled, not disabled, so focus stays on the button.
  async function press() {
    if (busy) return;
    busy = true;
    error = await apply(!on);
    busy = false;
  }
</script>

<button class="btn" type="button" {disabled} aria-disabled={busy} onclick={() => void press()}>{on ? onLabel : offLabel}</button>
{#if error}<p class="field__error" role="alert">{error}</p>{/if}
