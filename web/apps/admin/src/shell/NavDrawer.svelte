<!--
  The phone's navigation drawer (M3E spec "Phone", gate G7): the expanded
  rail's content in a modal dialog. Opens from the top bar's menu button or a
  swipe in from the left edge; closes on Escape, ×, a tap on the scrim, or
  following a link. Focus stays inside while it is open and returns to the
  menu button when it closes without navigating (a followed link lands on the
  new page, as any route change does).
-->
<script lang="ts">
  import { Icon, initial } from '@kanade/ui';
  import type { Snippet } from 'svelte';
  import NavList from './NavList.svelte';

  let {
    open = $bindable(false),
    id,
    name,
    avatar = null,
    active,
    inbox = 0,
    counts = {},
    timezone = '',
    returnTo,
    account,
  }: {
    open: boolean;
    id: string;
    name: string;
    avatar?: string | null;
    active: string;
    inbox?: number;
    counts?: Partial<Record<string, number>>;
    timezone?: string;
    /** Where focus goes when the drawer closes without navigating. */
    returnTo?: HTMLElement;
    account: Snippet;
  } = $props();

  let dialog: HTMLDialogElement;
  let navigated = false;
  let downOnScrim = false;

  $effect(() => {
    if (open && !dialog.open) {
      navigated = false;
      dialog.showModal();
      dialog.querySelector<HTMLElement>('[aria-current="page"]')?.focus();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  // A link to the page already shown changes no route, so nothing would move
  // focus: only a real navigation skips returning it to the menu button.
  function follow(event: MouseEvent) {
    const link = event.currentTarget;
    navigated = link instanceof HTMLAnchorElement && link.pathname + link.search !== location.pathname + location.search;
    open = false;
  }

  function onclose() {
    open = false;
    if (!navigated) returnTo?.focus();
  }

  // showModal() makes the page inert, but Tab can still leave for the
  // browser's own chrome; wrap it inside the drawer instead.
  function onkeydown(event: KeyboardEvent) {
    if (event.key !== 'Tab') return;
    const focusable = [...dialog.querySelectorAll<HTMLElement>('a[href], button:not([disabled])')].filter(
      (el) => el.getClientRects().length > 0,
    );
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<dialog
  bind:this={dialog}
  {id}
  class="drawer"
  aria-label="Navigation"
  {onclose}
  {onkeydown}
  onpointerdown={(event) => (downOnScrim = event.target === dialog)}
  onclick={(event) => {
    // Only the scrim hits the dialog element itself; the panel covers the rest.
    if (downOnScrim && event.target === dialog) open = false;
    downOnScrim = false;
  }}
>
  {#if open}
    <div class="drawer__panel" data-fid="drawer">
      <div class="drawer__head" data-fid="drawer-head">
        <a class="navrail__brand" href="/" onclick={follow}>
          {#if avatar}
            <img class="brand__avatar navrail__tile" src={avatar} alt="" width="36" height="36" />
          {:else}
            <span class="brand__avatar navrail__tile" aria-hidden="true">{initial(name)}</span>
          {/if}
          <span class="brand__name">{name}</span>
        </a>
        <button type="button" class="drawer__close" data-fid="drawer-close" onclick={() => (open = false)}><Icon name="x" label="Close the navigation" /></button>
      </div>
      <NavList {active} {inbox} {counts} onnavigate={follow} />
      <div class="drawer__foot" data-fid="drawer-foot">
        {@render account()}
        <span class="drawer__keys" title="Commands: Ctrl K (Cmd K on a Mac)">Ctrl K</span>
        {#if timezone}<span class="drawer__tz" title="Guild timezone — every time here is in it">{timezone}</span>{/if}
      </div>
    </div>
  {/if}
</dialog>
