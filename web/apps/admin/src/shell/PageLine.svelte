<!--
  The 36 px page line that replaced the page-head card (M3E spec "Frame",
  gate G2): the page's title, its heading (the count or state, an h1), the
  page's own controls, an optional ⓘ for the one-time explanation, then the
  Live chip and the command palette. On a phone the top bar carries the title
  and the Live chip, so the line keeps only the page's own part.
-->
<script lang="ts">
  import { Freshness, Icon } from '@kanade/ui';
  import type { Snippet } from 'svelte';
  import { getChrome } from './chrome';

  let {
    title = '',
    class: extra = '',
    about,
    children,
  }: {
    /** The section's name; omit it when the h1 is the title (detail pages). */
    title?: string;
    class?: string;
    /** One-time explanation, behind an ⓘ disclosure. */
    about?: Snippet;
    children: Snippet;
  } = $props();

  const chrome = getChrome();
</script>

<div class="page-head pageline {extra}" class:pageline--titled={!!title}>
  {#if title && !chrome?.phone}<p class="pageline__title">{title}</p>{/if}
  {@render children()}
  {#if about}
    <details class="pageline__about">
      <summary class="pageline__about-btn" title="About this page"><Icon name="info" label="About this page" /></summary>
      <div class="pageline__about-body">{@render about()}</div>
    </details>
  {/if}
  {#if chrome && !chrome.phone}
    <div class="pageline__end">
      <!-- The zone is printed from 1440 px; below that this tooltip (and the footnote on tall frames) carries it. -->
      <span
        class="mchip mchip--status"
        role="group"
        aria-label="Status"
        title={chrome.timezone ? `Every time here is ${chrome.timezone}` : undefined}
      >
        <Freshness state={chrome.fresh} updated={chrome.updated} />
        {#if chrome.timezone}<span class="masthead__tz">{chrome.timezone}</span>{/if}
      </span>
      <button
        type="button"
        class="mchip pageline__commands"
        onclick={() => chrome.palette()}
        aria-keyshortcuts="Control+K Meta+K"
        aria-label="Commands"
        title="Commands (Ctrl K)"
      >
        <Icon name="search" /><kbd class="kbd">Ctrl K</kbd>
      </button>
    </div>
  {/if}
</div>
