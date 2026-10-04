<!--
  A pane's empty or failed state (M3E boards B_Empty, B_States): a cookie
  glyph, a heading, one line of explanation and the way on, centred.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from './Icon.svelte';

  let {
    tone = 'empty',
    icon,
    title,
    level = 2,
    children,
    actions,
  }: {
    /** `empty`: 112 px glyph on --select; `error`: 72 px on a risk wash. */
    tone?: 'empty' | 'error';
    icon: IconName;
    title: string;
    level?: 2 | 3;
    children: Snippet;
    actions?: Snippet;
  } = $props();
</script>

<div class="state-note state-note--{tone}" data-fid="state-note">
  <span class="state-note__glyph" data-fid="state-glyph" aria-hidden="true"><Icon name={icon} /></span>
  <svelte:element this={`h${level}`} class="state-note__title" data-fid="state-title">{title}</svelte:element>
  <p class="state-note__text" data-fid="state-text">{@render children()}</p>
  {#if actions}<div class="state-note__actions" data-fid="state-actions">{@render actions()}</div>{/if}
</div>
