<!--
  The run sheet's Estimate panel (boards B_CharEstimateRun, B_EstRunStates,
  phone twins): one scouter lookup over the characters the party brings
  (at most 6), as a job that may first wait its turn ("Queued · 2 ahead").
  Each member's row fills in as their lookup finishes; one that fails says why
  in its row and never stops the others. Per boss of the run: the combined
  minutes and % over the members found, "technically possible", "not
  complete"; once done, Who carries (`ShareChart`, boards in
  docs/research/2026-10-15-estimate-share-chart) between the totals and the
  table, whose rows carry each member's numbered colour. Members without a
  character are listed as missing, never 0%.
  Labelled as the scouter's and as planning only, and "Assumes 100% critical
  rate (actual N%)" for a member the scouter assumed it for. No scouter URL:
  no panel.
-->
<script lang="ts">
  import type { Boss, EstimateCharacter, RunCharacters } from '@kanade/api-types';
  import {
    activeJob,
    assumedCrit,
    dayTime,
    DIFFICULTY_WORDS,
    ESTIMATE_LABEL,
    ESTIMATE_NOTE,
    estimateErrorText,
    EstimateLookup,
    EstimateProgress,
    Icon,
    minutes,
    olderThan,
    percent,
    ShareChart,
    StatusChip,
    whoColour,
    type ShareMember,
  } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-data.scss';
  import { joinNames, MISSING_REASONS } from './format';
  import { adminEstimateApi, chardataRoutes, estimateStep } from './routes.svelte';

  let {
    runId,
    bosses,
    slots,
    name,
    timeZone,
    sheet,
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
  } = $props();

  const uid = $props.id();
  const lookup = $derived(new EstimateLookup(adminEstimateApi({ run_id: runId }), chardataRoutes, estimateStep));
  $effect(() => {
    const current = lookup;
    void current.load();
    return () => current.close();
  });

  const job = $derived(lookup.job);
  const active = $derived(activeJob(job));
  const queued = $derived(job?.state === 'queued');
  const result = $derived(job?.result ?? null);
  const done = $derived(job?.state === 'done');
  // Failed: nobody was found. Done with an error: the party total itself failed.
  const error = $derived(job?.state === 'failed' ? job.error : null);
  const totalError = $derived(done ? (job?.error ?? null) : null);
  const failed = $derived(job?.characters.filter((c) => c.outcome === 'failed') ?? []);
  const memberOf = (c: EstimateCharacter) => result?.members.find((m) => m.character_id === c.character_id) ?? null;
  const assumedNames = $derived(result?.members.filter((m) => m.critical_rate_assumed).map((m) => m.ign) ?? []);
  // Who carries, once done: party order (the table's rows), each looked-up character numbered by its row.
  const shareMembers = $derived.by((): ShareMember[] => {
    if (!job || !done || !result) return [];
    const found: ShareMember[] = job.characters.map((c, i) => {
      const member = memberOf(c);
      return {
        id: c.member_id,
        name: name(c.member_id),
        ign: c.ign,
        n: i + 1,
        effective: Object.fromEntries(member?.bosses.map((b) => [b.boss, b.effective_percentage]) ?? []),
        assumed: member?.critical_rate_assumed ?? false,
      };
    });
    return [...found, ...job.missing.map((gap) => ({ id: gap.member_id, name: name(gap.member_id), ign: null, n: null, effective: {}, assumed: false }))];
  });
  const shareBosses = $derived(bosses.map((boss) => ({ token: boss.token, title: `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}` })));

  // Before a lookup: who brings a character, and who will be listed as missing.
  const bringing = $derived(slots?.slots.filter((s) => s.brings) ?? []);
  const without = $derived(slots?.slots.filter((s) => !s.brings).map((s) => name(s.member_id)) ?? []);
  const nothing = $derived(slots !== null && bringing.length === 0);

  const off = $derived(active || lookup.starting || nothing || error?.code === 'manual_check' || lookup.wait > 0);
  const again = $derived(Boolean((done && job?.result) || error));
  // A member that failed is asked again with the others answered from memory.
  const retry = $derived(error?.code === 'failed' || (done && failed.length > 0));
  const label = $derived.by(() => {
    if (queued) return 'Queued…';
    if (active) return 'Looking up…';
    if (retry) return 'Try again';
    if (again && error?.code !== 'manual_check' && error?.code !== 'busy') return 'Look up again';
    return 'Look up party';
  });

  const who = $derived.by(() => {
    if (!job) return '';
    const current = job.characters.find((c) => c.character_id === job.progress.current);
    if (current) return `Looking up ${Math.min(job.progress.done + 1, job.progress.total)} of ${job.progress.total} · ${current.ign}`;
    return job.progress.done < job.progress.total ? `Looking up ${job.progress.total} characters…` : `Adding up ${job.progress.total} characters…`;
  });

  const issues = $derived.by(() => {
    if (!job || !result || !done) return [];
    const lines: string[] = [];
    const missing = job.missing.map((m) => name(m.member_id));
    if (missing.length) lines.push(`${joinNames(missing)} ${missing.length === 1 ? 'has' : 'have'} no character (missing, never 0%).`);
    if (failed.length) lines.push(`${joinNames(failed.map((c) => c.ign))} couldn’t be looked up: the totals leave ${failed.length === 1 ? 'them' : 'them all'} out.`);
    if (assumedNames.length && result.combined?.length)
      lines.push(`The totals count ${joinNames(assumedNames)} at an assumed 100% critical rate.`);
    for (const total of result.combined ?? []) {
      const boss = bosses.find((b) => b.token === total.boss);
      const title = boss ? `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}` : total.boss;
      for (const issue of total.issues) {
        const ign = job.characters.find((c) => c.character_id === issue.character_id)?.ign ?? null;
        if (issue.issue === 'missing_boss') lines.push(`${ign ?? 'A character'} has no ${title} card.`);
        else if (issue.issue === 'not_covered') lines.push(`The scouter has no ${title} card at all.`);
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
      if (!off) void lookup.start(again && !retry);
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
      ><b>{job?.note ?? ESTIMATE_NOTE}.</b> Members see their own and their run party’s estimates. {#if done && job?.fetched_at}Looked up
        <span class="mono">{dayTime(job.fetched_at, timeZone)}</span>{job.stale ? olderThan(job.fresh_days) : ''}.{/if}</span
    >

    {#if lookup.refused}
      {@render problem('Couldn’t look up the party.', lookup.refused.message)}
    {:else if active && job}
      <EstimateProgress data-fid="estimate-progress" done={job.progress.done} total={job.progress.total} {who} elapsed={lookup.elapsed} queued={queued ? job.position : null} />
    {:else if error && job}
      {@const words = estimateErrorText(error, job.characters.find((c) => c.character_id === error.character_id)?.ign ?? null)}
      {@render problem(
        failed.length > 1 ? 'No one in the party could be looked up.' : words.lead,
        error.code === 'busy' && lookup.wait > 0 ? `It stayed busy with other lookups. Try again in ${lookup.wait} s.` : words.text,
      )}
    {:else if totalError}
      {@const words = estimateErrorText(totalError, null)}
      {@render problem('No party total.', `The members below were found, but the scouter’s total didn’t come back: ${words.lead} ${words.text}`)}
    {/if}

    {#if job && (result || failed.length || active) && !lookup.refused}
      {#if done && result?.combined?.length}
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
      {#if shareMembers.length}
        <ShareChart bosses={shareBosses} members={shareMembers} fids={{ missing: 'estimate-share-missing' }} data-fid="estimate-share" />
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
          {#each job.characters as c, i (c.character_id)}
            {@const member = memberOf(c)}
            {#if member}
              {@const assumed = assumedCrit(member)}
              <tr>
                <th scope="row" class="estimate__who"
                  >{#if shareMembers.length}<span
                      class="share__sw share--who-{whoColour(i + 1)}"
                      class:share__sw--assumed={member.critical_rate_assumed}
                      aria-hidden="true">{i + 1}</span
                    >{/if}<b>{name(c.member_id)}</b> <span class="chardata__sub">{member.ign}</span>{#if assumed}<span
                      class="estimate__who-note estimate__assumed"
                      data-fid="estimate-assumed"
                      ><Icon name="alert-triangle" />{assumed}</span
                    >{/if}</th
                >
                {#each bosses as boss (boss.token)}
                  <td class="estimate__n">{percent(member.bosses.find((b) => b.boss === boss.token)?.effective_percentage ?? null)}</td>
                {/each}
              </tr>
            {:else if c.outcome === 'failed' && c.error}
              {@const words = estimateErrorText(c.error, c.ign)}
              <tr class="estimate__failed">
                <th scope="row" class="estimate__who"
                  >{#if shareMembers.length}<span class="share__sw share__sw--none" aria-hidden="true"></span>{/if}<b>{name(c.member_id)}</b> <span class="chardata__sub"
                    >{c.ign}</span
                  ><span class="estimate__who-note" title={words.text}
                    >failed · {words.lead}<span class="vh"> {words.text}</span></span
                  ></th
                >
                {#each bosses as boss (boss.token)}<td class="estimate__n">—</td>{/each}
              </tr>
            {:else}
              <tr class="estimate__pending">
                <th scope="row" class="estimate__who"
                  ><b>{name(c.member_id)}</b> <span class="chardata__sub">{c.ign}</span><span class="estimate__who-note"
                    >{job.progress.current === c.character_id ? 'looking up…' : queued ? 'queued' : 'waiting'}</span
                  ></th
                >
                {#each bosses as boss (boss.token)}<td class="estimate__n">…</td>{/each}
              </tr>
            {/if}
          {/each}
          {#each job.missing as gap (gap.member_id)}
            <tr class="estimate__missing" data-fid="estimate-missing">
              <th scope="row" class="estimate__who"
                >{#if shareMembers.length}<span class="share__sw share__sw--none" aria-hidden="true"></span>{/if}<b>{name(gap.member_id)}</b>
                <span>missing · {MISSING_REASONS[gap.reason]}</span></th
              >
              {#each bosses as boss (boss.token)}<td class="estimate__n">—</td>{/each}
            </tr>
          {/each}
        </tbody>
      </table>
      {#if issues.length}<p class="chardata__foot" data-fid="estimate-issues"><span><b>Issues:</b> {issues.join(' ')}</span></p>{/if}
    {:else if nothing}
      <span class="chardata__msg" id="{uid}-why">No one in this party has a character to look up yet.</span>
    {:else if !lookup.refused && !error}
      <span class="chardata__msg"
        >Looks up the {bringing.length || ''} character{bringing.length === 1 ? '' : 's'} this run brings (at most 6) and adds their effective % per boss.
        Takes up to 2 minutes, after any lookup ahead of it.</span
      >
      {#if without.length}<span class="chardata__sub">{joinNames(without)} {without.length === 1 ? 'has' : 'have'} no character yet and will be listed as missing.</span>{/if}
    {/if}
    {#if sheet}<div class="chardata__actions">{@render button()}</div>{/if}
  </svelte:element>
{/if}
