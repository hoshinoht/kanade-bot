<!--
  The strip's MapleScouter row (boards B_CharEstimate, B_EstStates): one
  character's estimate per catalog boss, run as a job of up to 2 minutes.
  Labelled as MapleScouter's and as planning only; MapleScouter gets the
  roster IGN and nothing from NEXON. Drawn only while estimates are on (no
  scouter URL: the routes answer the generic 404 and nothing shows).
-->
<script lang="ts">
  import type { BossRow, CharacterRow, SettingsActor } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, Icon } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import EstimateProgress from './EstimateProgress.svelte';
  import { catalogBoss, clock, dayTime, ESTIMATE_LABEL, ESTIMATE_NOTE, estimateErrorText, hourMinute, lookupTarget, minutes, percent } from './format';
  import { chardataRoutes, EstimateLookup } from './routes.svelte';

  let {
    character,
    sheet,
    timeZone,
    actor,
  }: {
    character: CharacterRow;
    /** The phone sheet: the button under the row, 44 px. */
    sheet: boolean;
    timeZone: string;
    actor: (who: SettingsActor) => string;
  } = $props();

  const uid = $props.id();
  const lookup = $derived(new EstimateLookup({ character_ids: [character.id] }));
  $effect(() => {
    const current = lookup;
    void current.load();
    return () => current.close();
  });
  const catalog = new Resource<BossRow[]>('/api/admin/bosses');

  const job = $derived(lookup.job);
  const running = $derived(job?.state === 'running');
  const result = $derived(job?.state === 'done' ? job.result : null);
  $effect(() => {
    if (result && !catalog.data && !catalog.loading) void catalog.load();
  });
  const rows = $derived(
    (result?.members[0]?.bosses ?? [])
      .map((b) => ({ ...b, at: catalogBoss(catalog.data ?? [], b.boss) }))
      .filter((b) => b.at)
      .sort((a, b) => (b.effective_percentage ?? -1) - (a.effective_percentage ?? -1)),
  );
  const error = $derived(job?.state === 'failed' ? job.error : null);
  const blocker = $derived(lookup.blocker);

  // The button follows the state: off while a lookup runs, MapleScouter waits for a check, or is busy.
  const off = $derived(running || Boolean(blocker) || lookup.starting || error?.code === 'manual_check' || lookup.wait > 0);
  // The shown reason (another lookup, a refusal, the job's error) says why the button is off.
  const why = $derived(blocker || lookup.refused || error ? `${uid}-why` : undefined);
  const label = $derived.by(() => {
    if (running) return 'Looking up…';
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
    }}><Icon name={again ? 'refresh-cw' : 'gauge'} />{label}</button
  >
{/snippet}

{#if !chardataRoutes.estimatesGone && lookup.loaded}
  <div class="estimate" data-fid="roster-estimate">
    <div class="estimate__head">
      <span class="estimate__label" data-fid="estimate-label"><Icon name="gauge" />{job?.label ?? ESTIMATE_LABEL}</span>
      <span class="chardata__grow"></span>
      {#if !sheet}{@render button()}{/if}
    </div>
    <span class="chardata__sub" data-fid="estimate-disclaimer"
      ><b>{job?.note ?? ESTIMATE_NOTE}.</b> Admins only. MapleScouter gets the roster IGN and nothing from NEXON.</span
    >
    {#if blocker}
      <div class="settings__box settings__box--warn estimate__error" role="alert" data-fid="estimate-error">
        <b>Another lookup is running.</b>
        <span class="chardata__sub" id="{uid}-why"
          >Only one lookup runs at a time ({actor(blocker.started_by)} started one for {lookupTarget(blocker)} at {hourMinute(blocker.started_at, timeZone)}). Try again
          when it ends.</span
        >
      </div>
    {:else if lookup.refused}
      <div class="settings__box settings__box--warn estimate__error" role="alert" data-fid="estimate-error">
        <b>Couldn’t look up {character.ign}.</b>
        <span class="chardata__sub" id="{uid}-why">{lookup.refused.message}</span>
      </div>
    {:else if running && job}
      <EstimateProgress data-fid="estimate-progress" done={job.progress.done} total={job.progress.total} who="Looking up {character.ign}…" elapsed={lookup.elapsed} />
    {:else if error}
      {@const words = estimateErrorText(error, character.ign, false)}
      <div class="settings__box settings__box--warn estimate__error" role="alert" data-fid="estimate-error">
        <b>{words.lead}</b>
        <span class="chardata__sub" id="{uid}-why"
          >{#if error.code === 'busy' && lookup.wait > 0}It is serving another lookup. Try again in {lookup.wait} s.{:else}{words.text}{/if}{#if error.code === 'manual_check'}
            Status: <a href="/config?section=character-data">Config › Character data</a>.{/if}</span
        >
      </div>
    {:else if result && job}
      <table class="estimate__table" data-fid="estimate-result">
        <caption class="vh">{ESTIMATE_LABEL} for {character.ign}</caption>
        <thead><tr><th scope="col">Boss</th><th scope="col" class="estimate__n">Effective</th><th scope="col" class="estimate__n">Est. time</th><th scope="col">Rating</th></tr></thead>
        <tbody>
          {#each rows as row (row.boss)}
            <tr>
              <th scope="row"
                ><span class="estimate__boss"><b>{row.at!.name}</b><span class="pill pill--{row.at!.difficulty}">{DIFFICULTY_WORDS[row.at!.difficulty].toUpperCase()}</span></span></th
              >
              <td class="estimate__n">{percent(row.effective_percentage)}</td>
              <td class="estimate__n">{minutes(row.estimated_minutes)}</td>
              <td>{row.rating}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <span class="chardata__foot"
        >Looked up <span class="mono">{dayTime(job.fetched_at ?? job.started_at, timeZone)}</span> {#if result.members[0]}(preset <span class="mono">{result.members[0].preset}</span>){/if} ·
        kept 24 h in memory only · bosses outside the catalog are left out</span
      >
    {:else}
      <span class="chardata__msg">No lookup in the last 24 h. A lookup takes up to 2 minutes.</span>
    {/if}
    {#if sheet}<div class="chardata__actions">{@render button()}</div>{/if}
  </div>
{/if}
