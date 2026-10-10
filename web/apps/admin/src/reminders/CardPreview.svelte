<!--
  A reminder card drawn as Discord shows it (O9): the bot's name and avatar,
  the message text (mentions named, never pinged; `-#` lines as subtext),
  then each embed with its colour bar, title, description, fields (inline
  ones side by side), its boss portrait top right, the day-of entry art below
  and the footer. Discord timestamps are drawn in the guild zone, relative
  ones from the server's clock. Read only.
-->
<script lang="ts">
  import type { CardPreview, EmbedPreview } from '@kanade/api-types';
  import type { Attachment } from 'svelte/attachments';
  import Mentions from '../names/Mentions.svelte';
  import { cardText, lead } from './discord';

  let {
    card,
    bot,
    avatar,
    at,
    now,
    zone,
  }: { card: CardPreview; bot: string; avatar: string | null; at: string; now: number; zone?: string } = $props();

  const embeds = $derived<EmbedPreview[]>([card, ...card.more_embeds]);

  // The embed colour goes in through CSSOM: a style attribute is blocked by style-src 'self'.
  const bar =
    (color: string): Attachment<HTMLElement> =>
    (node) => {
      node.style.setProperty('--card-bar', color);
    };
</script>

{#snippet text(value: string)}
  {#each cardText(value, now, zone) as line, i (i)}
    <span class={['dcard__line', line.sub && 'dcard__line--sub']}
      >{#each line.runs as run, j (j)}{@const [space, rest] = lead(run.text)}{#if run.bold}<strong>{space}<Mentions text={rest} plain /></strong>{:else}{space}<Mentions text={rest} plain />{/if}{/each}</span
    >
  {/each}
{/snippet}

<article class="dcard" aria-label="The card as posted in Discord" data-fid="reminder-card">
  {#if avatar}<img class="dcard__avatar" src={avatar} alt="" width="40" height="40" />{:else}<span class="dcard__avatar" aria-hidden="true"></span>{/if}
  <div class="dcard__message">
    <p class="dcard__author"><strong>{bot}</strong> <span class="dcard__app">APP</span> <span class="dcard__time">{at}</span></p>
    <p class="dcard__content">{@render text(card.content)}</p>
    {#each embeds as embed, e (e)}
      <div class="dcard__embed" {@attach bar(embed.color)} data-fid="reminder-embed">
        <div class="dcard__body">
          {#if embed.title}<p class="dcard__title">{@render text(embed.title)}</p>{/if}
          {#if embed.description}<p class="dcard__description">{@render text(embed.description)}</p>{/if}
          {#if embed.fields.length}
            <dl class="dcard__fields">
              {#each embed.fields as field, i (i)}
                <div class={['dcard__field', field.inline && 'dcard__field--inline']}>
                  <dt>{@render text(field.name)}</dt>
                  <dd>{@render text(field.value)}</dd>
                </div>
              {/each}
            </dl>
          {/if}
        </div>
        {#if embed.thumbnail}<img class="dcard__thumb" src={embed.thumbnail} alt="" width="80" height="80" />{/if}
        {#if embed.image}<img class="dcard__image" src={embed.image} alt="" />{/if}
        {#if embed.footer}<p class="dcard__footer">{embed.footer}</p>{/if}
      </div>
    {/each}
  </div>
</article>
