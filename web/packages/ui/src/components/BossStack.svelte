<script lang="ts">
  import type { Boss } from '@kanade/api-types';
  import type { Snippet } from 'svelte';
  import { DIFFICULTY_WORDS } from '../format';
  import Portrait from './Portrait.svelte';

  let { bosses, suffix }: { bosses: Boss[]; suffix?: Snippet } = $props();
</script>

<span class="boss-stack" title={bosses.map((boss) => `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}`).join(' + ')}>
  <span class="boss-stack__portraits" aria-hidden="true">{#each bosses as boss (boss.token)}<Portrait {boss} />{/each}</span>
  <span class="boss-stack__names">{bosses.map((boss) => boss.name).join(' + ')}{@render suffix?.()}</span>
  <span class="boss-stack__pills">{#each bosses as boss (boss.token)}<span class="pill pill--{boss.difficulty}"><span class="vh">{boss.name}: </span>{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>{/each}</span>
</span>
