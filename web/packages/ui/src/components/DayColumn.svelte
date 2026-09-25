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

<section
  class="board__col {extraClass}"
  class:board__col--empty={count === 0}
  class:board__col--today={day.is_today}
  data-day={day.index}
  aria-labelledby={headId}
  {@attach attach}
>
  <!-- Focusable by script only: the phone rail moves focus to the day it shows. -->
  <h2 class="board__head" id={headId} tabindex="-1">
    <span class="board__dow">{day.dow}</span>
    <span class="board__date">{dayNumber(day.date)}</span>
    {#if day.is_today}<span class="vh">(today)</span>{/if}
    {#if count > 0}<span class="board__count"><span class="vh">,</span> {count}<span class="vh"> runs</span></span>{/if}
    {#if day.is_reset}<span class="board__reset">reset</span>{/if}
  </h2>
  {@render children()}
</section>
