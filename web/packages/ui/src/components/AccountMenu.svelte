<!--
  The account chip: who is signed in (and how), opening a small menu whose
  items each app passes in (`items`, given `hide`). A menu button (APG
  pattern): Enter/Space/↓ open on the first item, ↑ on the last; arrows move,
  Home/End jump, Escape or Tab close, and focus returns to the chip. Items are
  `role="menuitem" tabindex="-1"` elements with the `account__item` class.
-->
<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import Avatar from './Avatar.svelte';
  import Icon from './Icon.svelte';
  import { focusMenuItem, menuButtonKey, menuKey } from './menu';

  let {
    who,
    avatar = null,
    detail = '',
    items,
    ...data
  }: {
    who: string;
    /** The signed-in person's portrait URL; the initial when null. */
    avatar?: string | null;
    /** How they signed in ("signed in with Discord"), under the name in the menu. */
    detail?: string;
    /** The menu items; `hide(false)` closes without returning focus to the chip. */
    items: Snippet<[hide: (refocus?: boolean) => void]>;
    /** `data-*` attributes for the chip (a call site's layout-fidelity tag). */
    [attribute: `data-${string}`]: string | undefined;
  } = $props();

  const uid = $props.id();
  let open = $state(false);
  let chip = $state<HTMLButtonElement>();
  let menu = $state<HTMLDivElement>();

  async function show(at: 'first' | 'last') {
    open = true;
    await tick();
    focusMenuItem(menu, at);
  }

  function hide(refocus = true) {
    open = false;
    if (refocus) chip?.focus();
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
    onkeydown={(event) => menuButtonKey(event, (at) => void show(at))}
    {...data}
  >
    <Avatar class="account__initial" src={avatar} name={who} />
    <span class="account__name">{who}</span>
    <Icon name="chevron-down" />
  </button>
  {#if open}
    <div class="account__menu" id="{uid}-menu" role="menu" aria-label="Account" tabindex="-1" bind:this={menu} onkeydown={(event) => menuKey(event, menu, hide)}>
      <p class="account__who">
        <strong>{who}</strong>{#if detail}<span>{detail}</span>{/if}
      </p>
      {@render items(hide)}
    </div>
  {/if}
</div>
