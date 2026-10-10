<!--
  Full text in a modal: pre-wrapped monospace that scrolls inside the dialog,
  with Copy. Esc, the close button or a backdrop click close it; focus returns
  to the opener (the shared Modal). Without a clipboard the text is shown
  selected in a textarea, ready for Ctrl/Cmd-C.
-->
<script lang="ts">
  import { Modal, type Toaster } from '@kanade/ui';
  import { tick } from 'svelte';
  import { copyText } from './copy';

  let {
    open = $bindable(false),
    title,
    eyebrow = '',
    text,
    toaster,
    copied = 'Copied.',
  }: { open: boolean; title: string; eyebrow?: string; text: string; toaster?: Toaster; copied?: string } = $props();

  let manual = $state(false);
  let area = $state<HTMLTextAreaElement>();
  $effect(() => {
    if (!open) manual = false;
  });

  async function copy() {
    if (await copyText(text)) {
      toaster?.show({ message: copied, tone: 'ok' });
      return;
    }
    manual = true;
    await tick();
    area?.focus();
    area?.select();
  }
</script>

<Modal bind:open {title} {eyebrow} wide lightDismiss>
  {#if manual}
    <p class="note">Copying isn't available here; the text is selected — press Ctrl/Cmd-C.</p>
    <textarea class="textmodal__area" readonly rows="16" bind:this={area} aria-label="{title} (selected for copying)">{text}</textarea>
  {:else}
    <!-- A scroll area must take keyboard focus so arrows can read long text (axe scrollable-region-focusable). -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <pre class="textmodal__pre" tabindex="0" aria-label={title}>{text}</pre>
  {/if}
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Close</button>
    <button class="btn btn--primary" type="button" onclick={() => void copy()}>Copy</button>
  {/snippet}
</Modal>

<style>
  .textmodal__pre,
  .textmodal__area {
    margin: 0;
    width: 100%;
    max-height: 60dvh;
    overflow: auto;
    padding: 0.6rem 0.75rem;
    border: 2px solid var(--line-soft);
    border-radius: var(--r-sm);
    background: var(--raise);
    font-family: var(--mono);
    font-size: var(--fs-small);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
