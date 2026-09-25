<script lang="ts">
  import { applyColorway, applyMode, COLORWAYS, currentColorway, currentMode, type Colorway, type ThemeMode } from '../theme/theme';

  let { onchange }: { onchange?: (label: string) => void } = $props();

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

<fieldset class="field">
  <legend class="label">Colourway</legend>
  <div class="swatches">
    {#each COLORWAYS as way (way.key)}
      <label class="swatch">
        <input
          type="radio"
          name="{uid}-colorway"
          value={way.key}
          checked={colorway === way.key}
          onchange={() => {
            colorway = way.key;
            applyColorway(way.key);
            onchange?.(`Colourway ${way.name}`);
          }}
        />
        <span class="swatch__dots swatch__dots--{way.key}" aria-hidden="true"></span>
        <span class="swatch__name">{way.name}</span>
      </label>
    {/each}
  </div>
</fieldset>
<fieldset class="field">
  <legend class="label">Mode</legend>
  <div class="swatches">
    {#each MODES as item (item.key)}
      <label class="swatch swatch--plain">
        <input
          type="radio"
          name="{uid}-mode"
          value={item.key}
          checked={mode === item.key}
          onchange={() => {
            mode = item.key;
            applyMode(item.key);
            onchange?.(`${item.name} mode`);
          }}
        />
        <span class="swatch__name">{item.name}</span>
      </label>
    {/each}
  </div>
</fieldset>
<p class="note">Kept in this browser only; nothing is sent to the server.</p>
