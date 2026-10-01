<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/evidence.scss';
  import type { ChatTurn } from '@kanade/api-types';
  import { Icon, Tabs, type TabItem, type Toaster } from '@kanade/ui';
  import { preview } from '../logs/format';
  import { Resource } from '../resource.svelte';
  import { copyText } from '../shared/copy';
  import TextModal from '../shared/TextModal.svelte';
  import { guardrailFlags, messageParts, modelViewState, profileText, routeLabel, took } from './facts';
  import { rounds, transcriptJson, transcriptMarkdown } from './transcript';
  import LogTime from '../logs/LogTime.svelte';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';

  let { id, timeZone = 'Asia/Kuala_Lumpur', toaster }: { id: string; timeZone?: string; toaster?: Toaster } = $props();
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

  const WITHHELD = '[message withheld]';
  // One viewer for any long text on the page: a tool's arguments or result, or a transcript that could not be copied.
  let viewer = $state({ open: false, title: '', eyebrow: '', text: '' });
  const show = (title: string, eyebrow: string, text: string) => (viewer = { open: true, title, eyebrow, text });
  // The cell shows one line; a short button name keeps screen readers from reading up to 8 KiB.
  const PREVIEW = 120;
  const short = (text: string) => {
    const line = preview(text);
    return line.length > PREVIEW ? `${line.slice(0, PREVIEW)}…` : line;
  };

  // Tool calls grouped under the round that asked for them; a turn with no rounds (withheld) keeps its calls in one group.
  const groups = $derived.by((): { key: string; label: string; head: string | null; calls: ChatTurn['tools'] }[] => {
    const data = turn.data;
    if (!data) return [];
    const byRound = rounds(data)
      .filter((r) => r.calls.length)
      .map((r) => ({
        key: `r${r.round}`,
        label: `Round ${r.round} · call `,
        head: [`Round ${r.round}`, r.model ?? 'model unknown', r.effort ? `effort ${r.effort}` : 'no effort sent', routeLabel(r.route), took(r.latency_ms)].join(' · '),
        calls: r.calls,
      }));
    return byRound.length || !data.tools.length ? byRound : [{ key: 'all', label: 'Call ', head: null, calls: data.tools }];
  });
  const roundFlags = (g: { clean: boolean; content_filter: boolean } | undefined) =>
    [g?.clean && 'clean retry', g?.content_filter && 'content filter'].filter(Boolean).join(', ') || '—';

  const uid = $props.id();
  // Sensitive and long: collapsed until asked for, and rendered only once opened.
  let modelViewOpen = $state(false);

  let format = $state<'markdown' | 'json'>('markdown');
  async function copyTranscript() {
    const data = turn.data;
    if (!data) return;
    const text = format === 'json' ? transcriptJson(data, { timeZone }) : transcriptMarkdown(data, { timeZone });
    if (await copyText(text)) toaster?.show({ message: `Transcript copied as ${format === 'json' ? 'JSON' : 'Markdown'}.`, tone: 'ok' });
    else show('Transcript', format === 'json' ? 'JSON' : 'Markdown', text);
  }
</script>

<PageLine class="turn-head">
  <p class="eyebrow"><a href="/chat">Chat</a>{#if turn.data} · <Name kind="member" id={turn.data.member_id || turn.data.member.id} name={turn.data.member.name} />{/if}</p>
  <h1>{#if turn.data}<LogTime at={turn.data.at} {timeZone} />{:else}Interaction{/if}</h1>
  {#if turn.data}<p class="pageline__context">{turn.data.model} · <Name kind="channel" id={turn.data.channel_id} name={turn.data.channel} /> · {turn.data.outcome}</p>{/if}
  {#if turn.data}
    <!-- For agent debugging: the whole turn as one paste. -->
    <div class="page-head__side transcript">
      <label class="field"
        ><span class="vh">Transcript format</span>
        <select bind:value={format}><option value="markdown">Markdown</option><option value="json">JSON</option></select>
      </label>
      <button class="btn" type="button" onclick={() => void copyTranscript()}>Copy transcript</button>
    </div>
  {/if}
</PageLine>

{#if turn.error}
  <div class="empty" role="alert"><strong>No interaction “{id}”.</strong>{turn.error}</div>
{:else if turn.data}
  {@const data = turn.data}
  <Tabs items={tabs} bind:selected={tab} label="Sections of this interaction">
    {#snippet panel(which)}
      {#if which === 'conversation'}
        {@const flags = guardrailFlags(data.guardrail)}
        {@const view = modelViewState(data)}
        {#if data.error || data.error_code}
          <p class="turn-error">
            <strong>Failed:</strong>
            {data.error ?? 'no detail recorded'}{#if data.error_code}<span class="mono turn-error__code">{` (${data.error_code})`}</span>{/if}
          </p>
        {/if}
        <dl class="turn-facts">
          <div><dt>Persona</dt><dd class="mono">{data.persona ?? '—'}</dd></div>
          <div><dt>Reply profile</dt><dd>{profileText(data)}</dd></div>
          <div><dt>Route</dt><dd>{routeLabel(data.route)}</dd></div>
          {#if flags.length}<div><dt>Guardrail</dt><dd>{flags.join(', ')}</dd></div>{/if}
        </dl>
        <h3 class="pane__section">What they asked</h3>
        <p><Mentions text={data.asked} asked /></p>
        <h3 class="pane__section">What it said</h3>
        <p>{#if data.said}<Mentions text={data.said} />{:else}— nothing was sent —{/if}</p>
        {#if view === 'withheld'}
          <h3 class="pane__section">Model view (masked)</h3>
          <p class="note">Model view unavailable for withheld questions.</p>
        {:else if view === 'shown' && data.model_view}
          {@const mv = data.model_view}
          <h3 class="pane__section">
            <button type="button" class="modelview__toggle" aria-expanded={modelViewOpen} aria-controls="{uid}-modelview" onclick={() => (modelViewOpen = !modelViewOpen)}
              ><Icon name="chevron-right" /> Model view (masked)</button
            >
          </h3>
          <p class="note">Admin only, sensitive: the turn as the model received it, with members under fake names.</p>
          <div class="modelview" id="{uid}-modelview" hidden={!modelViewOpen}>
            {#if modelViewOpen}
              <h4 class="modelview__head">Name mapping</h4>
              {#if mv.mapping.length}
                <table class="modelview__map">
                  <caption class="vh">Fake names and the members they stand for</caption>
                  <thead><tr><th scope="col">Fake name</th><th scope="col">Member</th></tr></thead>
                  <tbody>
                    {#each mv.mapping as m (m.token)}<tr><th scope="row" class="mono">{m.token}</th><td>{m.name}</td></tr>{/each}
                  </tbody>
                </table>
              {:else}<p class="note">No names were replaced.</p>{/if}
              {#each mv.rounds as r (r.round)}
                <h4 class="modelview__head">Round {r.round}{#if r.clean}<span class="note"> · clean retry</span>{/if}</h4>
                <h5 class="modelview__label">Request as sent</h5>
                <ol class="modelview__messages">
                  {#each r.request as message, i (i)}
                    {@const m = messageParts(message)}
                    <li>
                      <span class="modelview__role mono">{m.role}</span>
                      {#if m.content !== null}<pre>{m.content}</pre>{/if}
                      {#if m.extra}<pre class="modelview__extra">{m.extra}</pre>{/if}
                      {#if m.content === null && !m.extra}<span class="note">— empty —</span>{/if}
                    </li>
                  {/each}
                </ol>
                <h5 class="modelview__label">Raw reply</h5>
                {#if r.reply !== null}<pre>{r.reply}</pre>{:else}<p class="note">— no text; it asked for tools —</p>{/if}
                {#if r.tool_calls.length}
                  <h5 class="modelview__label">Raw tool calls</h5>
                  <ul class="modelview__calls">
                    {#each r.tool_calls as c, i (i)}<li><span class="mono">{c.name}</span><pre>{c.arguments}</pre></li>{/each}
                  </ul>
                {/if}
              {/each}
              <h4 class="modelview__head">Final reply members saw</h4>
              <pre>{mv.reply}</pre>
            {/if}
          </div>
        {/if}
      {:else if which === 'tools'}
        {#if data.tools.length}
          <!-- One line per call (area principle): long text opens in a viewer. -->
          <div class="table-wrap">
            <table class="trace">
              <caption class="vh">Tool calls</caption>
              <thead><tr><th scope="col">Tool</th><th scope="col">Arguments</th><th scope="col">Return</th><th scope="col" class="num">Took</th><th scope="col">Outcome</th></tr></thead>
              {#each groups as g (g.key)}
                <tbody>
                  {#if g.head}<tr class="trace__round"><th scope="rowgroup" colspan="5">{g.head}</th></tr>{/if}
                  {#each g.calls as t, i (i)}
                    <tr>
                      <th scope="row" class="mono trace__one">{t.name}</th>
                      {#each [['Arguments', t.arguments], ['Return', t.result]] as [what, text] (what)}
                        <td class="mono trace__cell">
                          {#if !text}—
                          {:else if text === WITHHELD}<span class="note">{WITHHELD}</span>
                          {:else}<button type="button" class="trace__preview" onclick={() => show(`${t.name}: ${what?.toLowerCase()}`, `${g.label}${i + 1}`, text!)}
                              ><span class="trace__text">{short(text!)}</span><span class="vh">, open the full {what?.toLowerCase()}</span></button
                            >{/if}
                        </td>
                      {/each}
                      <td class="num trace__one">{took(t.took_ms)}</td>
                      <td><span class="tone tone--{t.outcome === 'ok' ? 'success' : 'danger'}">{t.outcome}</span></td>
                    </tr>
                  {/each}
                </tbody>
              {/each}
            </table>
          </div>
        {:else}<p class="note">No tools were called.</p>{/if}
      {:else if which === 'model'}
        {#if data.rounds.length}
          <div class="table-wrap">
            <table class="rounds">
              <caption class="vh">Model requests, one per round</caption>
              <thead>
                <tr>
                  <th scope="col">Round</th><th scope="col">Model</th><th scope="col">Effort</th><th scope="col">Route</th><th scope="col" class="num">Latency</th><th
                    scope="col">Finish</th
                  ><th scope="col">Requested tools</th><th scope="col">Guardrail</th>
                </tr>
              </thead>
              <tbody>
                {#each data.rounds as r (r.round)}
                  <tr>
                    <th scope="row">Round {r.round}</th>
                    <td class="mono">{r.model || '—'}</td>
                    <td>{r.effort ?? '—'}</td>
                    <td>{routeLabel(r.route)}</td>
                    <td class="num">{took(r.latency_ms)}</td>
                    <td class="mono">{r.finish || '—'}</td>
                    <td class="mono">{r.requested_tools.join(', ') || 'none'}</td>
                    <td>{roundFlags(r.guardrail)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else}<p class="note">No model was called.</p>{/if}
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

<TextModal bind:open={viewer.open} title={viewer.title} eyebrow={viewer.eyebrow} text={viewer.text} {toaster} />

<style>
  .transcript {
    display: flex;
    align-items: flex-end;
    gap: 0.4rem;
  }

  /* Phones: the transcript controls stack, so the tab window keeps its
     minimum height. */
  @media (max-width: 640px) {
    .transcript {
      flex-direction: column;
      align-items: stretch;
      flex-shrink: 0;
    }
  }

  /* One line per call: previews cut with an ellipsis, durations never wrap. */
  .trace {
    table-layout: fixed;
    width: 100%;
    /* Phones scroll the table sideways rather than squeeze the previews to nothing. */
    min-width: 42rem;
  }

  .trace th:nth-child(1) {
    width: 12rem;
  }

  .trace th:nth-child(4) {
    width: 5.5rem;
  }

  .trace th:nth-child(5) {
    width: 6rem;
  }

  .trace__one,
  .trace__cell {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .trace__preview {
    display: block;
    width: 100%;
    min-height: 1.5rem;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .trace__text {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-decoration: underline dotted;
    text-underline-offset: 0.2em;
  }

  .rounds {
    width: 100%;
    min-width: 46rem;
  }

  .rounds td,
  .rounds th {
    white-space: nowrap;
  }

  .trace__round th {
    background: var(--raise);
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .turn-error {
    margin: 0 0 0.8rem;
    padding: 0.5rem 0.75rem;
    border: 2px solid color-mix(in srgb, var(--risk) 45%, transparent);
    border-left-width: 6px;
    border-radius: var(--r-sm);
    color: var(--risk-text);
  }

  .turn-error__code {
    font-size: var(--fs-small);
  }

  .turn-facts {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem 1.4rem;
    margin: 0 0 0.4rem;
    font-size: var(--fs-body-sm);
  }

  .turn-facts > div {
    display: flex;
    gap: 0.4rem;
  }

  .turn-facts dt {
    color: var(--dim-text);
  }

  .turn-facts dd {
    margin: 0;
  }

  .modelview__toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    min-height: 2rem;
    padding: 0.2rem 0.5rem 0.2rem 0.2rem;
    margin-left: -0.2rem;
    border: 0;
    border-radius: var(--r-sm);
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }

  .modelview__toggle:hover {
    background: var(--raise);
  }

  .modelview__toggle :global(.icon) {
    transition: transform 0.15s ease;
  }

  .modelview__toggle[aria-expanded='true'] :global(.icon) {
    transform: rotate(90deg);
  }

  .modelview {
    padding-left: 0.75rem;
    border-left: 3px solid var(--line);
  }

  .modelview__head {
    margin: 1rem 0 0.4rem;
    font-size: var(--fs-body);
    font-weight: 700;
  }

  .modelview__head:first-child {
    margin-top: 0.4rem;
  }

  .modelview__label {
    margin: 0.7rem 0 0.3rem;
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--dim-text);
  }

  .modelview__map {
    width: auto;
    min-width: min(22rem, 100%);
  }

  .modelview__map th,
  .modelview__map td {
    text-align: left;
  }

  .modelview__messages,
  .modelview__calls {
    display: grid;
    gap: 0.5rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .modelview__messages li,
  .modelview__calls li {
    display: grid;
    gap: 0.25rem;
  }

  .modelview__role {
    font-size: var(--fs-small);
    font-weight: 700;
  }

  .modelview__extra {
    color: var(--dim-text);
  }
</style>
