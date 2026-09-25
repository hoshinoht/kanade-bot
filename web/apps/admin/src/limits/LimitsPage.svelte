<!--
  v5 Limits: the Kanata gateway's backend groups replace v4's single model
  lock. Each group shows its permits, queue (with positions), rate bucket,
  retry budget and circuit breaker; admission refusals are counted by kind,
  including key-level ones. Allowances keep v4's per-member reset. Polled.
-->
<script lang="ts">
  import '@kanade/ui/styles/evidence.scss';
  import type { Limits } from '@kanade/api-types';
  import { ApiRequestError, createClient, createPoller } from '@kanade/client';
  import { Tabs, Toaster, type TabItem } from '@kanade/ui';
  import { errorText, send } from '../resource.svelte';

  let { toaster }: { toaster: Toaster } = $props();

  let limits = $state<Limits | null>(null);
  let error = $state('');
  const client = createClient();
  const poller = createPoller<Limits>({
    task: (signal) => client.get<Limits>('/api/admin/limits', { signal }),
    intervalMs: 5000,
    maxFailures: 6,
    onData: (next) => {
      limits = next;
      error = '';
    },
    onError: (e) => {
      const unbuilt = e instanceof ApiRequestError && e.status === 404;
      if (unbuilt) poller.stop();
      error = unbuilt ? errorText(e) : 'Could not refresh the limits; retrying.';
    },
  });
  $effect(() => {
    poller.start();
    return () => poller.stop();
  });

  type Tab = 'backends' | 'queue' | 'admission' | 'allowances';
  let tab = $state<Tab>('backends');
  const queued = $derived(limits?.groups.reduce((n, g) => n + g.queue.length, 0) ?? 0);
  const tabs = $derived<TabItem<Tab>[]>([
    { id: 'backends', label: 'Backends', count: limits?.groups.length ?? null },
    { id: 'queue', label: 'Queue', count: queued },
    { id: 'admission', label: 'Admission', count: limits?.admission.refusals.reduce((n, r) => n + r.count, 0) ?? null },
    { id: 'allowances', label: 'Allowances', count: limits?.allowances.length ?? null },
  ]);
  const BREAKER = {
    closed: { word: 'closed — calls flow', status: 'confirmed' },
    half_open: { word: 'half-open — probing', status: 'planned' },
    open: { word: 'open — calls refused', status: 'at_risk' },
  } as const;
  const REFUSAL: Record<string, string> = {
    rate: 'rate limit',
    concurrency: 'too many in flight',
    quota: 'key quota spent',
    key_rate: 'key rate limit',
    key_quota: 'key quota spent',
  };
  const busiest = $derived(limits?.groups.find((g) => g.permits.in_use >= g.permits.total));

  async function reset(id: string, name: string) {
    const result = await send((c) => c.delete<{ message: string }>(`/api/admin/limits/windows/${encodeURIComponent(id)}`));
    toaster.show({ message: result.ok ? result.value.message : `Couldn't reset ${name}: ${result.message}`, tone: result.ok ? 'ok' : 'error' });
    if (result.ok) void poller.refresh();
  }
</script>

<div class="page-head">
  <div>
    <p class="eyebrow">Capacity</p>
    <h1>{limits ? (busiest ? `${busiest.name} is at capacity` : 'Every backend has room') : 'Limits'}</h1>
    <p class="note">What each model backend is doing now, what is waiting, and what the gateway turned away.</p>
  </div>
  <p class="field__error" role="status">{error}</p>
</div>

{#if limits}
  {@const data = limits}
  <Tabs items={tabs} bind:selected={tab} label="Limits">
    {#snippet panel(which)}
      {#if which === 'backends'}
        <div class="stats">
          {#each data.groups as g (g.name)}
            <section class="stat" aria-labelledby="g-{g.name}">
              <h3 class="stat__name" id="g-{g.name}">{g.name} · {g.backend}</h3>
              <p class="stat__big">{g.permits.in_use}/{g.permits.total}<span class="stat__unit">permits in use</span></p>
              <meter min="0" max={g.permits.total} value={g.permits.in_use} aria-label="{g.name} permits in use"></meter>
              <dl class="stat__rows">
                <div><dt>Queue</dt><dd class="mono">{g.queue.length}</dd></div>
                <div><dt>Rate bucket</dt><dd class="mono">{g.rate.available}/{g.rate.capacity} · +{g.rate.refill_per_min}/min</dd></div>
                <div><dt>Retry budget</dt><dd class="mono">{g.retry.remaining}/{g.retry.capacity}</dd></div>
                <div>
                  <dt>Breaker</dt>
                  <dd><span class="status status--{BREAKER[g.breaker.state].status}">{BREAKER[g.breaker.state].word}</span></dd>
                </div>
                <div><dt>Since</dt><dd class="mono">{g.breaker.since}{g.breaker.failures ? ` · ${g.breaker.failures} failures` : ''}</dd></div>
                {#if g.breaker.retry_at}<div><dt>Next probe</dt><dd class="mono">{g.breaker.retry_at}</dd></div>{/if}
                <div><dt>Models</dt><dd class="mono">{g.models.join(', ')}</dd></div>
              </dl>
            </section>
          {/each}
        </div>
      {:else if which === 'queue'}
        {#if queued}
          <table>
            <caption class="vh">Waiting for a permit, by backend and position</caption>
            <thead><tr><th scope="col">Backend</th><th scope="col" class="num">Position</th><th scope="col">What</th><th scope="col">For</th><th scope="col" class="num">Waiting</th></tr></thead>
            <tbody>
              {#each data.groups as g (g.name)}
                {#each g.queue as item (item.position)}
                  <tr><th scope="row">{g.name}</th><td class="num">{item.position}</td><td>{item.kind}</td><td>{item.who}</td><td class="num">{item.waiting_s} s</td></tr>
                {/each}
              {/each}
            </tbody>
          </table>
        {:else}<p class="note">Nothing is waiting.</p>{/if}
      {:else if which === 'admission'}
        <p class="note">Refused by the Kanata gateway in the {data.admission.window}.</p>
        <table>
          <caption class="vh">Admission refusals by kind</caption>
          <thead><tr><th scope="col">Kind</th><th scope="col">Scope</th><th scope="col">Where</th><th scope="col" class="num">Count</th><th scope="col">Last</th></tr></thead>
          <tbody>
            {#each data.admission.refusals as r (r.kind + r.target)}
              <tr>
                <th scope="row">{REFUSAL[r.kind] ?? r.kind}</th>
                <td><span class="chip chip--mono">{r.scope === 'key' ? 'key-level' : 'backend'}</span></td>
                <td class="mono">{r.target}</td><td class="num">{r.count}</td><td class="mono">{r.last_at}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}
        <p class="note">Members with chatbot access. Staff are exempt from every budget. The member-facing view comes later.</p>
        <table>
          <caption class="vh">Chatbot allowances</caption>
          <thead><tr><th scope="col">Member</th><th scope="col">Allowance</th><th scope="col">This window</th><th scope="col"><span class="vh">Actions</span></th></tr></thead>
          <tbody>
            {#each data.allowances as a (a.member.id)}
              <tr>
                <th scope="row">{a.member.name}{#if a.staff} <span class="chip">staff</span>{/if}{#if a.override} <span class="chip">own allowance</span>{/if}</th>
                <td class="mono">{a.allowance ? `${a.allowance.count} per ${a.allowance.per_s}s` : 'exempt'}</td>
                <td class="mono">{a.allowance ? (a.used ? `${a.used} used, ${a.allowance.count - a.used} left` : 'idle') : '—'}</td>
                <td>{#if a.allowance && a.used}<button class="btn" type="button" onclick={() => void reset(a.member.id, a.member.name)} aria-label="Reset {a.member.name}'s window">Reset</button>{/if}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {/snippet}
  </Tabs>
{:else}
  <section class="card window-fill" aria-busy="true"><div class="card__head"><h2 class="card__title">Loading the limits…</h2></div></section>
{/if}

<style>
  meter {
    width: 100%;
    height: 0.6rem;
    margin-bottom: 0.4rem;
  }
</style>
