<!--
  A party member's character for one run (module `roster`; B_RosterRunPick,
  B_PhoneRosterRunPick) or one weekly timing (B_FixedReadyPick,
  B_PhoneFixedReadyPick), or for one later boss of either (`boss`;
  B_RunBossPick, B_FixedBoss2Pick, B_PhoneFixedBossPick): their active
  characters, the main crowned. On a wide screen a popover by the chip (a
  listbox: a choice saves at once); in the narrow sheet or editor a bottom
  sheet with radios and "Use <IGN>". On a run, Clear pick goes back to the
  default (the timing's pick, the main or the only character; for a later
  boss the timing's pick for it, else the first boss's character); a timing's
  caller clears when the default is chosen. Picks save live, never into a
  staged draft or a form.
-->
<script lang="ts">
  import { tick, untrack, type Snippet } from 'svelte';
  import type { CharacterChoice, RunCharacterSlot } from '@kanade/api-types';
  import { DEFAULT_REGION, Icon, Modal, regionLabel } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-pick.scss';

  let {
    slot,
    name,
    title,
    eyebrow = '',
    variant,
    what = 'run',
    fid = 'roster-picker',
    optionFid = 'roster-picker-option',
    tagEveryOption = false,
    open = $bindable(false),
    busy = false,
    staging = false,
    meta,
    under,
    note,
    boss = null,
    onpick,
    onclose,
    returnFocus,
  }: {
    slot: RunCharacterSlot;
    /** The member as the sheet names them ("Mika", "Ren (2)"). */
    name: string;
    /** The run or timing ("HCarling + HStar"). */
    title: string;
    /** The sheet's line over its title: "HCarling + HStar · Tue 22:00". */
    eyebrow?: string;
    variant: 'popover' | 'sheet';
    /** A run's pick, or a weekly timing's. */
    what?: 'run' | 'timing';
    fid?: string;
    /** The options' `data-fid`: the first one's, or with `tagEveryOption` each one's (as its board tags them). */
    optionFid?: string;
    tagEveryOption?: boolean;
    /** The bottom sheet's state (the popover is mounted only while open). */
    open?: boolean;
    busy?: boolean;
    /** Run-sheet staging is on: say the pick still saves live. */
    staging?: boolean;
    /** More words after a choice's class ("the default", "picked 12 Oct by Asahi"). */
    meta?: (c: CharacterChoice) => string[];
    /** A choice below an entry level: tinted, still offered (it never blocks). */
    under?: (c: CharacterChoice) => boolean;
    /** Lines under the choices. */
    note?: Snippet;
    /** A character id, or `null` to clear the pick. */
    onpick: (characterId: string | null) => void;
    /** The popover closed without a pick; `refocus` when Escape asked for it. */
    onclose?: (refocus: boolean) => void;
    /** Where focus returns when the bottom sheet closes. */
    returnFocus?: () => HTMLElement | null;
    /** A later boss's pick: that boss's token, the first boss's, and what it brings and has picked. */
    boss?: { token: string; first: string; brings: string | null; picked: string | null } | null;
  } = $props();

  const uid = $props.id();
  const current = $derived(boss ? boss.brings : slot.brings);
  let pop = $state<HTMLDivElement>();
  let list = $state<HTMLDivElement>();
  // The sheet's radio: what the member brings, else the main, else the first.
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
      if (event.target instanceof Element && (pop?.contains(event.target) || event.target.closest('[data-char-for][aria-expanded="true"]'))) return;
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
    // The sheet's own Escape (close the run or the editor) must not fire too.
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
    if (chosen.id === current) open = false;
    else onpick(chosen.id);
  }
</script>

{#snippet row(c: CharacterChoice)}
  <span class="charpick__id">
    <span class="charpick__ign"
      >{c.ign}{#if c.role === 'main'}<span class="charpick__crown"><Icon name="crown" />main</span>{/if}</span
    >
    <span class="charpick__meta"
      >{[`${c.class ?? 'No class'}${c.world !== DEFAULT_REGION ? ` · ${regionLabel(c.world)}` : ''}`, ...(meta?.(c) ?? [])].join(' · ')}</span
    >
  </span>
  <span class="charpick__lv mono">{#if c.level}<small>Lv.</small> {c.level}{/if}</span>
{/snippet}

{#snippet extras()}
  <!-- A timing goes back to its default by choosing it (a later boss: the first boss's character); with no main and several characters there is none to choose. -->
  {#if boss ? boss.picked && what === 'run' : slot.picked && (what === 'run' || !(slot.candidates.some((c) => c.role === 'main') || slot.candidates.length === 1))}
    <button type="button" class="btn charpick__clear" disabled={busy} onclick={() => onpick(null)}
      >Clear pick<span class="charpick__clearhint">&nbsp;· {what === 'run' ? 'back to the default' : 'nothing is brought'}</span></button
    >
  {/if}
  {#if slot.retired}<p class="charpick__note">{slot.retired} retired character{slot.retired === 1 ? '' : 's'} not offered.</p>{/if}
  {@render note?.()}
  {#if staging}<p class="charpick__note">Saves live: character picks are not staged in the draft.</p>{/if}
{/snippet}

{#if variant === 'popover'}
  <div
    class="charpick charpick--pop"
    role="dialog"
    tabindex="-1"
    aria-label="{name}'s character for {boss ? `${boss.token} on ${title}` : title}"
    data-fid={fid}
    bind:this={pop}
    onkeydown={popKey}
    onfocusout={leave}
  >
    <p class="cap charpick__cap" id="{uid}-cap">{name}'s character for {boss ? boss.token : `this ${what}`}</p>
    <div class="charpick__options" role="listbox" aria-labelledby="{uid}-cap" tabindex="-1" bind:this={list} onkeydown={listKey} onclick={listClick}>
      {#each slot.candidates as c, index (c.id)}
        <div
          class="charpick__option"
          class:charpick__option--on={c.id === current}
          class:charpick__option--under={under?.(c)}
          role="option"
          aria-selected={c.id === current}
          tabindex="-1"
          data-id={c.id}
          data-fid={index === 0 || tagEveryOption ? optionFid : undefined}
        >
          <span class="charpick__dot" aria-hidden="true"></span>
          {@render row(c)}
        </div>
      {/each}
    </div>
    {@render extras()}
  </div>
{:else}
  <Modal bind:open title="{name}'s character{boss ? ` for ${boss.token}` : what === 'timing' ? ' for this timing' : ''}" {eyebrow} narrow className="charsheet" {fid} fidParts={{}} {returnFocus}>
    <fieldset class="charpick charpick--sheet">
      <legend class="vh">{name}'s character for {boss ? `${boss.token} on ${title}` : title}</legend>
      {#each slot.candidates as c, index (c.id)}
        <label class="charpick__option" class:charpick__option--on={c.id === choice} class:charpick__option--under={under?.(c)} data-fid={index === 0 || tagEveryOption ? optionFid : undefined}>
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
