<!--
  One closed Inbox item, read-only: what it was, how it ended (outcome, who,
  when, why), who asked, the messages it cited, and links to the card, the
  History record an approval wrote and the log entry that staged it. A
  closed item never applies again, so there are no decision controls.
-->
<script lang="ts">
  import type { PastItem } from '@kanade/api-types';
  import { Avatar } from '@kanade/ui';
  import { localAt } from '../history/describe';
  import { directory } from '../names/directory.svelte';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';
  import { memberAvatar } from '../shared/avatar';
  import { discordLink } from '../shared/discordLink.svelte';
  import { SOURCE_LABEL } from './flags';
  import OutcomeChip from './OutcomeChip.svelte';
  import { outcomeSentence, sourceLink } from './past';

  let { item, timeZone }: { item: PastItem; timeZone: string } = $props();
  const uid = $props.id();
  const source = $derived(sourceLink(item));
  const hasLinks = $derived(Boolean(item.card_url || item.history_seq !== null || source));
</script>

<article class="past" data-fid="past-detail" aria-labelledby="{uid}-title">
  <header class="past__head">
    <p class="cap past__kind">{item.kind_label}{item.tab === 'extractor' ? ' · Kanade’s proposal' : ''}</p>
    <h2 class="proposal__title past__title" id="{uid}-title">{item.summary}</h2>
    <p class="proposal__meta">
      <span class="chip proposal__source">{SOURCE_LABEL[item.source]}</span>
      <span class="proposal__fact mono">#{item.short_id}</span>
      {#if item.channel}<span class="proposal__fact">{item.channel}</span>{/if}
      <span class="proposal__fact">raised <time datetime={item.created_at}>{localAt(item.created_at, timeZone)}</time></span>
    </p>
  </header>

  <section class="past__card past__verdict past__verdict--{item.outcome}" data-fid="past-outcome" aria-labelledby="{uid}-outcome">
    <h3 class="cap proposal__cap" id="{uid}-outcome">Outcome</h3>
    <p class="past__sentence"><OutcomeChip outcome={item.outcome} /> <span>{outcomeSentence(item, timeZone)}</span></p>
    {#if item.reason}
      <p class="past__reason"><span class="cap">Reason</span> “{item.reason}”</p>
    {/if}
    {#if item.requester}
      <p class="past__asked">Asked by <Name kind="member" id={item.requester.id} name={item.requester.name} /></p>
    {/if}
  </section>

  {#if item.evidence.length}
    <section class="proposal__thread past__thread" aria-labelledby="{uid}-thread">
      <div class="proposal__threadhead">
        <h3 class="proposal__threadtitle" id="{uid}-thread">Evidence <span class="mono">· {item.evidence.length} message{item.evidence.length === 1 ? '' : 's'}</span></h3>
      </div>
      <ul class="evidence" aria-label="Evidence">
        {#each item.evidence as line (line.id)}
          {@const who = line.author_id ? null : directory.label('member', '', line.author)}
          <li class="msg msg--used" class:msg--gone={line.missing}>
            <Avatar class="msg__av" src={line.author_id ? memberAvatar(line.author_id) : null} name={who ?? line.author} />
            <div class="msg__body">
              <p class="msg__line">
                <span class="msg__who">{#if line.author_id}<Name kind="member" id={line.author_id} name={line.author} />{:else}{who}{/if}</span>
                {#if line.url}<a class="msg__at" {...discordLink(line.url)}>{line.at}<span class="vh"> (open in Discord)</span></a>
                {:else}<span class="msg__at">{line.at}</span>{/if}
              </p>
              {#if line.missing}<p class="msg__text">Message no longer cached{#if line.url}; the link still opens it in Discord{/if}.</p>
              {:else}<p class="msg__text"><Mentions text={line.content ?? ''} /></p>{/if}
            </div>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if hasLinks}
    <nav class="past__card past__links" aria-label="Related records">
      {#if item.history_seq !== null}<a href="/history">History record #{item.history_seq}</a>{/if}
      {#if source}<a href={source.href}>{source.label}</a>{/if}
      {#if item.card_url}<a {...discordLink(item.card_url)}>See the card<span class="vh"> (opens Discord)</span></a>{/if}
    </nav>
  {/if}
</article>
