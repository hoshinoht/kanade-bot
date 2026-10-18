<!--
  Who else has this open (boards B_PresenceInbox, B_PresenceRunSheet and their
  phone boards): avatars (an initial on the admin's colour; a pencil badge when
  editing; faded and dashed once stale) with the names, opening a popover with
  each admin's mode and age. Nothing when nobody else is here. `wide` is the
  phone's full-width row, which also carries the editing note; `dark` sits on
  a window's title bar. Not a live region (the page announces once, itself).
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { PresenceKind } from '@kanade/api-types';
  import '@kanade/ui/styles/screens/presence.scss';
  import PresenceAvatar from './PresenceAvatar.svelte';
  import { ageText, buttonLabel, doing, editingNote, editors, type Shown } from './presence';

  let { list, kind, variant = 'pill' }: { list: Shown[]; kind: PresenceKind; variant?: 'pill' | 'dark' | 'wide' } = $props();
  const uid = $props.id();
  let open = $state(false);
  let button = $state<HTMLButtonElement>();
  let root = $state<HTMLElement>();
  const busy = $derived(editors(list));
  const watching = $derived(list.filter((entry) => entry.mode !== 'editing'));
  const note = $derived(variant === 'wide' ? editingNote(kind, list, true) : null);
  const shownAvatars = $derived(list.slice(0, 3));

  async function close(refocus: boolean) {
    open = false;
    if (!refocus) return;
    await tick();
    button?.focus({ preventScroll: true });
  }
  // A click away closes it; Escape (from the button or inside) closes it and returns focus to the button.
  $effect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => {
      if (root && !root.contains(event.target as Node)) void close(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || !root?.contains(document.activeElement)) return;
      event.preventDefault();
      event.stopPropagation();
      void close(true);
    };
    document.addEventListener('pointerdown', away);
    // Capture, so a dialog around it (the run sheet) does not close first.
    document.addEventListener('keydown', escape, true);
    return () => {
      document.removeEventListener('pointerdown', away);
      document.removeEventListener('keydown', escape, true);
    };
  });
  // The last one left: the popover goes with the button.
  $effect(() => {
    if (!list.length) open = false;
  });
</script>

{#snippet viewers(prefix: string)}
  {#if watching.length}<span class="presence__dim">{prefix}{watching.map((entry) => `${entry.name} viewing${variant === 'wide' && entry.stale ? `, ${ageText(entry.age)}` : ''}`).join(' · ')}</span>{/if}
{/snippet}

{#if list.length}
  <div class="presence presence--{variant}" bind:this={root}>
    <button
      type="button"
      class="presence__btn"
      data-fid="presence"
      bind:this={button}
      aria-haspopup="dialog"
      aria-expanded={open}
      aria-controls="{uid}-pop"
      aria-label={buttonLabel(list)}
      onclick={() => (open = !open)}
    >
      <span class="presence__stack">
        {#each shownAvatars as entry (entry.id)}<PresenceAvatar {entry} />{/each}
      </span>
      {#if variant === 'wide'}
        <span class="presence__lines">
          <span>
            {#if note}<b>{note.lead}</b>{@render viewers(' · ')}
            {:else}<b>{watching[0]!.name} is viewing this {kind === 'run' ? 'run' : 'request'}</b>{#if watching.length > 1}<span class="presence__dim">{watching.slice(1).map((entry) => ` · ${entry.name} viewing`).join('')}</span>{/if}{/if}
          </span>
          {#if note}<span class="presence__rule">{note.rule}</span>{/if}
        </span>
      {:else}
        <span class="presence__names">
          {#each busy as entry, i (entry.id)}{#if i}<span>&nbsp;·&nbsp;</span>{/if}<b>{entry.name}</b> editing{/each}{@render viewers(busy.length ? ' · ' : '')}
        </span>
      {/if}
    </button>
    {#if open}
      <div
        class="presence__pop"
        id="{uid}-pop"
        data-fid="presence-pop"
        role="dialog"
        aria-label="Admins here"
        tabindex="-1"
      >
        <span class="cap presence__cap">Here now · admins only</span>
        <ul class="presence__rows">
          {#each list as entry (entry.id)}
            <li class="presence__row" class:presence__row--stale={entry.stale}>
              <PresenceAvatar {entry} />
              <span><b>{entry.name}</b> · {doing(entry, kind)}</span>
              <span class="mono presence__age">{ageText(entry.age)}</span>
            </li>
          {/each}
        </ul>
        <span class="presence__hint">A hint, not a lock. Names fade 20 s after a tab stops checking in and go after 60 s.</span>
      </div>
    {/if}
  </div>
{/if}
