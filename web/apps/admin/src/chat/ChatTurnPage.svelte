<script lang="ts">
  import '@kanade/ui/styles/evidence.scss';
  import type { ChatTurn } from '@kanade/api-types';
  import { Tabs, type TabItem, type Toaster } from '@kanade/ui';
  import { duration } from '../logs/format';
  import { Resource } from '../resource.svelte';
  import { copyText } from '../shared/copy';
  import TextModal from '../shared/TextModal.svelte';
  import { transcriptJson, transcriptMarkdown } from './transcript';
  import LogTime from '../logs/LogTime.svelte';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';

  let { id, timeZone = 'Asia/Kuala_Lumpur', toaster }: { id: string; timeZone?: string; toaster?: Toaster } = $props();
  const turn = $derived(new Resource<ChatTurn>(`/api/admin/chat/${encodeURIComponent(id)}`));
  $effect(() => void turn.load());

  type Tab = 'conversation' | 'tools' | 'model' | 'cards' | 'raw';
  let tab = $state<Tab>('conversation');
  const tabs = $derived<TabItem<Tab>[]>([
    { id: 'conversation', label: 'Conversation' },
    { id: 'tools', label: 'Tool trace', count: turn.data?.tools.length ?? null },
    { id: 'model', label: 'Model trace', count: turn.data?.rounds.length ?? null },
    { id: 'cards', label: 'Produced', count: turn.data?.cards.length ?? null },
    { id: 'raw', label: 'Raw content' },
  ]);

  const WITHHELD = '[message withheld]';
  // One viewer for any long text on the page: a tool's arguments or result, or a transcript that could not be copied.
  let viewer = $state({ open: false, title: '', eyebrow: '', text: '' });
  const show = (title: string, eyebrow: string, text: string) => (viewer = { open: true, title, eyebrow, text });

  let format = $state<'markdown' | 'json'>('markdown');
  async function copyTranscript() {
    const data = turn.data;
    if (!data) return;
    const text = format === 'json' ? transcriptJson(data, { timeZone }) : transcriptMarkdown(data, { timeZone });
    if (await copyText(text)) toaster?.show({ message: `Transcript copied as ${format === 'json' ? 'JSON' : 'Markdown'}.`, tone: 'ok' });
    else show('Transcript', format === 'json' ? 'JSON' : 'Markdown', text);
  }
</script>

<div class="page-head turn-head">
  <div class="turn-head__text">
    <p class="eyebrow"><a href="/chat">Chat</a>{#if turn.data} · <Name kind="member" id={turn.data.member_id || turn.data.member.id} name={turn.data.member.name} />{/if}</p>
    <h1>{#if turn.data}<LogTime at={turn.data.at} {timeZone} />{:else}Interaction{/if}</h1>
    {#if turn.data}<p class="note">{turn.data.model} · <Name kind="channel" id={turn.data.channel_id} name={turn.data.channel} /> · {turn.data.outcome}</p>{/if}
  </div>
  {#if turn.data}
    <!-- For agent debugging: the whole turn as one paste. -->
    <div class="page-head__side transcript">
      <label class="field"
        ><span class="vh">Transcript format</span>
        <select bind:value={format}><option value="markdown">Markdown</option><option value="json">JSON</option></select>
      </label>
      <button class="btn" type="button" onclick={() => void copyTranscript()}>Copy transcript</button>
    </div>
  {/if}
</div>

{#if turn.error}
  <div class="empty" role="alert"><strong>No interaction “{id}”.</strong>{turn.error}</div>
{:else if turn.data}
  {@const data = turn.data}
  <Tabs items={tabs} bind:selected={tab} label="Sections of this interaction">
    {#snippet panel(which)}
      {#if which === 'conversation'}
        <h3 class="pane__section">What they asked</h3>
        <p><Mentions text={data.asked} asked /></p>
        <h3 class="pane__section">What it said</h3>
        <p>{#if data.said}<Mentions text={data.said} />{:else}— nothing was sent —{/if}</p>
      {:else if which === 'tools'}
        {#if data.tools.length}
          <!-- One line per call (area principle): long text opens in a viewer. -->
          <div class="table-wrap">
            <table class="trace">
              <caption class="vh">Tool calls</caption>
              <thead><tr><th scope="col">Tool</th><th scope="col">Arguments</th><th scope="col">Return</th><th scope="col" class="num">Took</th><th scope="col">Outcome</th></tr></thead>
              <tbody>
                {#each data.tools as t, i (i)}
                  <tr>
                    <th scope="row" class="mono trace__one">{t.name}</th>
                    {#each [['Arguments', t.arguments], ['Return', t.result]] as [what, text] (what)}
                      <td class="mono trace__cell">
                        {#if !text}—
                        {:else if text === WITHHELD}<span class="note">{WITHHELD}</span>
                        {:else}<button type="button" class="trace__preview" onclick={() => show(`${t.name}: ${what?.toLowerCase()}`, `Call ${i + 1}`, text!)}
                            ><span class="trace__text">{text}</span><span class="vh">, open the full {what?.toLowerCase()}</span></button
                          >{/if}
                      </td>
                    {/each}
                    <td class="num trace__one">{duration(t.took_ms)}</td>
                    <td><span class="tone tone--{t.outcome === 'ok' ? 'success' : 'danger'}">{t.outcome}</span></td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else}<p class="note">No tools were called.</p>{/if}
      {:else if which === 'model'}
        <ol class="rounds">
          {#each data.rounds as r (r.round)}
            <li>Round {r.round}: {r.requested_tools.length ? `requested ${r.requested_tools.join(', ')}` : 'no tools'} · finished with <span class="mono">{r.finish}</span></li>
          {/each}
        </ol>
      {:else if which === 'cards'}
        {#if data.cards.length}
          <ul>{#each data.cards as c (c.url)}<li><a href={c.url} target="_blank" rel="noopener noreferrer">{c.kind} card</a></li>{/each}</ul>
        {:else}<p class="note">Nothing was posted.</p>{/if}
      {:else}
        <pre>{data.raw}</pre>
      {/if}
    {/snippet}
  </Tabs>
{/if}

<TextModal bind:open={viewer.open} title={viewer.title} eyebrow={viewer.eyebrow} text={viewer.text} {toaster} />

<style>
  .transcript {
    display: flex;
    align-items: flex-end;
    gap: 0.4rem;
  }

  /* Phones: the transcript controls stack beside the heading rather than add a
     row, so the tab window keeps its minimum height. */
  @media (max-width: 640px) {
    .turn-head {
      flex-wrap: nowrap;
      align-items: flex-start;
    }

    .turn-head__text {
      min-width: 0;
    }

    .transcript {
      flex-direction: column;
      align-items: stretch;
      flex-shrink: 0;
    }
  }

  /* One line per call: previews cut with an ellipsis, durations never wrap. */
  .trace {
    table-layout: fixed;
    width: 100%;
    /* Phones scroll the table sideways rather than squeeze the previews to nothing. */
    min-width: 42rem;
  }

  .trace th:nth-child(1) {
    width: 12rem;
  }

  .trace th:nth-child(4) {
    width: 5.5rem;
  }

  .trace th:nth-child(5) {
    width: 6rem;
  }

  .trace__one,
  .trace__cell {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .trace__preview {
    display: block;
    width: 100%;
    min-height: 1.5rem;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .trace__text {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-decoration: underline dotted;
    text-underline-offset: 0.2em;
  }

  .rounds {
    padding-left: 1.25rem;
  }
</style>
