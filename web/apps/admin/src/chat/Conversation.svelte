<!--
  The Conversation tab (B_Chat): the turn's facts as tonal chips, the
  question and the reply as bubbles, reasoning per round, and — only for a
  historical masked turn with a stored Model view — that view, collapsed.
-->
<script lang="ts">
  import type { ChatTurn } from '@kanade/api-types';
  import Reasoning from '../logs/Reasoning.svelte';
  import Mentions from '../names/Mentions.svelte';
  import { guardrailFlags, modelViewState, profileText, routeLabel } from './facts';
  import ModelView from './ModelView.svelte';

  let { turn }: { turn: ChatTurn } = $props();
  const flags = $derived(guardrailFlags(turn.guardrail));
  const view = $derived(modelViewState(turn));
  const uid = $props.id();
</script>

{#if turn.error || turn.error_code}
  <p class="chat-error">
    <strong>Failed:</strong>
    {turn.error ?? 'no detail recorded'}{#if turn.error_code}<span class="mono chat-error__code">{` (${turn.error_code})`}</span>{/if}
  </p>
{/if}
<dl class="chat-facts" data-fid="chat-facts">
  <div class="chat-fact"><dt>Persona</dt><dd class="mono">{turn.persona ?? '—'}</dd></div>
  <div class="chat-fact"><dt>Reply profile</dt><dd>{profileText(turn)}</dd></div>
  <div class="chat-fact"><dt>Route</dt><dd>{routeLabel(turn.route)}</dd></div>
  {#if flags.length}<div class="chat-fact"><dt>Guardrail</dt><dd>{flags.join(', ')}</dd></div>{/if}
</dl>
<section class="chat-bubble" data-fid="chat-asked" aria-labelledby="{uid}-chat-asked-cap">
  <h3 class="cap" id="{uid}-chat-asked-cap">What they asked</h3>
  <p class="chat-bubble__text"><Mentions text={turn.asked} asked /></p>
</section>
<section class="chat-bubble chat-bubble--bot" data-fid="chat-said" aria-labelledby="{uid}-chat-said-cap">
  <h3 class="cap" id="{uid}-chat-said-cap">What it said</h3>
  <p class="chat-bubble__text">{#if turn.said}<Mentions text={turn.said} />{:else}— nothing was sent —{/if}</p>
</section>
{#each turn.rounds as r (r.round)}
  <Reasoning text={r.reasoning_content} tokens={r.reasoning_tokens} round={r.round} />
{/each}
{#if view === 'withheld'}
  <section class="chat-modelview" aria-labelledby="{uid}-chat-modelview-title">
    <h3 class="chat-modelview__title" id="{uid}-chat-modelview-title">Model view (masked)</h3>
    <p class="note">Model view unavailable for withheld questions.</p>
  </section>
{:else if view === 'shown' && turn.model_view}
  <ModelView view={turn.model_view} />
{/if}
