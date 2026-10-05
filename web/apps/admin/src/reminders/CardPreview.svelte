<!--
  A reminder card drawn as Discord shows it (O9): the bot's name and avatar,
  the message text (mentions named, never pinged), then one embed with its
  colour bar, description, fields, the lead boss's portrait top right, the
  day-of entry art below and the footer. Read only.
-->
<script lang="ts">
  import type { CardPreview } from '@kanade/api-types';
  import type { Attachment } from 'svelte/attachments';
  import Mentions from '../names/Mentions.svelte';
  import { cardLines, lead } from './discord';

  let { card, bot, avatar, at }: { card: CardPreview; bot: string; avatar: string | null; at: string } = $props();

  // The embed colour goes in through CSSOM: a style attribute is blocked by style-src 'self'.
  const bar: Attachment<HTMLElement> = (node) => {
    node.style.setProperty('--card-bar', card.color);
  };
</script>

{#snippet text(value: string)}
  {#each cardLines(value) as line, i (i)}
    <span class="dcard__line"
      >{#each line as run, j (j)}{@const [space, rest] = lead(run.text)}{#if run.bold}<strong>{space}<Mentions text={rest} plain /></strong>{:else}{space}<Mentions text={rest} plain />{/if}{/each}</span
    >
  {/each}
{/snippet}

<article class="dcard" aria-label="The card as posted in Discord" data-fid="reminder-card">
  {#if avatar}<img class="dcard__avatar" src={avatar} alt="" width="40" height="40" />{:else}<span class="dcard__avatar" aria-hidden="true"></span>{/if}
  <div class="dcard__message">
    <p class="dcard__author"><strong>{bot}</strong> <span class="dcard__app">APP</span> <span class="dcard__time">{at}</span></p>
    <p class="dcard__content">{@render text(card.content)}</p>
    <div class="dcard__embed" {@attach bar} data-fid="reminder-embed">
      <div class="dcard__body">
        {#if card.description}<p class="dcard__description">{@render text(card.description)}</p>{/if}
        {#if card.fields.length}
          <dl class="dcard__fields">
            {#each card.fields as field, i (i)}
              <div class="dcard__field">
                <dt>{@render text(field.name)}</dt>
                <dd>{@render text(field.value)}</dd>
              </div>
            {/each}
          </dl>
        {/if}
      </div>
      {#if card.thumbnail}<img class="dcard__thumb" src={card.thumbnail} alt="" width="80" height="80" />{/if}
      {#if card.image}<img class="dcard__image" src={card.image} alt="" />{/if}
      {#if card.footer}<p class="dcard__footer">{card.footer}</p>{/if}
    </div>
  </div>
</article>
