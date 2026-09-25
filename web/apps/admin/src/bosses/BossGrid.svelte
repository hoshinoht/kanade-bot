<script lang="ts">
  import '@kanade/ui/styles/boss-grid.scss';
  import type { BossRow } from '@kanade/api-types';
  import { Portrait } from '@kanade/ui';

  let {
    rows,
    selected = $bindable([]),
    readonly = false,
  }: { rows: BossRow[]; selected?: string[]; readonly?: boolean } = $props();

  // The grid wants a Boss; rows carry the same art fields.
  const asBoss = (row: BossRow) => ({
    token: row.key,
    key: row.key,
    name: row.name,
    difficulty: 'n' as const,
    level: row.level,
    portrait: row.portrait,
    portrait_sm: row.portrait,
    art: null,
    hue: row.hue,
  });

  function toggle(token: string, on: boolean) {
    selected = on ? [...selected, token] : selected.filter((t) => t !== token);
  }
</script>

<!-- v4 macros.boss_grid: each difficulty a pill; checked = filled with a tick, so state is not colour alone. -->
<div class="grid-bosses" role="group" aria-label="Bosses">
  {#each rows as row (row.key)}
    {@const on = readonly ? row.difficulties.some((d) => d.in_use) : row.difficulties.some((d) => selected.includes(d.token))}
    <div class="bossrow" class:bossrow--on={on}>
      <div class="bossrow__id">
        <Portrait boss={asBoss(row)} size="md" />
        <span>
          {#if readonly}<a class="bossrow__name" href="/bosses/{row.key}/knowledge">{row.name}</a>
          {:else}<span class="bossrow__name">{row.name}</span>{/if}
          <span class="bossrow__lv">Lv. {row.level}</span>
        </span>
      </div>
      <div class="bossrow__pills" role="group" aria-label="{row.name} difficulties">
        {#each row.difficulties as option (option.token)}
          {#if readonly}
            <span class="pill-toggle pill-toggle--{option.letter}" class:pill-toggle--on={option.in_use}>
              <span class="pill-toggle__tick" aria-hidden="true">✓</span>{option.name.toUpperCase()}
              <span class="vh">{option.in_use ? '(has a weekly timing)' : '(not run)'}</span>
            </span>
          {:else}
            <label class="pill-toggle pill-toggle--{option.letter}">
              <input
                type="checkbox"
                value={option.token}
                checked={selected.includes(option.token)}
                onchange={(event) => toggle(option.token, event.currentTarget.checked)}
                aria-label="{option.name} {row.name}"
              />
              <span class="pill-toggle__tick" aria-hidden="true">✓</span>{option.name.toUpperCase()}
            </label>
          {/if}
        {/each}
      </div>
    </div>
  {/each}
</div>
