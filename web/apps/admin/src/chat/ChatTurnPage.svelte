<script lang="ts">
  import '@kanade/ui/styles/evidence.scss';
  import type { ChatTurn } from '@kanade/api-types';
  import { Tabs, type TabItem } from '@kanade/ui';
  import { Resource } from '../resource.svelte';

  let { id }: { id: string } = $props();
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
</script>

<div class="page-head">
  <div>
    <p class="eyebrow"><a href="/chat">Chat</a> · {turn.data?.member.name ?? ''}</p>
    <h1>{turn.data?.at ?? 'Interaction'}</h1>
    {#if turn.data}<p class="note">{turn.data.model} · {turn.data.channel} · {turn.data.outcome}</p>{/if}
  </div>
</div>

{#if turn.error}
  <div class="empty" role="alert"><strong>No interaction “{id}”.</strong>{turn.error}</div>
{:else if turn.data}
  {@const data = turn.data}
  <Tabs items={tabs} bind:selected={tab} label="Sections of this interaction">
    {#snippet panel(which)}
      {#if which === 'conversation'}
        <h3 class="pane__section">What they asked</h3>
        <p>{data.asked}</p>
        <h3 class="pane__section">What it said</h3>
        <p>{data.said || '— nothing was sent —'}</p>
      {:else if which === 'tools'}
        {#if data.tools.length}
          <table>
            <caption class="vh">Tool calls</caption>
            <thead><tr><th scope="col">Tool</th><th scope="col">Arguments</th><th scope="col">Return</th><th scope="col" class="num">Took</th><th scope="col">Outcome</th></tr></thead>
            <tbody>
              {#each data.tools as t, i (i)}
                <tr><th scope="row" class="mono">{t.name}</th><td class="mono">{t.arguments}</td><td class="mono">{t.result || '—'}</td><td class="num">{t.took_ms} ms</td>
                  <td><span class="status status--{t.outcome === 'ok' ? 'confirmed' : 'at_risk'}">{t.outcome}</span></td></tr>
              {/each}
            </tbody>
          </table>
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

<style>
  .rounds {
    padding-left: 1.25rem;
  }
</style>
