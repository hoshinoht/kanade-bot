<!--
  The run sheet's Estimate panel (boards B_CharEstimateRun, B_EstRunStates,
  phone twins; since the Party grid, B_RunBossEstimate and
  B_PhoneRunBossEstimate): one scouter lookup over the characters the party
  brings (every character brought to some boss, at most 6 per boss), as a job
  that may first wait its turn ("Queued · 2 ahead"). One member failing never
  stops the others: the issues line says who. Once done, Who carries
  (`ShareChart`): a bar per boss, each that boss's own party, numbered and
  coloured by member, with its key and, when a member brings another
  character to a later boss, a line saying who brings whom. The per-boss
  totals and each member's % live in the Party grid (its Party total row and
  cells). Labelled as the scouter's and as planning only. No scouter URL: no
  panel.
-->
<script lang="ts">
  import type { Boss, RunCharacters } from '@kanade/api-types';
  import {
    activeJob,
    dayTime,
    DIFFICULTY_WORDS,
    ESTIMATE_LABEL,
    ESTIMATE_NOTE,
    estimateErrorText,
    EstimateLookup,
    EstimateProgress,
    Icon,
    olderThan,
    ShareChart,
    type ShareMember,
  } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-data.scss';
  import { joinNames } from './format';
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
    /** The run's bosses, in order (Who carries' bars). */
    bosses: Boss[];
    /** Who brings which character to each boss (the run sheet's picks), for the counts before a lookup. */
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

  // Each boss's party: the job's parties, else (one party) everyone looked up.
  const partyOf = (token: string): string[] => job?.parties.find((p) => p.bosses.includes(token))?.character_ids ?? job?.characters.map((c) => c.character_id) ?? [];

  // Who carries, once done: one member each, in party order and numbered by it, with that boss's character's share per bar.
  const shareMembers = $derived.by((): ShareMember[] => {
    if (!job || !done || !result) return [];
    const order = [...new Set([...job.characters.map((c) => c.member_id), ...job.missing.map((m) => m.member_id)])];
    let n = 0;
    return order.map((member) => {
      const mine = job.characters.filter((c) => c.member_id === member);
      const effective: Record<string, number | null> = {};
      const assumed: Record<string, boolean> = {};
      const igns: Record<string, string> = {};
      for (const boss of bosses) {
        const character = mine.find((c) => partyOf(boss.token).includes(c.character_id));
        const found = character ? result.members.find((m) => m.character_id === character.character_id) : undefined;
        if (!character) continue;
        igns[boss.token] = character.ign;
        effective[boss.token] = found?.bosses.find((b) => b.boss === boss.token)?.effective_percentage ?? null;
        assumed[boss.token] = found?.critical_rate_assumed ?? false;
      }
      return { id: member, name: name(member), igns, n: mine.length ? ++n : null, effective, assumed };
    });
  });
  const shareBosses = $derived(bosses.map((boss) => ({ token: boss.token, title: `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}` })));
  // "Per boss: Asahi brings AsahiMech and Mika brings MikaNL to HMaleficStar; each bar is that boss's own party."
  const by = $derived.by(() => {
    if (!job || job.parties.length < 2) return null;
    const first = job.parties[0]!;
    const lines = job.parties.slice(1).flatMap((party) => {
      const swaps = party.character_ids
        .filter((id) => !first.character_ids.includes(id))
        .map((id) => job.characters.find((c) => c.character_id === id))
        .flatMap((c) => (c ? [`${name(c.member_id)} brings ${c.ign}`] : []));
      return swaps.length ? [`${joinNames(swaps)} to ${joinNames(party.bosses)}`] : [];
    });
    return lines.length ? `Per boss: ${lines.join('; ')}; each bar is that boss’s own party.` : null;
  });

  // Before a lookup: the characters brought to any boss, and who will be listed as missing.
  const bringing = $derived([...new Set(slots?.slots.flatMap((s) => (s.bosses.length ? s.bosses.map((b) => b.brings) : [s.brings])).filter((id): id is string => id !== null) ?? [])]);
  const without = $derived(slots?.slots.filter((s) => !s.brings).map((s) => name(s.member_id)) ?? []);
  const nothing = $derived(slots !== null && bringing.length === 0);

  const off = $derived(active || lookup.starting || nothing || error?.code === 'manual_check' || lookup.wait > 0);
  const again = $derived(Boolean((done && job?.result) || error));
  // A member that failed is asked again with the others answered from memory.
  const retry = $derived(error?.code === 'failed' || error?.code === 'assumption_failed' || (done && failed.length > 0));
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
    // Why, in the strip's words (the members table that said it per row is gone).
    for (const c of failed) if (c.error) lines.push(`${c.ign}: ${estimateErrorText(c.error, c.ign).lead}`);
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
        <span class="mono">{dayTime(job.fetched_at, timeZone)}</span>{job.stale ? olderThan(job.fresh_days) : ''}{job.parties.length > 1
          ? ` · ${job.characters.length} characters, a party per boss`
          : ''}.{/if}</span
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
      {@render problem('No party total.', `The members were found, but the scouter’s total didn’t come back: ${words.lead} ${words.text}`)}
    {/if}

    {#if job && done && result && !lookup.refused}
      {#if shareMembers.some((m) => m.n !== null)}
        <ShareChart bosses={shareBosses} members={shareMembers} {by} fids={{ keys: 'estimate-share-keys', by: 'estimate-share-by' }} data-fid="estimate-share" />
      {/if}
      {#if issues.length}<p class="chardata__foot" data-fid="estimate-issues"><span><b>Issues:</b> {issues.join(' ')}</span></p>{/if}
    {:else if nothing}
      <span class="chardata__msg" id="{uid}-why">No one in this party has a character to look up yet.</span>
    {:else if !lookup.refused && !error && !active}
      <span class="chardata__msg"
        >Looks up the {bringing.length || ''} character{bringing.length === 1 ? '' : 's'} this run brings (at most 6 per boss) and adds their effective % per boss.
        Takes up to 2 minutes, after any lookup ahead of it.</span
      >
      {#if without.length}<span class="chardata__sub">{joinNames(without)} {without.length === 1 ? 'has' : 'have'} no character yet and will be listed as missing.</span>{/if}
    {/if}
    {#if sheet}<div class="chardata__actions">{@render button()}</div>{/if}
  </svelte:element>
{/if}
