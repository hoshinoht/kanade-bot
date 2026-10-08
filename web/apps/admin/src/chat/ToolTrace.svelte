<!--
  The Tool trace tab: every call as one table line (tool · arguments · return
  · took · outcome) under "Round n · model · effort · route · latency" group
  rows. Long text opens in the page's viewer.
-->
<script lang="ts">
  import type { ChatTurn } from '@kanade/api-types';
  import { StatusChip } from '@kanade/ui';
  import { took } from '../logs/format';
  import { callTone, routeLabel } from './facts';
  import { rounds } from './transcript';
  import { short, WITHHELD } from './trace';

  let { turn, show }: { turn: ChatTurn; show: (title: string, eyebrow: string, text: string) => void } = $props();

  // Calls grouped under the round that asked for them; a turn with no rounds (withheld) keeps its calls in one group.
  const groups = $derived.by((): { key: string; label: string; head: string | null; calls: ChatTurn['tools'] }[] => {
    const byRound = rounds(turn)
      .filter((r) => r.calls.length)
      .map((r) => ({
        key: `r${r.round}`,
        label: `Round ${r.round} · call `,
        head: [`Round ${r.round}`, r.model ?? 'model unknown', r.effort ? `effort ${r.effort}` : 'no effort sent', routeLabel(r.route), took(r.latency_ms)].join(' · '),
        calls: r.calls,
      }));
    return byRound.length || !turn.tools.length ? byRound : [{ key: 'all', label: 'Call ', head: null, calls: turn.tools }];
  });
</script>

{#if turn.tools.length}
  <!-- One line per call (area principle): long text opens in a viewer. -->
  <div class="table-wrap">
    <table class="chat-trace">
      <caption class="vh">Tool calls</caption>
      <thead><tr><th scope="col">Tool</th><th scope="col">Arguments</th><th scope="col">Return</th><th scope="col" class="num">Took</th><th scope="col">Outcome</th></tr></thead>
      {#each groups as g (g.key)}
        <tbody>
          {#if g.head}<tr class="chat-trace__round"><th scope="rowgroup" colspan="5">{g.head}</th></tr>{/if}
          {#each g.calls as t, i (i)}
            <tr>
              <th scope="row" class="mono chat-trace__one">{t.name}</th>
              {#each [['Arguments', t.arguments], ['Return', t.result]] as [what, text] (what)}
                <td class="mono chat-trace__cell">
                  {#if !text}—
                  {:else if text === WITHHELD}<span class="note">{WITHHELD}</span>
                  {:else}<button type="button" class="chat-trace__preview" onclick={() => show(`${t.name}: ${what?.toLowerCase()}`, `${g.label}${i + 1}`, text!)}
                      ><span class="chat-trace__text">{short(text!)}</span><span class="vh">, open the full {what?.toLowerCase()}</span></button
                    >{/if}
                </td>
              {/each}
              <td class="num chat-trace__one">{took(t.took_ms)}</td>
              <td><StatusChip tone={callTone(t.outcome)} legacyTone={t.outcome === 'ok' ? 'success' : 'danger'}>{t.outcome}</StatusChip></td>
            </tr>
          {/each}
        </tbody>
      {/each}
    </table>
  </div>
{:else}<p class="note">No tools were called.</p>{/if}
