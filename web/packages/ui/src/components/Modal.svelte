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
    lightDismiss = false,
    className = '',
    dismissible = true,
    children,
    footer,
    onclose,
    returnFocus,
  }: {
    open: boolean;
    title: string;
    eyebrow?: string;
    narrow?: boolean;
    /** v4 run-sheet width. */
    wide?: boolean;
    /** Body without padding, for content that is its own card (the run sheet). */
    flush?: boolean;
    /** Keep user-initiated dismissal locked while a mutation needs its recovery context. */
    dismissible?: boolean;
    children: Snippet;
    footer?: Snippet<[() => void]>;
    onclose?: () => void;
    /** Optional caller-owned destination when the opener will be removed. */
    returnFocus?: () => HTMLElement | null;
    /** A click on the backdrop closes it (read-only viewers; never forms that hold input). */
    lightDismiss?: boolean;
    /** Optional page-specific dialog class; keeps shared dialog semantics intact. */
    className?: string;
  } = $props();

  const uid = $props.id();
  let dialog: HTMLDialogElement;
  let returnTo: HTMLElement | null = null;
  // A selection drag from the panel that ends on the backdrop is not a dismiss: both ends must hit it.
  let downOnBackdrop = false;

  $effect(() => {
    if (open && !dialog.open) {
      returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  function close() {
    if (!dismissible) return;
    open = false;
  }

  function handleCancel(event: Event) {
    // Drive dismissal through state so Escape always closes only this (the
    // topmost native dialog) and reconciliation cannot reopen it.
    event.preventDefault();
    if (dismissible) close();
  }

  function handlePointerDown(event: PointerEvent) {
    downOnBackdrop = event.target === dialog;
    if (!dismissible && event.target instanceof Element && event.target.closest('.modal__x')) event.preventDefault();
  }

  function handleClose() {
    open = false;
    onclose?.();
    // Native restoration is not guaranteed everywhere. A caller may replace it
    // when its action removes the element that opened this dialog.
    const destination = returnFocus?.() ?? returnTo;
    if (destination?.isConnected) requestAnimationFrame(() => destination.isConnected && destination.focus());
    returnTo = null;
  }
</script>

<dialog
  bind:this={dialog}
   class={`modal ${className}`}
  class:modal--narrow={narrow}
  class:modal--wide={wide}
  aria-labelledby="{uid}-title"
  onclose={handleClose}
  oncancel={handleCancel}
  onpointerdown={handlePointerDown}
  onclick={(event) => {
    // The dialog element itself is only hit on its backdrop; the panel covers the rest.
    const dismiss = dismissible && lightDismiss && downOnBackdrop && event.target === dialog;
    downOnBackdrop = false;
    if (dismiss) close();
  }}
>
  <div class="modal__panel">
    <header class="modal__head">
      <div>
        {#if eyebrow}<p class="eyebrow">{eyebrow}</p>{/if}
        <h2 class="modal__title" id="{uid}-title">{title}</h2>
      </div>
      <button type="button" class="modal__x" disabled={!dismissible} onclick={close}><Icon name="x" label="Close" /></button>
    </header>
    <div class="modal__body" class:modal__body--flush={flush}>
      {@render children()}
    </div>
    {#if footer}
      <footer class="modal__foot">{@render footer(close)}</footer>
    {/if}
  </div>
</dialog>
