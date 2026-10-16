<!--
  The guide's messages as Discord shows them (boards G_Editor, G_PhonePreview,
  G_Unknown): Reminders' CardPreview drawn once per message, numbered in the
  gutter, the bot's name over the first. Text keeps Discord Markdown's
  headings, `-#` subtext, list items and bold; mentions show as names
  (nobody is pinged); a spacer is an empty message; boss embeds carry their
  colour bar, portrait and, when chosen, the large entry image. Read only.
-->
<script lang="ts">
  import type { PreviewSlot } from '@kanade/api-types';
  import { StatusChip } from '@kanade/ui';
  import type { Attachment } from 'svelte/attachments';
  import Mentions from '../names/Mentions.svelte';
  import { lead } from '../reminders/discord';
  import { SPACER, slotLines, type SlotLine } from './guides';

  let {
    slots,
    bot,
    at,
    here = null,
    tags = {},
  }: {
    slots: PreviewSlot[];
    bot: string;
    /** The first message's time ("21:10"). */
    at: string;
    /** The message the open block renders to (0-based), shaded. */
    here?: number | null;
    /** A chip under a message (0-based position): "unsaved · new message", "unconfirmed". */
    tags?: Record<number, { tone: 'neutral' | 'risk'; text: string }>;
  } = $props();

  // The embed colour goes in through CSSOM: a style attribute is blocked by style-src 'self'.
  const bar =
    (color: string): Attachment<HTMLElement> =>
    (node) => {
      node.style.setProperty('--card-bar', color);
    };

  // A `token` line (render.rs wraps the difficulty tokens in backticks) reads as subtext.
  const descLines = (text: string): SlotLine[] =>
    slotLines(text).map((line) => {
      const only = line.runs.length === 1 ? line.runs[0]!.text : '';
      return /^`[^`]*`$/.test(only) ? { kind: 'sub', runs: [{ bold: false, text: only.slice(1, -1) }] } : line;
    });
</script>

{#snippet lines(value: SlotLine[])}
  {#each value as line, i (i)}
    <span class={['dcard__line', line.kind === 'sub' && 'dcard__line--sub', line.kind !== 'sub' && line.kind !== 'text' && `gline--${line.kind}`]}
      >{#each line.runs as run, j (j)}{@const [space, rest] = lead(run.text)}{#if run.bold}<strong>{space}<Mentions text={rest} plain /></strong>{:else}{space}<Mentions text={rest} plain />{/if}{/each}</span
    >
  {/each}
{/snippet}

<ol class="dcard gpreview" aria-label="Messages as Discord shows them" data-fid="guide-preview-list">
  {#each slots as slot, n (slot.position)}
    {@const tag = tags[slot.position]}
    <li class={['gslot', slot.position === here && 'gslot--here', tag?.tone === 'risk' && 'gslot--risk']} data-fid="guide-slot" data-position={slot.position}>
      <span class="gslot__gutter" aria-hidden="true">{#if n === 0}<span class="dcard__avatar"></span>{/if}<span class="gslot__no mono">{slot.position + 1}</span></span>
      <div class="dcard__message">
        <span class="vh">Message {slot.position + 1}. </span>
        {#if n === 0}<p class="dcard__author"><strong>{bot}</strong> <span class="dcard__app">APP</span> <span class="dcard__time">{at}</span></p>{/if}
        {#if slot.content === SPACER}
          <p class="dcard__content"><span class="dcard__line gline--blank" aria-label="An empty message">{SPACER}</span></p>
        {:else if slot.content}
          <p class="dcard__content">{@render lines(slotLines(slot.content))}</p>
        {/if}
        {#each slot.embeds as embed, e (e)}
          <div class="dcard__embed" {@attach bar(embed.color)}>
            <div class="dcard__body">
              <p class="dcard__title">{@render lines(slotLines(embed.title))}</p>
              {#if embed.description}<p class="dcard__description">{@render lines(descLines(embed.description))}</p>{/if}
            </div>
            {#if embed.thumbnail}<img class="dcard__thumb" src={embed.thumbnail} alt="" width="80" height="80" />{/if}
            {#if embed.image}<img class="dcard__image" src={embed.image} alt="" />{/if}
          </div>
        {/each}
        {#if tag}<span class="gslot__tag"><StatusChip tone={tag.tone}>{tag.text}</StatusChip></span>{/if}
      </div>
    </li>
  {/each}
</ol>
