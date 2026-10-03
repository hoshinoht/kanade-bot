<!--
  Config's theme card (B_CfgTheme): colourway tiles with a small window
  preview, grouped into labelled sets inside one radio group, then the mode
  as a connected group. The same store and storage as
  the shared ThemePicker (which the public app keeps); native radios, so
  arrows move the choice and it applies at once.
-->
<script lang="ts">
  import {
    applyColorway,
    applyMode,
    COLORWAY_GROUPS,
    currentColorway,
    currentMode,
    openColorwaySets,
    refreshDynamic,
    rememberColorwaySet,
    setOf,
    type Colorway,
    type ThemeMode,
  } from '@kanade/ui';

  let colorway = $state<Colorway>(currentColorway());
  let mode = $state<ThemeMode>(currentMode());
  const uid = $props.id();
  // Expanded sets: the current colourway's set to begin with, then as toggled (memory only).
  let open = $state<Record<string, boolean>>(openColorwaySets());

  function toggle(key: string): void {
    open[key] = !open[key];
    rememberColorwaySet(key, open[key]);
  }

  // Palette commands change the theme too; keep the radios in step, and open
  // the set they chose so its checked radio is never hidden.
  $effect(() => {
    const observer = new MutationObserver(() => {
      colorway = currentColorway();
      mode = currentMode();
      const set = setOf(colorway);
      if (!open[set]) toggle(set);
    });
    observer.observe(document.documentElement, { attributeFilter: ['data-colorway', 'data-theme'] });
    return () => observer.disconnect();
  });

  // The Dynamic tile previews the avatar's palette.
  $effect(() => void refreshDynamic());

  const MODES: { key: ThemeMode; name: string }[] = [
    { key: 'system', name: 'System' },
    { key: 'light', name: 'Light' },
    { key: 'dark', name: 'Dark' },
  ];
</script>

<fieldset class="tiles__set">
  <legend class="cap">Colourway</legend>
  {#each COLORWAY_GROUPS as group (group.key)}
    <fieldset class="tiles__group">
      <legend class="setbar">
        <button
          type="button"
          class="setbar__toggle"
          aria-expanded={!!open[group.key]}
          aria-controls="{uid}-{group.key}"
          onclick={() => toggle(group.key)}
        >
          <span class="setbar__chev" aria-hidden="true"></span>
          <span class="tiles__label">{group.name}</span>
          {#if !open[group.key]}
            <span class="setbar__minis" aria-hidden="true">
              {#each group.ways as way (way.key)}<i class="setbar__mini swatch__dots--{way.key}"></i>{/each}
            </span>
          {/if}
        </button>
      </legend>
      <div class="setbody" class:setbody--open={open[group.key]} id="{uid}-{group.key}">
        <div class="setbody__inner">
          {#if 'note' in group}<p class="tiles__note">{group.note}</p>{/if}
          <div class="tiles">
            {#each group.ways as way (way.key)}
              <label class="tile">
                <input
                  type="radio"
                  name="{uid}-colorway"
                  value={way.key}
                  checked={colorway === way.key}
                  onchange={() => {
                    colorway = way.key;
                    applyColorway(way.key);
                  }}
                />
                <span class="tile__mini tile__mini--{way.key}" aria-hidden="true"><i></i><b></b></span>
                <span class="tile__name">{way.name}</span>
              </label>
            {/each}
          </div>
        </div>
      </div>
    </fieldset>
  {/each}
</fieldset>
<fieldset class="tiles__set">
  <legend class="cap">Mode</legend>
  <div class="tiles__seg">
    {#each MODES as item (item.key)}
      <label class="tiles__mode">
        <input
          type="radio"
          name="{uid}-mode"
          value={item.key}
          checked={mode === item.key}
          onchange={() => {
            mode = item.key;
            applyMode(item.key);
          }}
        />
        <span>{item.name}</span>
      </label>
    {/each}
  </div>
</fieldset>
