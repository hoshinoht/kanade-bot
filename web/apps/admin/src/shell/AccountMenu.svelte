<!--
  The masthead's account chip: who is signed in (and how), opening a small
  menu with "Your account", "Copy user ID" (when the id is known) and "Sign out". A menu
  button (APG pattern): Enter/Space/↓ open on the first item, ↑ on the last;
  arrows move, Home/End jump, Escape or Tab close, and focus returns to the chip.
-->
<script lang="ts">
  import type { Session } from '@kanade/api-types';
  import { Icon, initial } from '@kanade/ui';
  import { tick } from 'svelte';

  let {
    session,
    userId = null,
    oncopy,
    onsignout,
  }: {
    session: Session | null;
    /** The signed-in Discord user's id, when it can be told. */
    userId?: string | null;
    oncopy: (id: string) => void;
    onsignout: (event: MouseEvent) => void;
  } = $props();

  const uid = $props.id();
  const METHOD: Record<string, string> = { discord: 'Discord', tailscale: 'Tailscale', token: 'the admin token' };
  const who = $derived(session?.display ?? 'Signed in');
  let open = $state(false);
  let chip = $state<HTMLButtonElement>();
  let menu = $state<HTMLDivElement>();
  const items = () => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];

  async function show(at: 'first' | 'last') {
    open = true;
    await tick();
    const all = items();
    (at === 'first' ? all[0] : all[all.length - 1])?.focus();
  }

  function hide(refocus = true) {
    open = false;
    if (refocus) chip?.focus();
  }

  function onChipKey(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      void show('first');
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      void show('last');
    }
  }

  function onMenuKey(event: KeyboardEvent) {
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    const moves: Record<string, number> = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: all.length - 1 };
    if (event.key in moves) {
      event.preventDefault();
      all[(moves[event.key]! + all.length) % all.length]?.focus();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      hide();
    } else if (event.key === 'Tab') {
      hide(false);
    }
  }

  // A click anywhere else closes it, as a menu does.
  $effect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || (!menu?.contains(event.target) && !chip?.contains(event.target))) hide(false);
    };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  });
</script>

<div class="account">
  <button
    bind:this={chip}
    type="button"
    class="mchip account__chip"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls="{uid}-menu"
    aria-label="Account: {who}"
    onclick={() => (open ? hide() : void show('first'))}
    onkeydown={onChipKey}
  >
    <span class="account__initial" aria-hidden="true">{initial(who)}</span>
    <span class="account__name">{who}</span>
    <Icon name="chevron-down" />
  </button>
  {#if open}
    <div class="account__menu" id="{uid}-menu" role="menu" aria-label="Account" tabindex="-1" bind:this={menu} onkeydown={onMenuKey}>
      <p class="account__who">
        <strong>{who}</strong>{#if session?.method}<span>signed in with {METHOD[session.method] ?? session.method}</span>{/if}
      </p>
      <!-- The router takes the click; focus then moves to the page, as on any route change. -->
      <a role="menuitem" tabindex="-1" class="account__item" href="/account" onclick={() => hide(false)}><Icon name="users" /> Your account</a>
      {#if userId}
        <button
          type="button"
          role="menuitem"
          tabindex="-1"
          class="account__item"
          onclick={() => {
            oncopy(userId);
            hide();
          }}><Icon name="copy" /> Copy user ID</button
        >
      {/if}
      <a
        role="menuitem"
        tabindex="-1"
        class="account__item"
        href="/login"
        onclick={(event) => {
          hide(false);
          onsignout(event);
        }}><Icon name="log-out" /> Sign out</a
      >
    </div>
  {/if}
</div>
