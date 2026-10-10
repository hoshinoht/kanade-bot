<!--
  The dropdown's listbox (P_Select `.lb`): group headings, rows with an icon
  or monogram and secondary text, the active row (aria-activedescendant on the
  trigger or search box), and a check on the chosen row or a box per row.
-->
<script lang="ts">
  import Icon from './Icon.svelte';
  import { groupsOf, marked, type SelectOption } from './select';

  let {
    id,
    label,
    options,
    active,
    isOn,
    idOf,
    onpick,
    query = '',
    multi = false,
  }: {
    id: string;
    label: string;
    options: SelectOption[];
    active: string | null;
    isOn: (value: string) => boolean;
    idOf: (value: string) => string;
    onpick: (value: string) => void;
    query?: string;
    multi?: boolean;
  } = $props();

  const groups = $derived(groupsOf(options));
</script>

{#snippet content(o: SelectOption, on: boolean)}
  {@const [pre, hit, post] = marked(o.label, query)}
  {#if multi}<span class="dd-box" aria-hidden="true">{#if on}<Icon name="check" />{/if}</span>{/if}
  {#if o.mono}
    <span class="dd-mono" aria-hidden="true">{o.mono}</span>
  {:else if o.icon}
    <span class="dd-lead" aria-hidden="true"><Icon name={o.icon} /></span>
  {/if}
  <span class="dd-opt__text"
    ><span class="dd-opt__label">{pre}{#if hit}<mark>{hit}</mark>{/if}{post}</span>{#if o.sub}<span class="dd-opt__sub">{o.sub}</span>{/if}</span
  >
  {#if on && !multi}<span class="dd-opt__check"><Icon name="check" /></span>{/if}
{/snippet}

<!-- Focus stays on the trigger (select-only combobox): rows take no tab stop, and a press picks without moving it.
     The two branches differ only in their (literal, build-stripped) fidelity tag. -->
{#snippet row(o: SelectOption)}
  {@const on = isOn(o.value)}
  {#if multi}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
    <div
      class="dd-opt"
      class:dd-opt--act={o.value === active}
      class:dd-opt--on={on}
      class:dd-opt--dis={o.disabled}
      role="option"
      id={idOf(o.value)}
      aria-selected={on}
      aria-disabled={o.disabled || undefined}
      data-fid="ddm-row"
      data-value={o.value}
      onclick={() => !o.disabled && onpick(o.value)}
    >
      {@render content(o, on)}
    </div>
  {:else}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
    <div
      class="dd-opt"
      class:dd-opt--act={o.value === active}
      class:dd-opt--sel={on}
      class:dd-opt--dis={o.disabled}
      role="option"
      id={idOf(o.value)}
      aria-selected={on}
      aria-disabled={o.disabled || undefined}
      data-fid="dd-row"
      data-value={o.value}
      onclick={() => !o.disabled && onpick(o.value)}
    >
      {@render content(o, on)}
    </div>
  {/if}
{/snippet}

<div class="dd-list" role="listbox" {id} aria-label={label} aria-multiselectable={multi || undefined} tabindex="-1">
  {#each groups as group, g (g)}
    {#if group.label}
      <div class="dd-group" role="group" aria-labelledby="{id}-g{g}">
        <div class="dd-group__label" id="{id}-g{g}" data-fid="dd-group" aria-hidden="true">{group.label}</div>
        {#each group.options as o (o.value)}{@render row(o)}{/each}
      </div>
    {:else}
      {#each group.options as o (o.value)}{@render row(o)}{/each}
    {/if}
  {/each}
</div>
