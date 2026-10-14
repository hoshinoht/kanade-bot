<!--
  One backend group (B_LimitsLive, rows not columns): the head — name, the
  backend, the breaker chip in words, and the permits in use — then the
  permit bar across the card, one line of facts with the models, the open
  breaker's sentence, and the calls waiting for a permit when any are.
-->
<script lang="ts">
  import type { BackendGroup } from '@kanade/api-types';
  import { Icon, WavyProgress } from '@kanade/ui';
  import { BREAKER, serverTime } from './view';

  let { group: g, wavy, zone, phone = false }: { group: BackendGroup; wavy: boolean; zone: string | undefined; phone?: boolean } = $props();

  const breaker = $derived(BREAKER[g.breaker.state]);
  const flight = $derived(g.permits.in_use ? 'requests in flight' : 'idle');
  const probe = $derived(g.breaker.retry_at ? serverTime(g.breaker.retry_at, zone) : '');
</script>

<article class="limits-group" class:limits-group--phone={phone} data-fid="limits-group" aria-labelledby="limits-g-{g.name}">
  <header class="limits-group__head">
    <h3 class="limits-group__name" id="limits-g-{g.name}">{g.name}</h3>
    <span class="limits-group__via mono">via {g.backend}</span>
    <span class="tone tone--{breaker.tone} limits-group__breaker" data-fid="limits-breaker"><Icon name={breaker.icon} />{breaker.word}</span>
    <p class="limits-group__permits">
      <span class="limits-group__num mono">{g.permits.in_use}<span class="limits-group__total">/{g.permits.total}</span></span>
      <span>permits in use</span>
      <span class="limits-group__flight">{phone ? '' : '· '}{flight}</span>
    </p>
  </header>
  <div class="limits-group__bar" data-fid="limits-bar-row">
    <WavyProgress
      value={g.permits.in_use}
      max={g.permits.total}
      {wavy}
      fullWave
      label="{g.name} permits in use"
      text="{g.permits.in_use} of {g.permits.total} in use{g.permits.in_use ? ', requests in flight' : ''}"
    />
  </div>
  <dl class="limits-group__facts" data-fid="limits-facts">
    <div><dt>Rate bucket</dt><dd class="mono">{g.rate.available}/{g.rate.capacity} <span class="limits-dim">· +{g.rate.refill_per_min}/min</span></dd></div>
    <div><dt>Retry budget</dt><dd class="mono">{g.retry.remaining}/{g.retry.capacity}</dd></div>
    <div><dt>Since</dt><dd class="mono">{serverTime(g.breaker.since, zone)}</dd></div>
    {#if g.breaker.failures}<div><dt>Failures</dt><dd class="mono">{g.breaker.failures}</dd></div>{/if}
    {#if probe}<div><dt>Next probe</dt><dd class="mono"><b>{probe}</b></dd></div>{/if}
    {#if !g.queue.length}<div><dt>Waiting</dt><dd class="mono">none</dd></div>{/if}
    <div class="limits-group__models">
      <dt>Models</dt>
      <dd>{#each g.models as model (model)}<span class="limits-model mono">{model}</span>{/each}</dd>
    </div>
  </dl>
  {#if g.breaker.state === 'open'}
    <p class="limits-group__refused">
      Calls to {g.name} are refused{#if probe}&nbsp;until the next probe at <b class="mono">{probe}</b>{/if}.
    </p>
  {/if}
  {#if g.queue.length}
    <section class="limits-group__waiting" data-fid="limits-waiting" aria-labelledby="limits-w-{g.name}">
      <h4 class="cap" id="limits-w-{g.name}">Waiting for a permit · {g.queue.length}</h4>
      <ol class="limits-waiting">
        {#each g.queue as item (item.position)}
          <li class="limits-waiting__row" data-fid="limits-waiting-row">
            <span class="limits-pos mono"><span class="vh">Position </span>{item.position}</span>
            <span class="limits-waiting__kind mono">{item.kind}</span>
            <span class="limits-waiting__who">{item.who}</span>
            <b class="limits-waiting__time mono">{item.waiting_s} s<span class="vh"> waiting</span></b>
          </li>
        {/each}
      </ol>
    </section>
  {/if}
</article>
