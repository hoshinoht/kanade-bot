<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Attachment } from 'svelte/attachments';
  import type { WeekDay } from '@kanade/api-types';
  import { dayNumber } from '../format';

  let {
    day,
    count,
    extraClass = '',
    attach,
    children,
  }: { day: WeekDay; count: number; extraClass?: string; attach?: Attachment<HTMLElement>; children: Snippet } =
    $props();
  const headId = $props.id();
</script>

{#snippet head(fid: boolean)}
  <!-- Focusable by script only: the phone rail moves focus to the day it shows. -->
  {#if fid}
    <h2 class="board__head" id={headId} tabindex="-1" data-fid="week-day-head">{@render label()}</h2>
  {:else}
    <h2 class="board__head" id={headId} tabindex="-1">{@render label()}</h2>
  {/if}
{/snippet}

{#snippet label()}
  <span class="board__dow">{day.dow}</span>
  <span class="board__date">{dayNumber(day.date)}</span>
  {#if day.is_today}<span class="vh">(today)</span>{/if}
  {#if count > 0}<span class="board__count" class:board__count--one={count === 1}><span class="vh">,</span> {count}<span class="vh"> runs</span></span>{/if}
  {#if day.is_reset}<span class="board__reset">reset</span>{/if}
{/snippet}

<!-- The fidelity tags (stripped from production builds) tell an empty day's spine from a full column. -->
{#if count === 0}
  <section
    class="board__col board__col--empty {extraClass}"
    class:board__col--today={day.is_today}
    data-day={day.index}
    aria-labelledby={headId}
    data-fid="week-empty"
    {@attach attach}
  >
    {@render head(false)}
    {@render children()}
  </section>
{:else}
  <section class="board__col {extraClass}" class:board__col--today={day.is_today} data-day={day.index} aria-labelledby={headId} data-fid="week-day" {@attach attach}>
    {@render head(true)}
    {@render children()}
  </section>
{/if}
