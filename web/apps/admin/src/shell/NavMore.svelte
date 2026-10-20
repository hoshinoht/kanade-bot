<!--
  The rail's More item (rail option B, boards B_RailMore800/720/Drawer): the
  rarely visited destinations (`more` in routes.ts) behind one item, so the
  rail keeps its full-size items at 800 px tall. On one of their pages More
  takes that page's name, glyph and highlight; their badges add up on it.

  Rail: a menu button (APG pattern, as AccountMenu): Enter/Space/↓ open on
  the first item, ↑ on the last; arrows move and wrap, Home/End jump, Escape
  closes and returns focus to More, Tab or a click elsewhere closes. Choosing
  a page moves focus to the page, as any route change does.
  Drawer (`inline`): a disclosure row that opens the links in place, open
  from the start when the current page is one of them.
-->
<script lang="ts">
  import { focusMenuItem, Icon, menuButtonKey, menuKey, pulse } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import { arrival } from '../resource.svelte';
  import { sumBadges, type Section } from '../routes';

  let {
    more,
    active,
    badges = {},
    counts = {},
    inline = false,
    onnavigate,
  }: {
    /** The destinations behind More (`more` in routes.ts) whose module is on. */
    more: Section[];
    active: string;
    badges?: Partial<Record<string, number>>;
    counts?: Partial<Record<string, number>>;
    /** The drawer's row, opening in place, instead of the rail's menu. */
    inline?: boolean;
    onnavigate?: (event: MouseEvent) => void;
  } = $props();

  const uid = $props.id();
  const current = $derived(more.find((s) => s.key === active));
  const badge = $derived(sumBadges(more, badges));
  // The drawer mounts on each opening: it starts open on one of these pages.
  let open = $state(untrack(() => inline && more.some((s) => s.key === active)));
  // Open in the drawer, the current page's own link is the highlight.
  const here = $derived(current && !(inline && open) ? current : undefined);
  const name = $derived(
    [here ? `More, current page ${here.label}` : 'More', badge > 0 ? `${badge} waiting` : ''].filter(Boolean).join(', '),
  );
  /** A badged destination's name, as NavList says it ("Setup 2 waiting"); unbadged links keep their text. */
  const waiting = (section: { key: string; label: string }): string | undefined => {
    const count = badges[section.key] ?? 0;
    return count > 0 ? `${section.label} ${count} waiting` : undefined;
  };

  let button = $state<HTMLButtonElement>();
  let menu = $state<HTMLDivElement>();

  async function show(at: 'first' | 'last') {
    open = true;
    await tick();
    focusMenuItem(menu, at);
  }

  function hide(refocus = true) {
    open = false;
    if (refocus) button?.focus();
  }

  // A click anywhere else closes the menu, as a menu does.
  $effect(() => {
    if (inline || !open) return;
    const away = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || (!menu?.contains(event.target) && !button?.contains(event.target))) hide(false);
    };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  });
</script>

{#snippet face()}
  <span class="navlist__ind"><Icon name={here?.icon ?? 'more-horizontal'} /></span><span class="navlist__label">{here?.label ?? 'More'}</span
  >{#if badge > 0}<span class="navlist__badge" {@attach pulse(badge, arrival.seq)}>{badge}<span class="vh"> waiting</span></span>{/if}
{/snippet}

<div class="navmore" class:navmore--inline={inline}>
  {#if inline}
    <button
      bind:this={button}
      type="button"
      class="navlist__item navmore__button"
      class:navlist__item--here={here !== undefined}
      data-fid="drawer-more"
      aria-expanded={open}
      aria-controls="{uid}-list"
      aria-label={name === 'More' ? undefined : name}
      onclick={async () => {
        open = !open;
        await tick();
        // The drawer's list scrolls: bring the opened links into view.
        if (open) document.getElementById(`${uid}-list`)?.scrollIntoView({ block: 'nearest' });
      }}
      >{@render face()}{#if !open}<span class="navmore__peek">{more.filter((s) => s !== here).map((s) => s.label).join(', ')}</span>{/if}<span
        class="navmore__chevron"
        class:navmore__chevron--open={open}><Icon name="chevron-down" /></span
      ></button
    >
    {#if open}
      <div class="navmore__list" id="{uid}-list">
        {#each more as section (section.key)}
          <a class="navlist__item" href={section.href} aria-current={section.key === active ? 'page' : undefined} aria-label={waiting(section)} onclick={onnavigate}
            ><span class="navlist__ind"><Icon name={section.icon} /></span><span class="navlist__label">{section.label}</span
            >{#if (badges[section.key] ?? 0) > 0}<span class="navlist__badge">{badges[section.key]}<span class="vh"> waiting</span></span
              >{:else if counts[section.key] !== undefined}<span class="navlist__count" aria-hidden="true">{counts[section.key]}</span>{/if}</a
          >
        {/each}
      </div>
    {/if}
  {:else}
    <button
      bind:this={button}
      type="button"
      class="navlist__item navmore__button"
      class:navlist__item--here={here !== undefined}
      data-fid="rail-more"
      aria-haspopup="menu"
      aria-expanded={open}
      aria-controls="{uid}-menu"
      aria-label={name === 'More' ? undefined : name}
      onclick={() => (open ? hide() : void show('first'))}
      onkeydown={(event) => menuButtonKey(event, (at) => void show(at))}>{@render face()}</button
    >
    {#if open}
      <div class="navmore__menu" id="{uid}-menu" role="menu" aria-label="More destinations" tabindex="-1" data-fid="rail-more-menu" bind:this={menu} onkeydown={(event) => menuKey(event, menu, hide)}>
        <span class="cap navmore__cap" aria-hidden="true">More</span>
        {#each more as section (section.key)}
          <a
            role="menuitem"
            tabindex="-1"
            class="navmore__item"
            href={section.href}
            aria-current={section.key === active ? 'page' : undefined}
            aria-label={waiting(section)}
            data-fid={section.key === 'guides' ? 'rail-more-guides' : undefined}
            onclick={() => hide(false)}
            ><Icon name={section.icon} /><span class="navmore__label">{section.label}</span>{#if (badges[section.key] ?? 0) > 0}<span class="navmore__badge"
                >{badges[section.key]}<span class="vh"> waiting</span></span
              >{/if}</a
          >
        {/each}
        <p class="navmore__note">On one of these pages, More takes its name and indicator.</p>
      </div>
    {/if}
  {/if}
</div>
