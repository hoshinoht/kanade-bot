<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  let {
    open = $bindable(false),
    title,
    eyebrow = '',
    narrow = false,
    wide = false,
    flush = false,
    children,
    footer,
    onclose,
  }: {
    open: boolean;
    title: string;
    eyebrow?: string;
    narrow?: boolean;
    /** v4 run-sheet width. */
    wide?: boolean;
    /** Body without padding, for content that is its own card (the run sheet). */
    flush?: boolean;
    children: Snippet;
    footer?: Snippet<[() => void]>;
    onclose?: () => void;
  } = $props();

  const uid = $props.id();
  let dialog: HTMLDialogElement;
  let returnTo: HTMLElement | null = null;

  $effect(() => {
    if (open && !dialog.open) {
      returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  function close() {
    open = false;
  }

  function handleClose() {
    open = false;
    onclose?.();
    // Native restoration is not guaranteed everywhere; put focus back where the reader was.
    if (returnTo?.isConnected) returnTo.focus();
    returnTo = null;
  }
</script>

<dialog
  bind:this={dialog}
  class="modal"
  class:modal--narrow={narrow}
  class:modal--wide={wide}
  aria-labelledby="{uid}-title"
  onclose={handleClose}
>
  <div class="modal__panel">
    <header class="modal__head">
      <div>
        {#if eyebrow}<p class="eyebrow">{eyebrow}</p>{/if}
        <h2 class="modal__title" id="{uid}-title">{title}</h2>
      </div>
      <button type="button" class="modal__x" onclick={close}><Icon name="x" label="Close" /></button>
    </header>
    <div class="modal__body" class:modal__body--flush={flush}>
      {@render children()}
    </div>
    {#if footer}
      <footer class="modal__foot">{@render footer(close)}</footer>
    {/if}
  </div>
</dialog>
