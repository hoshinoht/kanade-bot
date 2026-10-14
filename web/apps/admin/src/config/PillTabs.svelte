<!--
  In-panel pill tabs for a long section (spec: Persona "Active persona &
  profiles" / "Role overrides n"; Models "Roles" / "Context windows" /
  "Capacity"). The caller keeps every panel mounted (hidden when not
  selected), so a draft survives switching; arrows move between the tabs.
-->
<script lang="ts">
  let {
    id,
    label,
    tabs,
    selected = $bindable(),
  }: { id: string; label: string; tabs: { key: string; label: string; count?: number | string }[]; selected: string } = $props();

  const buttons: Record<string, HTMLButtonElement> = {};

  function onkeydown(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: tabs.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = tabs[(target + tabs.length) % tabs.length]!;
    selected = next.key;
    buttons[next.key]?.focus();
  }
</script>

<div class="settings__pills" role="tablist" aria-label={label} data-fid="cfg-subtabs">
  {#each tabs as tab, index (tab.key)}
    <button
      type="button"
      role="tab"
      class="settings__pill"
      id="{id}-tab-{tab.key}"
      aria-selected={selected === tab.key}
      aria-controls="{id}-panel-{tab.key}"
      tabindex={selected === tab.key ? 0 : -1}
      bind:this={buttons[tab.key]}
      onclick={() => (selected = tab.key)}
      onkeydown={(event) => onkeydown(event, index)}
      >{tab.label}{#if tab.count !== undefined}<span class="settings__pillcount">{tab.count}</span>{/if}</button
    >
  {/each}
</div>
