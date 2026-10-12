<!--
  The run sheet's Estimate panel (boards B_CharEstimateRun, B_EstRunStates,
  phone twins): one MapleScouter lookup over the party's picked or inferred
  characters (at most 6), as a job of up to 2 minutes. Per boss of the run:
  the combined minutes and %, "technically possible", "not complete"; per
  member: effective %. Members without a character are listed as missing,
  never 0%, and an error never leaves a partial total. Labelled as
  MapleScouter's and as planning only. No scouter URL: no panel.
-->
<script lang="ts">
  import type { Boss, RunCharacters, SettingsActor } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, Icon, StatusChip } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-data.scss';
  import EstimateProgress from './EstimateProgress.svelte';
  import { dayTime, ESTIMATE_LABEL, ESTIMATE_NOTE, estimateErrorText, hourMinute, joinNames, lookupTarget, minutes, MISSING_REASONS, percent } from './format';
  import { chardataRoutes, EstimateLookup } from './routes.svelte';

  let {
    runId,
    bosses,
    slots,
    name,
    timeZone,
    sheet,
    actor,
  }: {
    runId: string;
    /** The run's bosses, in order (the totals' columns). */
    bosses: Boss[];
    /** Who brings which character (the run sheet's picks), for the counts before a lookup. */
    slots: RunCharacters | null;
    /** A party member as the sheet names them. */
    name: (memberId: string) => string;
    timeZone: string;
    /** The phone's Party list: the button under the panel, 44 px. */
    sheet: boolean;
    actor: (who: SettingsActor) => string;
  } = $props();

  const uid = $props.id();
  const lookup = $derived(new EstimateLookup({ run_id: runId }));
  $effect(() => {
    const current = lookup;
    void current.load();
    return () => current.close();
  });

  const job = $derived(lookup.job);
  const running = $derived(job?.state === 'running');
  const result = $derived(job?.state === 'done' ? job.result : null);
  const error = $derived(job?.state === 'failed' ? job.error : null);
  const blocker = $derived(lookup.blocker);
  const ignOf = (id: string | null) => job?.characters.find((c) => c.character_id === id)?.ign ?? null;

  // Before a lookup: who brings a character, and who will be listed as missing.
  const bringing = $derived(slots?.slots.filter((s) => s.picked ?? s.inferred) ?? []);
  const without = $derived(slots?.slots.filter((s) => !(s.picked ?? s.inferred)).map((s) => name(s.member_id)) ?? []);
  const nothing = $derived(slots !== null && bringing.length === 0);

  const off = $derived(running || Boolean(blocker) || lookup.starting || nothing || error?.code === 'manual_check' || lookup.wait > 0);
  const again = $derived(Boolean(result || error));
  const label = $derived.by(() => {
    if (running) return 'Looking up…';
    if (error?.code === 'failed') return 'Try again';
    if (again && error?.code !== 'manual_check' && error?.code !== 'busy') return 'Look up again';
    return 'Look up party';
  });

  const issues = $derived.by(() => {
    if (!job || !result) return [];
    const lines: string[] = [];
    const missing = job.missing.map((m) => name(m.member_id));
    if (missing.length) lines.push(`${joinNames(missing)} ${missing.length === 1 ? 'has' : 'have'} no character (missing, never 0%).`);
    for (const total of result.combined ?? []) {
      const boss = bosses.find((b) => b.token === total.boss);
      const title = boss ? `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}` : total.boss;
      for (const issue of total.issues) {
        const ign = ignOf(issue.character_id);
        if (issue.issue === 'missing_boss') lines.push(`${ign ?? 'A character'} has no ${title} card.`);
        else if (issue.issue === 'not_covered') lines.push(`MapleScouter has no ${title} card at all.`);
        else if (issue.issue === 'unknown_benchmark_players') lines.push(`${ign ?? 'A character'}’s ${title} card has an unknown party size.`);
        else lines.push(`${title}: ${ign ? `${ign}’s card` : 'a card'} can’t be counted.`);
      }
    }
    return lines;
  });
</script>

{#snippet button()}
  <button
    type="button"
    class="btn chardata__btn"
    data-fid="estimate-run"
    aria-disabled={off ? 'true' : undefined}
    aria-describedby={off ? `${uid}-why` : undefined}
    onclick={() => {
      if (!off) void lookup.start(again);
    }}><Icon name={again && label !== 'Look up party' ? 'refresh-cw' : 'gauge'} />{label}</button
  >
{/snippet}

{#snippet problem(lead: string, text: string)}
  <div class="settings__box settings__box--warn estimate__error" role="alert" data-fid="estimate-error">
    <b>{lead}</b>
    <span class="chardata__sub" id="{uid}-why">{text}</span>
  </div>
{/snippet}

{#if !chardataRoutes.estimatesGone && lookup.loaded}
  <!-- The phone's Party list ends with it (one more item); the laptop sheet's aside holds it. -->
  <svelte:element this={sheet ? 'li' : 'section'} class="estimate-panel" class:runsheet__estimate={sheet} aria-labelledby="{uid}-h" data-fid="estimate-panel">
    <h3 class="vh" id="{uid}-h">Estimate</h3>
    <div class="estimate__head">
      <span class="estimate__label" data-fid="estimate-label"><Icon name="gauge" />{job?.label ?? ESTIMATE_LABEL}</span>
      <span class="chardata__grow"></span>
      {#if !sheet}{@render button()}{/if}
    </div>
    <span class="chardata__sub" data-fid="estimate-disclaimer"
      ><b>{job?.note ?? ESTIMATE_NOTE}.</b> Admins only; members never see it. {#if result && job?.fetched_at}Looked up
        <span class="mono">{dayTime(job.fetched_at, timeZone)}</span>, kept 24 h in memory.{/if}</span
    >

    {#if blocker}
      {@render problem(
        'Another lookup is running.',
        `Only one lookup runs at a time (${actor(blocker.started_by)} started one for ${lookupTarget(blocker)} at ${hourMinute(blocker.started_at, timeZone)}). Try again when it ends.`,
      )}
    {:else if lookup.refused}
      {@render problem('Couldn’t look up the party.', lookup.refused.message)}
    {:else if running && job}
      <EstimateProgress
        data-fid="estimate-progress"
        done={job.progress.done}
        total={job.progress.total}
        who={job.progress.current
          ? `Looking up ${Math.min(job.progress.done + 1, job.progress.total)} of ${job.progress.total} · ${ignOf(job.progress.current) ?? ''}`
          : `Adding up ${job.progress.total} characters…`}
        elapsed={lookup.elapsed}
      />
    {:else if error && job}
      {@const words = estimateErrorText(error, ignOf(error.character_id), true)}
      {@render problem(words.lead, error.code === 'busy' && lookup.wait > 0 ? `It is serving another lookup. Try again in ${lookup.wait} s.` : words.text)}
    {:else if result && job}
      {#if result.combined?.length}
        <div class="estimate__combined" class:estimate__combined--one={sheet || result.combined.length === 1} data-fid="estimate-combined">
          {#each result.combined as total (total.boss)}
            {@const boss = bosses.find((b) => b.token === total.boss)}
            <div class="chardata__col">
              <span class="estimate__boss"
                ><b>{boss?.name ?? total.boss}</b>{#if boss}<span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>{/if}</span
              >
              <span class="chardata__value"
                >{#if total.estimated_minutes !== null}<span class="mono">{minutes(total.estimated_minutes)}</span><span class="chardata__sub"
                    >· <span class="mono">{percent(total.combined_percentage)}</span></span
                  >{:else}—<span class="chardata__sub">no total</span>{/if}</span
              >
              <span class="estimate__chips">
                {#if total.technically_possible === true}<StatusChip tone="ok"><Icon name="check" />technically possible</StatusChip>
                {:else if total.technically_possible === false}<StatusChip tone="warn"><Icon name="alert-triangle" />not technically possible</StatusChip>{/if}
                {#if !total.complete}<StatusChip tone="warn"><Icon name="alert-triangle" />not complete</StatusChip>{/if}
              </span>
            </div>
          {/each}
        </div>
      {/if}
      <table class="estimate__table" data-fid="estimate-members">
        <caption class="vh">Effective % per member</caption>
        <thead>
          <tr>
            <th scope="col">Member · effective %</th>
            {#each bosses as boss (boss.token)}<th scope="col" class="estimate__n"><abbr title="{boss.name} {DIFFICULTY_WORDS[boss.difficulty]}">{boss.token}</abbr></th>{/each}
          </tr>
        </thead>
        <tbody>
          {#each result.members as member (member.character_id)}
            {@const who = job.characters.find((c) => c.character_id === member.character_id)}
            <tr>
              <th scope="row" class="estimate__who"><b>{who ? name(who.member_id) : member.ign}</b> <span class="chardata__sub">{member.ign}</span></th>
              {#each bosses as boss (boss.token)}
                <td class="estimate__n">{percent(member.bosses.find((b) => b.boss === boss.token)?.effective_percentage ?? null)}</td>
              {/each}
            </tr>
          {/each}
          {#each job.missing as gap (gap.member_id)}
            <tr class="estimate__missing" data-fid="estimate-missing">
              <th scope="row" class="estimate__who"><b>{name(gap.member_id)}</b> <span>missing · {MISSING_REASONS[gap.reason]}</span></th>
              {#each bosses as boss (boss.token)}<td class="estimate__n">—</td>{/each}
            </tr>
          {/each}
        </tbody>
      </table>
      {#if issues.length}<p class="chardata__foot" data-fid="estimate-issues"><span><b>Issues:</b> {issues.join(' ')}</span></p>{/if}
    {:else if nothing}
      <span class="chardata__msg" id="{uid}-why">No one in this party has a character to look up yet.</span>
    {:else}
      <span class="chardata__msg"
        >Looks up the {bringing.length || ''} character{bringing.length === 1 ? '' : 's'} picked or inferred for this run (at most 6) and adds their effective % per boss.
        Takes up to 2 minutes.</span
      >
      {#if without.length}<span class="chardata__sub">{joinNames(without)} {without.length === 1 ? 'has' : 'have'} no character yet and will be listed as missing.</span>{/if}
    {/if}
    {#if sheet}<div class="chardata__actions">{@render button()}</div>{/if}
  </svelte:element>
{/if}
