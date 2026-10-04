<!--
  One chat turn in the Chat window's detail pane (B_Chat, B_ChatTrace): who
  and when, model · channel · outcome · took, Copy transcript as the key
  action, then pill tabs whose panel is the one part that scrolls.
-->
<script lang="ts">
  import type { ChatTurn } from '@kanade/api-types';
  import { LoadingState, Select, type Toaster } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import LogTime from '../logs/LogTime.svelte';
  import TokenUsage from '../logs/TokenUsage.svelte';
  import { duration } from '../logs/format';
  import { OUTCOME_LABEL } from '../logs/filters';
  import Name from '../names/Name.svelte';
  import { Resource } from '../resource.svelte';
  import { copyText } from '../shared/copy';
  import TextModal from '../shared/TextModal.svelte';
  import Conversation from './Conversation.svelte';
  import ModelTrace from './ModelTrace.svelte';
  import ToolTrace from './ToolTrace.svelte';
  import { transcriptJson, transcriptMarkdown } from './transcript';

  let { id, timeZone, toaster }: { id: string; timeZone: string; toaster?: Toaster } = $props();
  const turn = $derived(new Resource<ChatTurn>(`/api/admin/chat/${encodeURIComponent(id)}`));
  $effect(() => void turn.load());
  const data = $derived(turn.data?.id === id ? turn.data : null);

  type Tab = 'conversation' | 'tools' | 'model' | 'cards' | 'raw';
  // The tab stays put while another turn opens: traces compare side by side.
  let tab = $state<Tab>('conversation');
  const tabs = $derived<{ id: Tab; label: string; count?: number }[]>([
    { id: 'conversation', label: 'Conversation' },
    { id: 'tools', label: 'Tool trace', count: data?.tools.length },
    { id: 'model', label: 'Model trace', count: data?.rounds.length },
    { id: 'cards', label: 'Produced', count: data?.cards.length },
    { id: 'raw', label: 'Raw' },
  ]);
  const uid = $props.id();
  const tabEls: Partial<Record<Tab, HTMLButtonElement>> = {};

  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: tabs.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    tab = tabs[(target + tabs.length) % tabs.length]!.id;
    tabEls[tab]?.focus();
  }

  // One viewer for any long text on the page: a tool's arguments or result, or a transcript that could not be copied.
  let viewer = $state({ open: false, title: '', eyebrow: '', text: '' });
  const show = (title: string, eyebrow: string, text: string) => (viewer = { open: true, title, eyebrow, text });

  let format = $state<'markdown' | 'json'>('markdown');
  async function copyTranscript() {
    if (!data) return;
    const text = format === 'json' ? transcriptJson(data, { timeZone }) : transcriptMarkdown(data, { timeZone });
    if (await copyText(text)) toaster?.show({ message: `Transcript copied as ${format === 'json' ? 'JSON' : 'Markdown'}.`, tone: 'ok' });
    else show('Transcript', format === 'json' ? 'JSON' : 'Markdown', text);
  }
</script>

{#if turn.error}
  <div class="empty chat-turn__missing" role="alert"><strong>No interaction “{id}”.</strong>{turn.error}</div>
{:else if !data}
  <LoadingState text="Loading the interaction…" />
{:else}
  <div class="chat-turn__head" data-fid="chat-head">
    <div class="chat-turn__title">
      <p class="cap chat-turn__eyebrow">Chat · <Name kind="member" id={data.member_id || data.member.id} name={data.member.name} /></p>
      <h2 class="chat-turn__when"><LogTime at={data.at} {timeZone} /></h2>
      <p class="chat-turn__meta">
        <span class="mono">{data.model}</span> · <Name kind="channel" id={data.channel_id} name={data.channel} /> · {OUTCOME_LABEL[data.outcome] ?? data.outcome}
        · <span class="mono">{duration(data.latency_ms)}</span> ·
        <span class="mono"><TokenUsage prompt={data.prompt_tokens} completion={data.completion_tokens} reasoning={data.reasoning_tokens} /> tokens</span>
      </p>
    </div>
    <!-- For agent debugging: the whole turn as one paste. -->
    <div class="chat-turn__format">
      <Select
        label="Transcript format"
        bind:value={() => format, (v) => (format = v as typeof format)}
        options={[
          { value: 'markdown', label: 'Markdown' },
          { value: 'json', label: 'JSON' },
        ]}
      />
    </div>
    <button class="btn btn--primary chat-turn__copy" data-fid="chat-copy" type="button" onclick={() => void copyTranscript()}>Copy transcript</button>
  </div>
  <div class="chat-turn__tabs" role="tablist" aria-label="Sections of this interaction" data-fid="chat-tabs">
    {#each tabs as t, index (t.id)}
      <button
        type="button"
        role="tab"
        class="ptab"
        id="{uid}-tab-{t.id}"
        aria-selected={tab === t.id}
        aria-controls="{uid}-panel"
        tabindex={tab === t.id ? 0 : -1}
        bind:this={tabEls[t.id]}
        onclick={() => (tab = t.id)}
        onkeydown={(event) => tabKey(event, index)}
        >{t.label}{#if t.count !== undefined}<span class="ptab__count mono">{t.count}</span>{/if}</button
      >
    {/each}
  </div>
  <div class="chat-turn__panel" role="tabpanel" id="{uid}-panel" aria-labelledby="{uid}-tab-{tab}" tabindex="0" data-fid="chat-panel">
    {#if tab === 'conversation'}
      <Conversation turn={data} />
    {:else if tab === 'tools'}
      <ToolTrace turn={data} {show} />
    {:else if tab === 'model'}
      <ModelTrace turn={data} {show} />
    {:else if tab === 'cards'}
      {#if data.cards.length}
        <ul class="chat-produced" aria-label="What this turn produced">
          {#each data.cards as c (c.url)}<li><a href={c.url} target="_blank" rel="noopener noreferrer">{c.kind} card</a></li>{/each}
        </ul>
      {:else}<p class="note">Nothing produced.</p>{/if}
    {:else}
      <pre class="chat-raw">{data.raw}</pre>
    {/if}
  </div>
{/if}

<TextModal bind:open={viewer.open} title={viewer.title} eyebrow={viewer.eyebrow} text={viewer.text} {toaster} />
