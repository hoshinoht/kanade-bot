<!--
  The strip's MapleScouter row (boards B_CharEstimate, B_EstStates): one
  character's estimate per catalog boss, run as a job of up to 2 minutes that
  may first wait its turn ("Queued · 2 ahead"; lookups never refuse one
  another). Labelled as MapleScouter's and as planning only, and "Assumes
  100% critical rate (actual N%)" when MapleScouter assumed it; MapleScouter
  gets the roster IGN and nothing from NEXON. Drawn only while estimates are
  on (no scouter URL: the routes answer the generic 404 and nothing shows).
-->
<script lang="ts">
  import type { BossRow, CharacterRow } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, Icon } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import EstimateProgress from './EstimateProgress.svelte';
  import { assumedCrit, catalogBoss, clock, dayTime, ESTIMATE_LABEL, ESTIMATE_NOTE, estimateErrorText, minutes, percent } from './format';
  import { activeJob, chardataRoutes, EstimateLookup } from './routes.svelte';

  let {
    character,
    sheet,
    timeZone,
  }: {
    character: CharacterRow;
    /** The phone sheet: the button under the row, 44 px. */
    sheet: boolean;
    timeZone: string;
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
  const active = $derived(activeJob(job));
  const queued = $derived(job?.state === 'queued');
  const result = $derived(job?.state === 'done' ? job.result : null);
  const member = $derived(result?.members[0] ?? null);
  const assumed = $derived(member ? assumedCrit(member) : null);
  $effect(() => {
    if (result && !catalog.data && !catalog.loading) void catalog.load();
  });
  const rows = $derived(
    (member?.bosses ?? [])
      .map((b) => ({ ...b, at: catalogBoss(catalog.data ?? [], b.boss) }))
      .filter((b) => b.at)
      .sort((a, b) => (b.effective_percentage ?? -1) - (a.effective_percentage ?? -1)),
  );
  const error = $derived(job?.state === 'failed' ? job.error : null);

  // The button follows the state: off while the lookup waits or runs, MapleScouter waits for a check, or is busy.
  const off = $derived(active || lookup.starting || error?.code === 'manual_check' || lookup.wait > 0);
  // The shown reason (a refusal, the job's error) says why the button is off.
  const why = $derived(lookup.refused || error ? `${uid}-why` : undefined);
  const label = $derived.by(() => {
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
    {#if lookup.refused}
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
      />
    {:else if error}
      {@const words = estimateErrorText(error, character.ign)}
      <div class="settings__box settings__box--warn estimate__error" role="alert" data-fid="estimate-error">
        <b>{words.lead}</b>
        <span class="chardata__sub" id="{uid}-why"
          >{#if error.code === 'busy' && lookup.wait > 0}It stayed busy with other lookups. Try again in {lookup.wait} s.{:else}{words.text}{/if}{#if error.code === 'manual_check'}
            Status: <a href="/config?section=character-data">Config › Character data</a>.{/if}</span
        >
      </div>
    {:else if result && job}
      {#if assumed}<span class="estimate__assumed"><Icon name="alert-triangle" />{assumed}</span>{/if}
      <table class="estimate__table" data-fid="estimate-result">
        <caption class="vh">{ESTIMATE_LABEL} for {character.ign}{assumed ? ` · ${assumed}` : ''}</caption>
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
        >Looked up <span class="mono">{dayTime(job.fetched_at ?? job.started_at, timeZone)}</span> {#if member}(preset <span class="mono">{member.preset}</span>){/if} ·
        kept 24 h in memory only · bosses outside the catalog are left out</span
      >
    {:else}
      <span class="chardata__msg">No lookup in the last 24 h. A lookup takes up to 2 minutes, after any ahead of it.</span>
    {/if}
    {#if sheet}<div class="chardata__actions">{@render button()}</div>{/if}
  </div>
{/if}
