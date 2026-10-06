<!--
  Account › This browser (A9, A10): settings kept in this browser only —
  colourway and mode (Config's theme tiles), Reduce motion, how Discord links
  open, Experiments — and the keyboard shortcuts.
-->
<script lang="ts">
  import { experiments, Icon, motionPreference, setExperiments } from '@kanade/ui';
  import ThemeTiles from '../config/ThemeTiles.svelte';
  import { discordLinks } from '../shared/discordLink.svelte';

  const KEYS: { keys: string[]; what: string }[] = [
    { keys: ['Ctrl', 'K'], what: 'Open commands' },
    { keys: ['Esc'], what: 'Close a pane or dialog' },
    { keys: ['↑', '↓'], what: 'Move through a list' },
    { keys: ['Enter'], what: 'Open the selected row' },
    { keys: ['M'], what: 'Move the focused run (Week)' },
    { keys: ['Ctrl', 'Z'], what: 'Undo the last move' },
    { keys: ['PgUp', 'PgDn'], what: 'Time picker: one hour' },
  ];
  const KEY_NAMES: Record<string, string> = { '↑': 'Up arrow', '↓': 'Down arrow', Esc: 'Escape', PgUp: 'Page up', PgDn: 'Page down' };
</script>

{#snippet toggle(id: string, title: string, note: string, on: boolean, flip: () => void)}
  <div class="account-row account-row--plain">
    <span class="account-row__text">
      <span class="account-row__title" id="{id}-t">{title}</span>
      <span class="account-row__sub" id="{id}-d">{note}</span>
    </span>
    <button type="button" role="switch" class="switch" aria-checked={on} aria-labelledby="{id}-t" aria-describedby="{id}-d" onclick={flip}
      ><span class="switch__knob" aria-hidden="true"></span></button
    >
  </div>
{/snippet}

<div class="account-browser" data-fid="account-browser">
  <p class="settings__box"><Icon name="monitor" /><span>Saved in <b>this browser only</b>. Your other devices keep their own look and settings.</span></p>
  <div class="account-browser__cols">
    <div class="account-browser__col">
      <section class="account-sec" aria-label="Look">
        <ThemeTiles />
      </section>
    </div>
    <div class="account-browser__col">
      <section class="account-sec" aria-labelledby="account-device">
        <div class="account-sec__head"><h3 class="cap" id="account-device">Motion, links and trials</h3></div>
        <div class="account-grp">
          {@render toggle(
            'account-motion',
            'Reduce motion',
            'Still shapes and flat bars, even if your system allows motion',
            motionPreference.reduce,
            () => motionPreference.set(!motionPreference.reduce),
          )}
          {@render toggle(
            'account-links',
            'Open Discord links in the app',
            'Applies only to this browser. Without the Discord app installed here the links will not open, so turn this off to open them in the browser.',
            discordLinks.app,
            () => discordLinks.set(!discordLinks.app),
          )}
          {@render toggle('account-exp', 'Experiments', 'Features still being tuned', experiments.on, () => setExperiments(!experiments.on))}
        </div>
      </section>
      <section class="account-sec" aria-labelledby="account-keys">
        <div class="account-sec__head"><h3 class="cap" id="account-keys">Keyboard shortcuts</h3></div>
        <ul class="account-grp">
          {#each KEYS as row (row.what)}
            <li class="account-row account-row--key">
              <span class="account-keys">
                <span class="vh">{row.keys.map((k) => KEY_NAMES[k] ?? k).join(' ')}</span>
                {#each row.keys as key (key)}<kbd class="account-kbd" aria-hidden="true">{key}</kbd>{/each}
              </span>
              <span>{row.what}</span>
            </li>
          {/each}
        </ul>
      </section>
    </div>
  </div>
</div>
