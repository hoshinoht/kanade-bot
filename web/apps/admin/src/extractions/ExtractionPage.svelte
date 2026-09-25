<script lang="ts">
  import '@kanade/ui/styles/evidence.scss';
  import type { Extraction } from '@kanade/api-types';
  import { Tabs, type TabItem } from '@kanade/ui';
  import { Resource } from '../resource.svelte';

  let { id }: { id: string } = $props();
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

<div class="page-head">
  <div>
    <p class="eyebrow"><a href="/extractions">Extractions</a> · #{call.data?.short_id ?? id}</p>
    <h1>{call.data?.at ?? 'Extraction'}</h1>
    {#if call.data}
      <p class="note">
        {call.data.model} · {call.data.channel ?? 'no channel'} ·
        {call.data.latency_ms !== null ? `${call.data.latency_ms.toLocaleString('en')} ms` : 'latency not recorded'}
      </p>
    {/if}
  </div>
</div>

{#if call.error}
  <div class="empty" role="alert"><strong>No call “{id}”.</strong>{call.error}</div>
{:else if call.data}
  {@const data = call.data}
  <Tabs items={tabs} bind:selected={tab} label="Sections of this call">
    {#snippet panel(which)}
      {#if which === 'changes'}
        {#if data.error}<p class="flash flash--error">{data.error}</p>{/if}
        {#if data.amendments.length}
          <table>
            <caption class="vh">Changes the model proposed</caption>
            <thead><tr><th scope="col">Kind</th><th scope="col">Bosses</th><th scope="col">When</th><th scope="col" class="num">Confidence</th><th scope="col">Status</th></tr></thead>
            <tbody>
              {#each data.amendments as a, i (i)}
                <tr><th scope="row">{a.kind}</th><td class="mono">{a.bosses}</td><td class="mono">{a.when}</td><td class="num">{a.confidence.toFixed(2)}</td>
                  <td><span class="status status--{a.status === 'confirmed' ? 'confirmed' : 'planned'}">{a.status}</span></td></tr>
              {/each}
            </tbody>
          </table>
        {:else if !data.error}<p class="note">The model found nothing to change.</p>{/if}
      {:else if which === 'chat'}
        <div class="evidence">
          {#each data.messages as m (m.id)}
            <p class="evidence__line"><span class="evidence__who">{m.author}</span><span class="evidence__at">{m.at}</span><br /><span class="evidence__text">{m.content}</span></p>
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
