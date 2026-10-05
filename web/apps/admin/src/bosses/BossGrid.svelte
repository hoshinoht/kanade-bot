<script lang="ts">
  import '@kanade/ui/styles/boss-grid.scss';
  import type { BossRow } from '@kanade/api-types';
  import { Portrait, RowContent } from '@kanade/ui';

  let {
    rows,
    selected = $bindable([]),
    readonly = false,
    active = '',
    infoOnly = [],
  }: {
    rows: BossRow[];
    selected?: string[];
    readonly?: boolean;
    active?: string;
    /** The active boss's knowledge-only difficulties (Champion, Destiny), shown after its catalog ones. */
    infoOnly?: string[];
  } = $props();

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

  const TICK: Record<string, string> = { e: 'EASY', n: 'NORM', h: 'HARD', c: 'CHAOS', x: 'EXT' };

  /**
   * A pointer anywhere on a catalog row follows its name link (as Fixed rows
   * do, without a `::after` overlay that WebKit lets escape); the keyboard
   * keeps using the link itself.
   */
  function forwardRowClicks(grid: HTMLElement) {
    const onclick = (event: MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (!target || target.closest('a, button, input, label')) return;
      target.closest('.bossrow')?.querySelector<HTMLAnchorElement>('a.bossrow__name')?.click();
    };
    grid.addEventListener('click', onclick);
    return () => grid.removeEventListener('click', onclick);
  }
</script>

<!-- v4 macros.boss_grid: each difficulty a pill; checked = filled with a tick, so state is not colour alone. -->
<div class="grid-bosses" role={readonly ? 'list' : 'group'} aria-label="Bosses" {@attach (grid) => (readonly ? forwardRowClicks(grid) : undefined)}>
  {#each rows as row (row.key)}
    {@const on = readonly ? row.difficulties.some((d) => d.in_use) : row.difficulties.some((d) => selected.includes(d.token))}
    {@const remaining = row.difficulties.filter((option) => !option.in_use).length}
    {#if readonly}
      <div class="bossrow expandable-row" data-fid="boss-row" class:bossrow--on={on} class:bossrow--active={row.key === active} role="listitem">
        <div class="bossrow__id"><Portrait boss={asBoss(row)} size="md" /><span><a class="bossrow__name" href="/bosses/{row.key}/knowledge" aria-current={row.key === active ? 'true' : undefined}>{row.name}</a><span class="bossrow__lv">Lv. {row.level}</span></span></div>
        <RowContent expanded={row.key === active}>
          {#snippet compact()}<span class="bossrow__pills" role="group" aria-label="{row.name} difficulties">
          {#each row.difficulties.filter((option) => option.in_use) as option (option.token)}
            <span class="boss-tick boss-tick--{option.letter}">{TICK[option.letter]}<span class="vh"> (has a weekly timing)</span></span>
          {/each}
          {#if remaining}<span class="boss-tick boss-tick--more">+{remaining}<span class="vh"> untracked difficulties</span></span>{/if}
          </span>{/snippet}
          <span class="bossrow__difficulties" role="group" aria-label="{row.name} difficulties">{#each row.difficulties as option (option.token)}<span><span class="boss-tick boss-tick--{option.letter}">{option.name.toUpperCase()}</span>{#if option.in_use}<span class="bossrow__tracked"> ✓ weekly timing</span>{/if}</span>{/each}{#if row.key === active}{#each infoOnly as name (name)}<span><span class="boss-tick boss-tick--{name.toLowerCase()}">{name.toUpperCase()}</span><span class="bossrow__tracked"> info only</span></span>{/each}{/if}</span>
        </RowContent>
      </div>
    {:else}
      <div class="bossrow" data-fid="fixed-boss-row" class:bossrow--on={on}>
        <div class="bossrow__id"><Portrait boss={asBoss(row)} size="md" /><span><span class="bossrow__name">{row.name}</span><span class="bossrow__lv">Lv. {row.level}</span></span></div>
        <div class="bossrow__pills" role="group" aria-label="{row.name} difficulties">
          {#each row.difficulties as option (option.token)}
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
          {/each}
        </div>
      </div>
    {/if}
  {/each}
</div>
