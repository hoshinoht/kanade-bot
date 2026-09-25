<!--
  One run's sheet, v4 partials/run.html in a modal: entry art under the veil,
  bosses with portraits and levels, the answer tally, answer chips with remove
  and add, the reminder-card timeline, Move + Preview ping, the status control
  and the folded answers editor. Every action is server-confirmed and reported
  in the sheet's own status line: a modal dialog makes the page's toast region
  inert, so results and Undo must live inside it. Failed input stays in its field.
-->
<script lang="ts">
  import type { Member, Participant, Run, RunStatus, Week } from '@kanade/api-types';
  import { AnswerChip, BossTag, Icon, Modal, StatusMark, dayLabel, runTitle } from '@kanade/ui';
  import { directory, memberLabel } from './names/directory.svelte';
  import Name from './names/Name.svelte';
  import BlamePanel from './sheet/BlamePanel.svelte';
  import { parseWhen } from './sheet/parseWhen';
  import { STATUS_LABELS, type MoveOutcome } from './store.svelte';
  import type { Slot } from './planner/keyboardMove';

  let {
    open = $bindable(false),
    run,
    week,
    members,
    onmove,
    onstatus,
    onrsvp,
    onroster,
    onping,
    onreset,
    onreread,
  }: {
    open: boolean;
    run: Run | null;
    week: Week;
    members: Member[];
    onmove: (runId: string, to: Slot) => Promise<MoveOutcome>;
    onstatus: (runId: string, status: RunStatus) => Promise<MoveOutcome>;
    onrsvp: (runId: string, memberId: string, answer: 'yes' | 'no' | 'clear') => Promise<MoveOutcome>;
    onroster: (runId: string, change: { add?: string; remove?: string }) => Promise<MoveOutcome>;
    onping: (runId: string) => Promise<MoveOutcome>;
    onreset: (runId: string) => Promise<MoveOutcome>;
    onreread: (run: Run) => Promise<MoveOutcome>;
  } = $props();

  const ANSWERS = [
    ['yes', 'On'],
    ['no', 'Out'],
    ['clear', 'Clear'],
  ] as const;
  const uid = $props.id();
  let to = $state('');
  let error = $state('');
  let busy = $state(false);
  let notice = $state<{ ok: boolean; message: string; undo?: () => void } | null>(null);
  let seeded: string | null = null;

  // A fresh field per opened run; a failed move keeps what was typed.
  $effect(() => {
    if (open && run && seeded !== run.id) {
      seeded = run.id;
      to = '';
      error = '';
      notice = null;
    }
    if (!open) seeded = null;
  });

  const counts = $derived.by(() => {
    const all = run?.participants ?? [];
    const count = (answer: Participant['answer']) => all.filter((p) => p.answer === answer).length;
    return { on: count('yes'), out: count('no'), maybe: count('maybe'), waiting: count('waiting'), total: all.length };
  });
  const arts = $derived(run ? run.bosses.filter((b) => b.art) : []);
  const addable = $derived(run ? members.filter((m) => !run.participants.some((p) => p.id === m.id)) : []);
  // A twin reads "Ren (2)", never its id.
  const who = (p: Participant) => memberLabel(run?.participants ?? [], p.id);

  async function act(work: () => Promise<MoveOutcome>, undo?: () => Promise<MoveOutcome>) {
    busy = true;
    try {
      const outcome = await work();
      notice = { ...outcome, undo: outcome.ok && undo ? () => void act(undo) : undefined };
      return outcome;
    } finally {
      busy = false;
    }
  }

  // aria-disabled, not disabled: the re-read takes seconds and focus must stay put.
  function rereadChannel(target: Run) {
    if (busy) return;
    notice = { ok: true, message: `Re-reading ${target.channel}…` };
    void act(() => onreread(target));
  }

  // Status changes are reversible, so they get Undo here rather than v4's confirm dialog.
  function setStatus(status: RunStatus) {
    if (!run || run.status === status) return;
    const id = run.id;
    const before = run.status;
    void act(() => onstatus(id, status), before === 'at_risk' ? undefined : () => onstatus(id, before));
  }

  async function move(event: SubmitEvent) {
    event.preventDefault();
    if (!run) return;
    const parsed = parseWhen(to, week.days, { day: run.day, time: run.time });
    if (!parsed.ok) {
      error = parsed.message;
      return;
    }
    const id = run.id;
    busy = true;
    const outcome = await onmove(id, parsed.slot).finally(() => (busy = false));
    if (outcome.ok) open = false;
    else error = outcome.message;
  }

  function add(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const id = select.value;
    select.value = '';
    if (run && id) void act(() => onroster(run.id, { add: id }));
  }
</script>

<Modal bind:open title={run ? runTitle(run) : 'Run'} wide flush>
  {#if run}
    <article class="run run--{run.status}">
      {#each arts as boss, index (boss.key)}
        <img
          class="run__art"
          class:run__art--lead={arts.length > 1 && index === 0}
          class:run__art--second={arts.length > 1 && index > 0}
          src={boss.art}
          alt=""
          decoding="async"
        />
      {/each}
      <div class="run__time">
        {run.status === 'otot' || !run.time ? 'own time' : run.time}
        <small>{dayLabel(week, run.day)}</small>
      </div>

      <div>
        <div class="run__bosses">
          {#if run.bosses.length > 1}
            <ul class="bosslist">
              {#each run.bosses as boss (boss.token)}<li><BossTag {boss} portrait level /></li>{/each}
            </ul>
          {:else}
            {#each run.bosses as boss (boss.token)}<BossTag {boss} portrait level />{/each}
          {/if}
        </div>
        <div class="run__meta">
          <StatusMark status={run.status} words pill />
          <span class="mono">{counts.on}/{counts.total} on</span>
          {#if counts.out}<span class="tone tone--danger mono">{counts.out} out</span>{/if}
          {#if counts.maybe}<span class="tone tone--info mono">{counts.maybe} maybe</span>{/if}
          {#if counts.waiting}<span class="tone tone--neutral mono">{counts.waiting} waiting</span>{/if}
          <span class="chanmark"
            >{run.channel}
            <button
              type="button"
              class="chanmark__btn"
              aria-label="Re-read {run.channel} from Discord and propose any changes"
              aria-disabled={busy}
              onclick={() => rereadChannel(run)}><Icon name="refresh-cw" /></button
            ></span
          >
          <span class="id">#{run.short_id}</span>
        </div>
        <div class="run__people">
          {#each run.participants as person (person.id)}
            <AnswerChip participant={person} label={who(person)}>
              <button
                type="button"
                class="chip__x"
                aria-label="Take {who(person)} off this run for this week only"
                disabled={busy}
                onclick={() => void act(() => onroster(run.id, { remove: person.id }))}>×</button
              >
            </AnswerChip>
          {/each}
          {#if addable.length > 0}
            <select class="chip__add" aria-label="Add someone to {runTitle(run)} for this week" onchange={add} disabled={busy}>
              <option value="">+ add…</option>
              {#each addable as member (member.id)}<option value={member.id}>{memberLabel(members, member.id)}</option>{/each}
            </select>
          {/if}
        </div>
        {#if run.roster_change}
          <p class="run__meta">
            <span class="chip chip--waiting"
              >this week:{#each run.roster_change.out as person (person.id)}{` −${directory.label('member', person.id, person.name)}`}{/each}{#each run.roster_change.in as person (person.id)}{` +${directory.label('member', person.id, person.name)}`}{/each}</span
            >
          </p>
        {/if}
        {#if run.cards.length > 0}
          <p class="run__cards">
            <a class="run__cards-label" href="/reminders?run={encodeURIComponent(run.id)}" aria-label="Cards: every reminder for this run">Cards</a>
            {#each run.cards as card (card.label)}
              {#if card.state === 'posted' && card.url}
                <a class="cardlink cardlink--posted" href={card.url} target="_blank" rel="noopener noreferrer"
                  title="Open the {card.label} card in Discord (posted {card.at})"
                  >{card.label} <span class="cardlink__at">{card.at}</span><span class="cardlink__out"><Icon name="external-link" /></span
                  ><span class="vh">(posted, opens Discord)</span></a
                >
              {:else if card.state === 'skipped'}
                <span class="cardlink cardlink--skipped" title="Due {card.at}, retired without posting">{card.label} skipped</span>
              {:else}
                <span class="cardlink cardlink--queued" title="Not posted yet; fires at {card.at}"
                  >{card.label} <span class="cardlink__at">{card.at}</span><span class="vh">(queued)</span></span
                >
              {/if}
            {/each}
          </p>
        {/if}
      </div>

      <div class="run__actions">
        <form onsubmit={move} novalidate>
          <input
            bind:value={to}
            placeholder="wed 21:30"
            size="10"
            aria-label="Move {runTitle(run)} to a new day and time"
            aria-invalid={error ? 'true' : undefined}
            aria-describedby="{uid}-error"
          />
          <button class="btn" type="submit" disabled={busy}>Move</button>
        </form>
        <button class="btn" type="button" disabled={busy} title="Post this run's morning card now, as a TEST message"
          onclick={() => void act(() => onping(run.id))}>Preview ping</button
        >
        {#if run.fixed_id && run.amended && !['done', 'cancelled'].includes(run.status)}
          <!-- v5: undo this week's amendments in one step (day, time and roster). -->
          <button class="btn" type="button" disabled={busy} title="Put this run back on its weekly timing"
            onclick={() => void act(() => onreset(run.id))}>Reset to fixed</button
          >
        {/if}
      </div>
      <p class="field__error run__error" id="{uid}-error" role="alert">{error}</p>

      <div class="statusbar" role="group" aria-labelledby="{uid}-status">
        <span class="statusbar__label" id="{uid}-status">Status</span>
        <div class="seg seg--status">
          {#each Object.entries(STATUS_LABELS) as [value, label] (value)}
            <button
              type="button"
              class="seg__btn"
              aria-pressed={run.status === value}
              disabled={busy}
              onclick={() => setStatus(value as RunStatus)}>{label}</button
            >
          {/each}
        </div>
        {#if run.status === 'at_risk'}
          <span class="statusbar__note">someone said no — pick a status to settle it</span>
        {/if}
      </div>

      <p class="sheet__notice" class:sheet__notice--error={notice && !notice.ok} role="status">
        {#if notice}
          {notice.message}
          {#if notice.undo}<button type="button" class="btn" onclick={notice.undo} disabled={busy}>Undo</button>{/if}
        {/if}
      </p>

      <details class="answers">
        <summary class="btn answers__summary"><Icon name="chevron-right" /> Answers — set who’s in or out</summary>
        <div class="answers__body">
          <p class="answers__hint">Records the answer as if they had reacted — the run's cards update in Discord.</p>
          {#each run.participants as person (person.id)}
            <div class="answers__row">
              <span class="answers__who"><Name kind="member" id={person.id} name={who(person)} /></span>
              <div class="seg seg--answer" role="group" aria-label="Answer for {who(person)} on {runTitle(run)}">
                {#each ANSWERS as [value, label] (value)}
                  <button
                    type="button"
                    class="seg__btn"
                    aria-pressed={value === 'clear' ? false : person.answer === value}
                    disabled={busy || (value === 'clear' && person.answer === 'waiting')}
                    onclick={() => void act(() => onrsvp(run.id, person.id, value))}>{label}</button
                  >
                {/each}
              </div>
            </div>
          {/each}
        </div>
      </details>
      {#key run.id}<BlamePanel runId={run.id} {members} timezone={week.timezone} />{/key}
    </article>
  {/if}
</Modal>

<style>
  .run__error:empty {
    display: none;
  }

  .run__error {
    grid-column: 1 / -1;
    margin: 0;
  }

  .sheet__notice {
    grid-column: 1 / -1;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.3rem 0.7rem;
    margin: 0.3rem 0 0;
    font-size: var(--fs-small);
    color: var(--ok-text);
  }

  .sheet__notice--error {
    color: var(--risk-text);
  }
</style>
