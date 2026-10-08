<!--
  One extraction call in the Extractions detail pane (B_Extract,
  B_ExtractPrompt): an eyebrow with the id, the time as the heading, model ·
  channel · latency, Copy transcript, and pill tabs (Changes / Chat read /
  Prompt / Raw) on the right. Changes and Chat read sit beside the outcome
  card; Prompt and Raw fill the pane with the code viewer. The chosen tab
  and transcript format stay while the selection moves.
-->
<script lang="ts">
  import type { Extraction } from '@kanade/api-types';
  import { initial, LoadingState, type Toaster } from '@kanade/ui';
  import CopyTranscript from '../logs/CopyTranscript.svelte';
  import { logTime } from '../logs/format';
  import LogTime from '../logs/LogTime.svelte';
  import Reasoning from '../logs/Reasoning.svelte';
  import type { TranscriptFormat } from '../logs/transcript';
  import { directory } from '../names/directory.svelte';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';
  import { Resource } from '../resource.svelte';
  import CallOutcome from './CallOutcome.svelte';
  import { clock, pretty, timeRange } from './code';
  import CodeViewer from './CodeViewer.svelte';
  import { extractionJson, extractionMarkdown } from './transcript';

  let {
    id,
    timeZone,
    toaster,
    canReread,
    rescanOff = null,
    onreread,
  }: {
    id: string;
    timeZone: string;
    toaster?: Toaster;
    canReread: (channel: string) => boolean;
    /** Why re-reading is off right now (the server's sentence). */
    rescanOff?: string | null;
    onreread: (channel: string) => void;
  } = $props();
  const uid = $props.id();
  const call = $derived(new Resource<Extraction>(`/api/admin/extractions/${encodeURIComponent(id)}`));
  $effect(() => void call.load());

  type Tab = 'changes' | 'chat' | 'prompt' | 'raw';
  let tab = $state<Tab>('chat');
  const tabs = $derived<{ id: Tab; label: string; count: number | null }[]>([
    { id: 'changes', label: 'Changes', count: call.data?.amendments.length ?? null },
    { id: 'chat', label: 'Chat read', count: call.data?.messages.length ?? null },
    { id: 'prompt', label: 'Prompt', count: null },
    { id: 'raw', label: 'Raw', count: null },
  ]);
  const tabEls: Partial<Record<Tab, HTMLButtonElement>> = {};

  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: tabs.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = tabs[(target + tabs.length) % tabs.length]!.id;
    tab = next;
    tabEls[next]?.focus();
  }

  let format = $state<TranscriptFormat>('markdown');
  let copier = $state<CopyTranscript>();

  let root = $state<HTMLElement>();
  /** Phones: an opened call takes focus, as a page would. */
  export function focus() {
    root?.focus({ preventScroll: true });
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<article class="extract-detail" data-fid="extract-detail" aria-labelledby="{uid}-title" tabindex="-1" bind:this={root}>
  {#if call.error}
    <div class="empty" role="alert"><strong>No call “{id}”.</strong>{call.error}</div>
  {:else if call.data}
    {@const data = call.data}
    <header class="extract-detail__head" data-fid="extract-head">
      <div class="extract-detail__title">
        <span class="cap">Extraction · #{data.short_id}</span>
        <h2 class="extract-detail__when" id="{uid}-title"><LogTime at={data.at} {timeZone} /></h2>
        <p class="extract-detail__meta">
          <span class="mono">{data.model}</span> ·
          {#if data.channel_id}<Name kind="channel" id={data.channel_id} name={data.channel} />{:else}no channel{/if} ·
          <span class="mono">{data.latency_ms !== null ? `${data.latency_ms.toLocaleString('en')} ms` : 'latency not recorded'}</span>
        </p>
        <!-- For agent debugging: the whole call as one paste. -->
        <div class="transcript-actions extract-detail__copy">
          <CopyTranscript bind:this={copier} bind:format build={(as) => (as === 'json' ? extractionJson(data, { timeZone }) : extractionMarkdown(data, { timeZone }))} {toaster} />
          <button class="btn btn--primary transcript-copy" type="button" onclick={() => void copier?.copy()}>Copy transcript</button>
        </div>
      </div>
      <div class="extract-detail__tabs" role="tablist" aria-label="Sections of this call" data-fid="extract-tabs">
        {#each tabs as t, index (t.id)}
          <button
            class="extract-tab"
            type="button"
            role="tab"
            id="{uid}-tab-{t.id}"
            aria-selected={tab === t.id}
            aria-controls="{uid}-panel"
            tabindex={tab === t.id ? 0 : -1}
            bind:this={tabEls[t.id]}
            onclick={() => (tab = t.id)}
            onkeydown={(event) => tabKey(event, index)}
            >{t.label}{#if t.count !== null}<span class="extract-tab__count mono">{t.count}</span>{/if}</button
          >
        {/each}
      </div>
    </header>
    {#if tab === 'prompt'}
      <CodeViewer title="Prompt as sent" text={data.prompt} find="find in prompt" panel="{uid}-panel" tab="{uid}-tab-prompt" />
    {:else if tab === 'raw'}
      {#if data.raw_response === 'null'}
        <div class="extract-detail__none" id="{uid}-panel" role="tabpanel" aria-labelledby="{uid}-tab-raw">No response (the call failed).</div>
      {:else}
        <CodeViewer title="Raw response" text={pretty(data.raw_response)} find="find in response" panel="{uid}-panel" tab="{uid}-tab-raw" />
      {/if}
    {:else}
      <div class="extract-detail__body" data-fid="extract-body" id="{uid}-panel" role="tabpanel" aria-labelledby="{uid}-tab-{tab}">
        {#if tab === 'chat'}
          <section class="extract-thread" data-fid="extract-thread" aria-labelledby="{uid}-thread">
            <div class="extract-thread__bar" data-fid="extract-thread-bar">
              <h3 class="extract-thread__title" id="{uid}-thread">{data.messages.length} message{data.messages.length === 1 ? '' : 's'}</h3>
              <span class="extract-thread__range mono">{timeRange(data.messages.map((m) => m.at), timeZone)}</span>
            </div>
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <ol class="extract-thread__body" data-fid="extract-thread-body" tabindex="0" aria-label="Messages read">
              {#each data.messages as m (m.id)}
                {@const who = m.author_id ? directory.label('member', m.author_id, m.author) : directory.label('member', '', m.author)}
                <li class="extract-msg" data-fid="extract-msg">
                  <span class="extract-msg__av" aria-hidden="true">{initial(who)}</span>
                  <span class="extract-msg__line">
                    <span class="extract-msg__who">{#if m.author_id}<Name kind="member" id={m.author_id} name={m.author} />{:else}{who}{/if}</span>
                    <time class="extract-msg__at" datetime={m.at} title={logTime(m.at, timeZone).title}>{clock(m.at, timeZone)}</time>
                    <span class="extract-msg__text"><Mentions text={m.content} /></span>
                  </span>
                </li>
              {:else}
                <li class="extract-msg extract-msg--none">No messages were stored for this call.</li>
              {/each}
            </ol>
          </section>
        {:else}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
          <section class="extract-changes" aria-label="Changes the model proposed" tabindex="0">
            {#if data.error}<p class="flash flash--error">{data.error}</p>{/if}
            <Reasoning text={data.reasoning_content} tokens={data.reasoning_tokens} />
            {#if data.amendments.length}
              <table>
                <caption class="vh">Changes the model proposed</caption>
                <thead><tr><th scope="col">Kind</th><th scope="col">Bosses</th><th scope="col">When</th><th scope="col" class="num">Confidence</th><th scope="col">Status</th></tr></thead>
                <tbody>
                  {#each data.amendments as a, i (i)}
                    <tr><th scope="row">{a.kind}</th><td class="mono">{a.bosses}</td><td class="mono">{a.when}</td><td class="num">{a.confidence.toFixed(2)}</td>
                      <td><span class="tone tone--{a.status === 'confirmed' ? 'success' : 'warning'}">{a.status}</span></td></tr>
                  {/each}
                </tbody>
              </table>
            {:else if !data.error}<p class="note">The model found nothing to change.</p>{/if}
          </section>
        {/if}
        <CallOutcome call={data} canReread={canReread(data.channel_id)} {rescanOff} onreread={() => onreread(data.channel_id)} />
      </div>
    {/if}
  {:else}
    <LoadingState text="Loading the call…" />
  {/if}
</article>
