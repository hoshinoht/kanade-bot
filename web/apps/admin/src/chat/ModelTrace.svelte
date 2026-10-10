<!--
  The Model trace tab (B_ChatTrace): one card per request round — model,
  effort, route and latency; finish, requested tools and guardrail; then the
  round's tool calls, each with its return line. Expand opens a call whole.
  Request ids are the `x-request-id`s the round sent, to find it in Kanata's log.
-->
<script lang="ts">
  import type { ChatTurn, RoundGuardrail } from '@kanade/api-types';
  import { StatusChip } from '@kanade/ui';
  import { took } from '../logs/format';
  import { callTone, routeLabel, routeTone } from './facts';
  import { rounds } from './transcript';
  import { short, WITHHELD } from './trace';

  let { turn, show }: { turn: ChatTurn; show: (title: string, eyebrow: string, text: string) => void } = $props();
  const uid = $props.id();
  const list = $derived(rounds(turn));
  const guard = (round: number) => turn.rounds.find((r) => r.round === round)?.guardrail;
  const flags = (g: RoundGuardrail | undefined) => [g?.clean && 'clean retry', g?.content_filter && 'content filter'].filter(Boolean).join(', ') || '—';
  const line = (text: string) => (!text ? '—' : text === WITHHELD ? text : `“${short(text)}”`);
  const whole = (call: ChatTurn['tools'][number]) => `Arguments:\n${call.arguments || '—'}\n\nReturn:\n${call.result || '—'}`;
</script>

{#if list.length}
  <div class="chat-rounds">
    {#each list as r (r.round)}
      <section class="chat-round" data-fid="chat-round" aria-labelledby="{uid}-r{r.round}">
        <div class="chat-round__head" data-fid="chat-round-head">
          <h3 class="chat-round__title" id="{uid}-r{r.round}">Round {r.round}</h3>
          <span class="mono">{r.model ?? '—'}</span>
          <span class="chat-chip">{r.effort ? `effort ${r.effort}` : 'no effort sent'}</span>
          <span class="chat-chip chat-chip--{routeTone(r.route)}">{routeLabel(r.route)}</span>
          <span class="chat-round__latency mono" title="Latency">{took(r.latency_ms)}</span>
        </div>
        <dl class="chat-round__facts" data-fid="chat-round-facts">
          <div><dt class="cap">Finish</dt><dd class="mono">{r.finish || '—'}</dd></div>
          <div><dt class="cap">Requested tools</dt><dd class="mono">{r.requested_tools.join(', ') || 'none'}</dd></div>
          <div><dt class="cap">Guardrail</dt><dd>{flags(guard(r.round))}</dd></div>
          {#if r.request_ids.length}
            <div><dt class="cap">Request ids</dt><dd class="mono">{r.request_ids.join(', ')}</dd></div>
          {/if}
        </dl>
        {#each r.calls as call, i (i)}
          <div class="chat-tool" data-fid="chat-tool">
            <span class="chat-tool__name mono">{call.name}</span>
            <span class="chat-tool__args mono" title={call.arguments === WITHHELD ? undefined : short(call.arguments)}>{call.arguments ? short(call.arguments) : '—'}</span>
            <span class="mono">{took(call.took_ms)}</span>
            <StatusChip tone={callTone(call.outcome)}>{call.outcome}</StatusChip>
            {#if call.arguments !== WITHHELD}
              <button type="button" class="chat-tool__expand" onclick={() => show(call.name, `Round ${r.round} · call ${i + 1}`, whole(call))}
                >Expand<span class="vh"> {call.name}, round {r.round} call {i + 1}</span></button
              >
            {/if}
          </div>
          <p class="chat-tool__return">Return: {line(call.result)}</p>
        {/each}
      </section>
    {/each}
  </div>
{:else if turn.session_id}<p class="note">No round answered. Gateway session <span class="mono">{turn.session_id}</span>.</p>
{:else}<p class="note">No model was called.</p>{/if}
