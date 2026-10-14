<!--
  The strip's estimate row (boards B_CharEstimate, B_EstStates; the
  portal's M_EstStates*): one character's estimate per catalog boss, run as a
  job of up to 2 minutes that may first wait its turn ("Queued · 2 ahead";
  lookups never refuse one another). Labelled as the scouter's and as
  planning only, and "Assumes 100% critical rate (actual N%)" when the
  scouter assumed it; the scouter gets the roster IGN and nothing from
  NEXON. Drawn only while estimates are on (no scouter URL: the routes answer
  the generic 404 and nothing shows). A member starts one lookup per 30
  minutes: refused, the button names the next time and the last result stays.
-->
<script lang="ts">
  import type { CharacterRow } from '@kanade/api-types';
  import { dayTime } from '../account';
  import Icon from '../components/Icon.svelte';
  import { clockTime, DIFFICULTY_WORDS } from '../format';
  import { assumedCrit, catalogBoss, clock, ESTIMATE_LABEL, ESTIMATE_NOTE, estimateErrorText, minutes, percent } from './chardata';
  import EstimateProgress from './EstimateProgress.svelte';
  import { activeJob, EstimateLookup, type CharacterFids, type CharacterKit } from './kit.svelte';

  let {
    kit,
    character,
    sheet,
    fids = {},
  }: {
    kit: CharacterKit;
    character: CharacterRow;
    /** The phone sheet: the button under the row, 44 px; boss tokens ("HLimbo") so four columns fit. */
    sheet: boolean;
    fids?: CharacterFids;
  } = $props();

  const uid = $props.id();
  const member = $derived(kit.audience === 'member');
  const lookup = $derived(new EstimateLookup(kit.api.estimate(character.id), kit.routes, kit.onstep));
  $effect(() => {
    const current = lookup;
    void current.load();
    return () => current.close();
  });

  const job = $derived(lookup.job);
  const active = $derived(activeJob(job));
  const queued = $derived(job?.state === 'queued');
  const result = $derived(job?.state === 'done' ? job.result : null);
  const found = $derived(result?.members[0] ?? null);
  const assumed = $derived(found ? assumedCrit(found) : null);
  $effect(() => {
    if (result && !kit.catalog.data && !kit.catalog.loading) void kit.catalog.load();
  });
  const rows = $derived(
    (found?.bosses ?? [])
      .map((b) => ({ ...b, at: catalogBoss(kit.catalog.data ?? [], b.boss) }))
      .filter((b) => b.at)
      .sort((a, b) => (b.effective_percentage ?? -1) - (a.effective_percentage ?? -1)),
  );
  const error = $derived(job?.state === 'failed' ? job.error : null);
  // A member's 30-minute limit: the last result stays, the button names the time.
  const limited = $derived(lookup.refused?.code === 'member_limited' ? lookup.refused : null);
  const nextAt = $derived(limited?.nextAt ? clockTime(limited.nextAt, kit.timeZone, false) : null);

  // The button follows the state: off while the lookup waits or runs, the scouter waits for a check, is busy, or the member's limit holds.
  const off = $derived(active || lookup.starting || error?.code === 'manual_check' || lookup.wait > 0 || Boolean(limited));
  // The shown reason (a refusal, the job's error) says why the button is off.
  const why = $derived(limited ? `${uid}-next` : lookup.refused || error ? `${uid}-why` : undefined);
  const label = $derived.by(() => {
    if (limited) return `Next lookup at ${nextAt ?? 'a later time'}`;
    if (queued) return 'Queued…';
    if (active) return 'Looking up…';
    if (lookup.wait > 0) return `Look up · ${clock(lookup.wait)}`;
    if (error?.code === 'failed') return 'Try again';
    if (result || error?.code === 'critical_rate') return 'Look up again';
    return 'Look up';
  });
  const again = $derived(Boolean(result || error));
</script>

{#snippet button()}
  <button
    type="button"
    class="btn chardata__btn"
    data-fid="estimate-run"
    aria-disabled={off ? 'true' : undefined}
    aria-describedby={off ? why : undefined}
    onclick={() => {
      if (!off) void lookup.start(again);
    }}><Icon name={limited ? 'clock' : again ? 'refresh-cw' : 'gauge'} />{label}</button
  >
{/snippet}

{#snippet resultTable()}
  {#if job}
    <table class="estimate__table" data-fid="estimate-result">
      <caption class="vh">{ESTIMATE_LABEL} for {character.ign}{assumed ? ` · ${assumed}` : ''}</caption>
      <thead><tr><th scope="col">Boss</th><th scope="col" class="estimate__n">Effective</th><th scope="col" class="estimate__n">Est. time</th><th scope="col">Rating</th></tr></thead>
      <tbody>
        {#each rows as row (row.boss)}
          <tr>
            <th scope="row"
              ><span class="estimate__boss"
                >{#if member && sheet}<b title="{DIFFICULTY_WORDS[row.at!.difficulty]} {row.at!.name}">{row.boss}</b>{:else}<b>{row.at!.name}</b><span class="pill pill--{row.at!.difficulty}"
                    >{DIFFICULTY_WORDS[row.at!.difficulty].toUpperCase()}</span
                  >{/if}</span
              ></th
            >
            <td class="estimate__n">{percent(row.effective_percentage)}</td>
            <td class="estimate__n">{minutes(row.estimated_minutes)}</td>
            <td>{row.rating}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <span class="chardata__foot"
      >Looked up <span class="mono">{dayTime(job.fetched_at ?? job.started_at, kit.timeZone)}</span>{#if member}{` by ${kit.actor(job.started_by)}`}{/if}
      {#if found}(preset <span class="mono">{found.preset}</span>){/if} · kept 24 h in memory only · bosses outside the catalog are left out</span
    >
  {/if}
{/snippet}

{#if !kit.routes.estimatesGone && lookup.loaded}
  <div class="estimate" data-fid="roster-estimate">
    <div class="estimate__head">
      <span class="estimate__label" data-fid="estimate-label"><Icon name="gauge" />{job?.label ?? ESTIMATE_LABEL}</span>
      <span class="chardata__grow"></span>
      {#if !sheet}{@render button()}{/if}
    </div>
    <span class="chardata__sub" data-fid="estimate-disclaimer"
      >{#if member}<b>{job?.note ?? ESTIMATE_NOTE}.</b> Your own characters only. The scouter gets the roster IGN and nothing from NEXON; the admins and your party see the
        result.{:else}<b>{job?.note ?? ESTIMATE_NOTE}.</b> Members see their own and their run party’s estimates. The scouter gets the roster IGN and nothing from NEXON.{/if}</span
    >
    {#if limited}
      <p class="chardata__sub" id="{uid}-next" data-fid={fids.next}>
        You can start one lookup every 30 minutes. <b>Next lookup at {nextAt ?? 'a later time'}.</b>{#if result}&nbsp;This result stays until then.{/if}
      </p>
      {#if result}{@render resultTable()}{/if}
    {:else if lookup.refused}
      <div class="settings__box settings__box--warn estimate__error" role="alert" data-fid="estimate-error">
        <b>Couldn’t look up {character.ign}.</b>
        <span class="chardata__sub" id="{uid}-why">{lookup.refused.message}</span>
      </div>
    {:else if active && job}
      <EstimateProgress
        data-fid="estimate-progress"
        done={job.progress.done}
        total={job.progress.total}
        who="Looking up {character.ign}…"
        elapsed={lookup.elapsed}
        queued={queued ? job.position : null}
        {member}
      />
    {:else if error}
      {@const words = estimateErrorText(error, character.ign)}
      {@const text = member && error.code === 'failed' ? words.text.replace(/Nothing was kept\.$/, 'Nothing was kept, and this try doesn’t count toward your 30 minutes.') : words.text}
      <div class="settings__box settings__box--warn estimate__error" role="alert" data-fid="estimate-error">
        <b>{words.lead}</b>
        <span class="chardata__sub" id="{uid}-why"
          >{#if error.code === 'busy' && lookup.wait > 0}It stayed busy with other lookups. Try again in {lookup.wait} s.{:else}{text}{/if}{#if error.code === 'manual_check' && kit.configHref}
            Status: <a href={kit.configHref}>Config › Character data</a>.{/if}</span
        >
      </div>
    {:else if result && job}
      {#if assumed}<span class="estimate__assumed" data-fid={fids.assumed}><Icon name="alert-triangle" />{assumed}</span>{/if}
      {@render resultTable()}
    {:else}
      <span class="chardata__msg">No lookup in the last 24 h. A lookup takes up to 2 minutes, after any ahead of it.</span>
    {/if}
    {#if sheet}<div class="chardata__actions">{@render button()}</div>{/if}
  </div>
{/if}
