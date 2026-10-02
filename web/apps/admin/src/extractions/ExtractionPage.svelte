<script lang="ts">
  import Reasoning from '../logs/Reasoning.svelte';
  import PageLine from '../shell/PageLine.svelte';
  import { directory } from '../names/directory.svelte';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';
  import '@kanade/ui/styles/evidence.scss';
  import type { Extraction } from '@kanade/api-types';
  import { Tabs, type TabItem } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import LogTime from '../logs/LogTime.svelte';

  let { id, timeZone = 'Asia/Kuala_Lumpur' }: { id: string; timeZone?: string } = $props();
  const call = $derived(new Resource<Extraction>(`/api/admin/extractions/${encodeURIComponent(id)}`));
  $effect(() => void call.load());

  type Tab = 'changes' | 'chat' | 'prompt' | 'raw';
  let tab = $state<Tab>('changes');
  const tabs = $derived<TabItem<Tab>[]>([
    { id: 'changes', label: 'Changes', count: call.data?.amendments.length ?? null },
    { id: 'chat', label: 'Chat read', count: call.data?.messages.length ?? null },
    { id: 'prompt', label: 'Prompt as sent' },
    { id: 'raw', label: 'Raw response' },
  ]);
</script>

<PageLine>
  <p class="eyebrow"><a href="/extractions">Extractions</a> · #{call.data?.short_id ?? id}</p>
  <h1>{#if call.data}<LogTime at={call.data.at} {timeZone} />{:else}Extraction{/if}</h1>
  {#if call.data}
    <p class="pageline__context">
      {call.data.model} · {#if call.data.channel_id}<Name kind="channel" id={call.data.channel_id} name={call.data.channel} />{:else}no channel{/if} ·
      {call.data.latency_ms !== null ? `${call.data.latency_ms.toLocaleString('en')} ms` : 'latency not recorded'}
    </p>
  {/if}
</PageLine>

{#if call.error}
  <div class="empty" role="alert"><strong>No call “{id}”.</strong>{call.error}</div>
{:else if call.data}
  {@const data = call.data}
  <Tabs items={tabs} bind:selected={tab} label="Sections of this call">
    {#snippet panel(which)}
      {#if which === 'changes'}
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
      {:else if which === 'chat'}
        <div class="evidence">
          {#each data.messages as m (m.id)}
            <p class="evidence__line"><span class="evidence__who">{#if m.author_id}<Name kind="member" id={m.author_id} name={m.author} />{:else}{directory.label('member', '', m.author)}{/if}</span><span class="evidence__at"><LogTime at={m.at} {timeZone} /></span><br /><span class="evidence__text"><Mentions text={m.content} /></span></p>
          {/each}
        </div>
      {:else if which === 'prompt'}
        <pre>{data.prompt}</pre>
      {:else}
        <pre>{data.raw_response === 'null' ? 'No response (the call failed).' : data.raw_response}</pre>
      {/if}
    {/snippet}
  </Tabs>
{/if}
