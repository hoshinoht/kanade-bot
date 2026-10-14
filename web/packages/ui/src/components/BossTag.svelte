<script lang="ts">
  import type { Boss } from '@kanade/api-types';
  import Portrait from './Portrait.svelte';
  import { DIFFICULTY_WORDS } from '../format';

  let {
    boss,
    short = false,
    portrait = false,
    level = false,
  }: { boss: Boss; short?: boolean; portrait?: boolean; level?: boolean } = $props();
  const full = $derived(`${DIFFICULTY_WORDS[boss.difficulty]} ${boss.name}`);
</script>

<!-- v4 macros.boss_line: portrait, name (token on compact cards), pill, level. -->
<!-- Tooltip only when the name is shortened to its token; a full tag already reads it. -->
<span class="boss" title={short ? `${full}${boss.level ? ` · Lv. ${boss.level}` : ''}` : undefined}>
  {#if portrait}<Portrait {boss} />{/if}
  {#if short}
    <span class="boss__name" aria-hidden="true">{boss.token}</span><span class="vh">{full}</span>
  {:else}
    <span class="boss__name">{boss.name}</span>
  {/if}
  <span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>
  {#if level && boss.level}<span class="boss__lv">Lv. {boss.level}</span>{/if}
</span>
