<!--
  An empty Inbox tab (M3E B_Empty): why nothing waits, with the extractor's
  last read and the ways on, beside the tab's last three decisions. Both
  reads are supporting facts: a failed one leaves its part out.
-->
<script lang="ts">
  import type { ExtractionRow, Extractions, InboxTab, PastPage } from '@kanade/api-types';
  import { StateNote } from '@kanade/ui';
  import { shortAt } from '../history/describe';
  import Name from '../names/Name.svelte';
  import { Resource } from '../resource.svelte';
  import { OUTCOME } from './past';

  let { tab, timeZone }: { tab: InboxTab; timeZone: string } = $props();

  const past = new Resource<PastPage>('/api/admin/inbox/past');
  const calls = new Resource<Extractions>('/api/admin/extractions');
  $effect(() => {
    void past.load();
    if (tab === 'extractor') void calls.load();
  });

  const recent = $derived((past.data?.items ?? []).filter((p) => p.tab === tab).slice(0, 3));
  const last = $derived(
    tab === 'extractor' ? (calls.data?.rows ?? []).reduce<ExtractionRow | null>((a, r) => (!a || Date.parse(r.at) > Date.parse(a.at) ? r : a), null) : null,
  );
</script>

{#snippet ways()}
  <a class="btn" href="/extractions">See recent extractions</a>
  <a class="btn" href="/config?section=rescan">Re-read channels</a>
{/snippet}

<div class="inbox-empty" data-fid="empty-body">
  <StateNote icon="check" title="Nothing waiting" actions={tab === 'extractor' ? ways : undefined}>
    {#if tab === 'extractor'}
      The extractor posts a card when it reads a change in a watched channel. {#if last}<span
          >The last read was <time class="mono inbox-empty__at" datetime={last.at}>{shortAt(last.at, timeZone)}</time>{#if last.channel}<span>&nbsp;in <a href="/extractions?channel={encodeURIComponent(last.channel_id)}"><Name kind="channel" id={last.channel_id} name={last.channel} plain /></a></span
            >{/if}.</span
        >{/if}
    {:else}
      Nothing waiting here. Members’ requests arrive here.
    {/if}
  </StateNote>
  {#if recent.length}
    <section class="inbox-empty__recent" data-fid="empty-recent" aria-labelledby="inbox-recent-{tab}">
      <h3 class="cap inbox-empty__head" id="inbox-recent-{tab}" data-fid="empty-recent-head">Recently decided</h3>
      <ul class="inbox-empty__rows">
        {#each recent as p (p.id)}
          <li>
            <a class="inbox-empty__row" data-fid="empty-recent-row" href="/inbox?tab=past&item={encodeURIComponent(p.id)}">
              <span class="inbox-empty__lines">
                <span class="inbox-empty__what">{p.summary}</span>
                <span class="inbox-empty__when" class:inbox-empty__when--risk={OUTCOME[p.outcome].tone === 'risk'}
                  >{OUTCOME[p.outcome].label.toLowerCase()} · <time class="mono" datetime={p.decided_at}>{shortAt(p.decided_at, timeZone)}</time></span
                >
              </span>
            </a>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
</div>
