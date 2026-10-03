<!--
  Config's theme card (B_CfgTheme): colourway tiles with a small window
  preview, then the mode as a connected group. The same store and storage as
  the shared ThemePicker (which the public app keeps); native radios, so
  arrows move the choice and it applies at once.
-->
<script lang="ts">
  import { applyColorway, applyMode, COLORWAYS, currentColorway, currentMode, type Colorway, type ThemeMode } from '@kanade/ui';

  let colorway = $state<Colorway>(currentColorway());
  let mode = $state<ThemeMode>(currentMode());
  const uid = $props.id();

  // Palette commands change the theme too; keep the radios in step.
  $effect(() => {
    const observer = new MutationObserver(() => {
      colorway = currentColorway();
      mode = currentMode();
    });
    observer.observe(document.documentElement, { attributeFilter: ['data-colorway', 'data-theme'] });
    return () => observer.disconnect();
  });

  const MODES: { key: ThemeMode; name: string }[] = [
    { key: 'system', name: 'System' },
    { key: 'light', name: 'Light' },
    { key: 'dark', name: 'Dark' },
  ];
</script>

<fieldset class="tiles__set">
  <legend class="cap">Colourway</legend>
  <div class="tiles">
    {#each COLORWAYS as way (way.key)}
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
