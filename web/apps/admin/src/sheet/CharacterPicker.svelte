<!--
  A party member's character for one run (module `roster`; B_RosterRunPick,
  B_PhoneRosterRunPick): their active characters, the main crowned. On a wide
  sheet a popover by the slot's chip (a listbox: a choice saves at once); in
  the narrow sheet a bottom sheet with radios and "Use <IGN>". Clear pick
  goes back to inferred (one character) or nothing. Picks save live, never
  into a staged draft.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import type { CharacterChoice, RunCharacterSlot } from '@kanade/api-types';
  import { Icon, Modal } from '@kanade/ui';
  import { DEFAULT_REGION, regionLabel } from '../members/characters';

  let {
    slot,
    name,
    title,
    eyebrow = '',
    variant,
    open = $bindable(false),
    busy = false,
    staging = false,
    onpick,
    onclose,
    returnFocus,
  }: {
    slot: RunCharacterSlot;
    /** The member as the sheet names them ("Mika", "Ren (2)"). */
    name: string;
    /** The run ("HCarling + HStar"). */
    title: string;
    /** The sheet's line over its title: "HCarling + HStar · Tue 22:00". */
    eyebrow?: string;
    variant: 'popover' | 'sheet';
    /** The bottom sheet's state (the popover is mounted only while open). */
    open?: boolean;
    busy?: boolean;
    /** Run-sheet staging is on: say the pick still saves live. */
    staging?: boolean;
    /** A character id, or `null` to clear the pick. */
    onpick: (characterId: string | null) => void;
    /** The popover closed without a pick; `refocus` when Escape asked for it. */
    onclose?: (refocus: boolean) => void;
    /** Where focus returns when the bottom sheet closes. */
    returnFocus?: () => HTMLElement | null;
  } = $props();

  const uid = $props.id();
  const current = $derived(slot.picked ?? slot.inferred);
  let pop = $state<HTMLDivElement>();
  let list = $state<HTMLDivElement>();
  // The sheet's radio: the pick, else the inferred one, else the main, else the first.
  let choice = $state('');
  const chosen = $derived(slot.candidates.find((c) => c.id === choice) ?? null);

  $effect(() => {
    if (variant !== 'sheet' || !open) return;
    choice = untrack(() => current ?? slot.candidates.find((c) => c.role === 'main')?.id ?? slot.candidates[0]?.id ?? '');
  });

  // The popover takes focus on its selected (else first) choice, as a listbox does.
  $effect(() => {
    if (variant !== 'popover' || !list) return;
    const options = [...list.querySelectorAll<HTMLElement>('[role="option"]')];
    (options.find((o) => o.getAttribute('aria-selected') === 'true') ?? options[0])?.focus({ preventScroll: true });
  });

  // A press outside the popover (and outside the chip that toggles it) closes it.
  $effect(() => {
    if (variant !== 'popover') return;
    const away = (event: PointerEvent) => {
      if (event.target instanceof Element && (pop?.contains(event.target) || event.target.closest('.slotchar__btn[aria-expanded="true"]'))) return;
      onclose?.(false);
    };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  });

  function listKey(event: KeyboardEvent) {
    const options = [...(list?.querySelectorAll<HTMLElement>('[role="option"]') ?? [])];
    const at = options.indexOf(document.activeElement as HTMLElement);
    const moves: Record<string, number> = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: options.length - 1 };
    if (event.key in moves) {
      event.preventDefault();
      options[(moves[event.key]! + options.length) % options.length]?.focus();
    } else if ((event.key === 'Enter' || event.key === ' ') && at >= 0) {
      event.preventDefault();
      if (!busy) onpick(options[at]!.dataset.id!);
    }
  }

  function listClick(event: MouseEvent) {
    const option = event.target instanceof Element ? event.target.closest<HTMLElement>('[role="option"]') : null;
    if (option && !busy) onpick(option.dataset.id!);
  }

  function popKey(event: KeyboardEvent) {
    if (event.key !== 'Escape' || event.defaultPrevented) return;
    // The sheet's own Escape (close the run) must not fire too.
    event.preventDefault();
    event.stopPropagation();
    onclose?.(true);
  }

  // Tab past the last control leaves the popover: it closes, focus goes on.
  async function leave(event: FocusEvent) {
    if (event.relatedTarget instanceof Node && pop?.contains(event.relatedTarget)) return;
    await tick();
    if (!pop?.contains(document.activeElement)) onclose?.(false);
  }

  function use() {
    if (!chosen || busy) return;
    if (chosen.id === slot.picked) open = false;
    else onpick(chosen.id);
  }
</script>

{#snippet row(c: CharacterChoice)}
  <span class="charpick__id">
    <span class="charpick__ign"
      >{c.ign}{#if c.role === 'main'}<span class="charpick__crown"><Icon name="crown" />main</span>{/if}</span
    >
    <span class="charpick__meta">{c.class ?? 'No class'}{#if c.world !== DEFAULT_REGION}{` · ${regionLabel(c.world)}`}{/if}</span>
  </span>
  <span class="charpick__lv mono">{#if c.level}<small>Lv.</small> {c.level}{/if}</span>
{/snippet}

{#snippet extras()}
  {#if slot.picked}
    <button type="button" class="btn charpick__clear" disabled={busy} onclick={() => onpick(null)}
      >Clear pick{#if slot.candidates.length === 1}<span class="charpick__clearhint">&nbsp;· back to inferred</span>{/if}</button
    >
  {/if}
  {#if staging}<p class="charpick__note">Saves live: character picks are not staged in the draft.</p>{/if}
{/snippet}

{#if variant === 'popover'}
  <div class="charpick charpick--pop" role="dialog" tabindex="-1" aria-label="{name}'s character for {title}" data-fid="roster-picker" bind:this={pop} onkeydown={popKey} onfocusout={leave}>
    <p class="cap charpick__cap" id="{uid}-cap">{name}'s character for this run</p>
    <div class="charpick__options" role="listbox" aria-labelledby="{uid}-cap" tabindex="-1" bind:this={list} onkeydown={listKey} onclick={listClick}>
      {#each slot.candidates as c, index (c.id)}
        <div
          class="charpick__option"
          class:charpick__option--on={c.id === current}
          role="option"
          aria-selected={c.id === current}
          tabindex="-1"
          data-id={c.id}
          data-fid={index === 0 ? 'roster-picker-option' : undefined}
        >
          <span class="charpick__dot" aria-hidden="true"></span>
          {@render row(c)}
        </div>
      {/each}
    </div>
    {@render extras()}
  </div>
{:else}
  <Modal bind:open title="{name}'s character" {eyebrow} narrow className="charsheet" fid="roster-picker" fidParts={{}} {returnFocus}>
    <fieldset class="charpick charpick--sheet">
      <legend class="vh">{name}'s character for {title}</legend>
      {#each slot.candidates as c, index (c.id)}
        <label class="charpick__option" class:charpick__option--on={c.id === choice} data-fid={index === 0 ? 'roster-picker-option' : undefined}>
          <input type="radio" name="{uid}-char" value={c.id} bind:group={choice} />
          {@render row(c)}
        </label>
      {/each}
    </fieldset>
    {@render extras()}
    {#snippet footer(close)}
      <button type="button" class="btn" onclick={close}>Cancel</button>
      <button type="button" class="btn btn--primary charpick__use" disabled={!chosen || busy} onclick={use}>{chosen ? `Use ${chosen.ign}` : 'Use'}</button>
    {/snippet}
  </Modal>
{/if}
