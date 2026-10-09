<!-- The guide's pill tabs (the run pane's `.ptab`): arrows, Home and End move between them. -->
<script lang="ts" generics="T extends string">
  let {
    tabs,
    selected,
    id,
    onselect,
    fid,
  }: {
    tabs: { id: T; label: string; count: number | null }[];
    selected: T;
    id: string;
    onselect: (tab: T) => void;
    /** Fidelity tag (member portal). */
    fid?: string;
  } = $props();

  const buttons: Record<string, HTMLButtonElement> = {};

  function onkeydown(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: tabs.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = tabs[(target + tabs.length) % tabs.length]!;
    onselect(next.id);
    buttons[next.id]?.focus();
  }
</script>

<div class="guide-tabs" role="tablist" aria-label="Guide sections" data-fid={fid}>
  {#each tabs as tab, index (tab.id)}
    <button
      type="button"
      role="tab"
      class="ptab"
      id="{id}-tab-{tab.id}"
      aria-selected={selected === tab.id}
      aria-controls="{id}-panel"
      tabindex={selected === tab.id ? 0 : -1}
      bind:this={buttons[tab.id]}
      onclick={() => onselect(tab.id)}
      onkeydown={(event) => onkeydown(event, index)}
      >{tab.label}{#if tab.count !== null} <span class="ptab__count mono">{tab.count}</span>{/if}</button
    >
  {/each}
</div>
