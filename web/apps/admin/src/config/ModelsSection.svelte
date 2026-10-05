<!--
  Three model roles, each a base model from Kanata's live list with a
  reasoning level resolved against its published efforts (every level when it
  publishes none; inherit only when the extraction effort fits; no `off` where
  the model requires reasoning). Capacity groups are read-only (kanade.toml):
  one row per group, the server's startup check, and what Kanata admits.
-->
<script lang="ts">
  import type { ConfigView, ModelInfo, ModelRole, RoleModel } from '@kanade/api-types';
  import { CHECK_TONE, Icon, Select, WavyProgress } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import {
    groupCap,
    groupRows,
    isReasoningValid,
    kanataLimits,
    keyLine,
    modelOptions,
    reasoningChoices,
    resetStrandedInheritors,
    ROLES,
    rowCheckText,
    splitChecks,
    wavingRows,
  } from './capacity';
  import ContextWindows from './ContextWindows.svelte';
  import { changes, type Change } from './dirty';
  import PillTabs from './PillTabs.svelte';
  import { followSaved } from './follow.svelte';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';

  let { models, save }: { models: ConfigView['models']; save: Save } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  let roles = $state<Record<ModelRole, RoleModel>>(structuredClone($state.snapshot(models.roles)));
  followSaved(
    () => structuredClone($state.snapshot(models.roles)),
    () => roles,
    (saved) => (roles = saved),
  );
  let rolesError = $state('');
  let saving = $state(false);
  let tab = $state('roles');
  let contextEditor: { discard: () => void } | undefined = $state();
  let contextChanges = $state<Change[]>([]);
  let contextSaving = $state(false);
  const level = (role: ModelRole, value: string) => (value === '' ? (role === 'extraction' ? 'Off' : 'Same as extraction') : value);
  const pending = $derived(
    changes(
      ROLES.flatMap(({ id, name }) => [
        { label: `${name} model`, from: models.roles[id].alias || 'none', to: roles[id].alias || 'none' },
        { label: `${name} reasoning`, from: level(id, models.roles[id].reasoning), to: level(id, roles[id].reasoning) },
      ]),
    ),
  );

  function discard() {
    roles = structuredClone($state.snapshot(models.roles));
    resetNote = '';
    rolesError = '';
  }
  // Said in the section, next to the fields: which inheriting role was reset and why.
  let resetNote = $state('');

  const info = (alias: string): ModelInfo | undefined => models.catalog.find((m) => m.id === alias);
  const MARK = { ok: 'check', warning: 'alert-circle', error: 'alert-triangle' } as const;
  const TONE = CHECK_TONE;
  const groups = $derived(groupRows(models));
  // Bars wave while calls hold the group's permits, two at most (as on Limits).
  const waving = $derived(wavingRows(groups));
  // The server runs the startup check on every read and save; it is the truth.
  // Groups are read-only (kanade.toml), so no unsaved edit changes them and the
  // saved verdicts are the whole story: each sits on its group's row, and
  // cross-group ones (a role in no group, Kanata unreachable) under the table.
  const checks = $derived(splitChecks(models.capacity_check, groups.map((g) => g.group)));
  const limits = $derived(kanataLimits(models));
  const permitsTotal = $derived(groups[0]?.permits ?? 0);
  function pick(role: ModelRole, alias: string) {
    // A base pick replaces a stored variant, and with it the baked-in level.
    roles[role] = { alias, reasoning: roles[role].reasoning };
    // A level the new alias does not publish would strand the role; the
    // server would reset it with a notice, so reset it here instead.
    const current = roles[role].reasoning;
    const resolved = current === '' ? roles.extraction.reasoning : current;
    // As the server: `off` where allowed, else the lowest published level.
    const next = info(alias);
    if (!isReasoningValid(next, resolved)) roles[role].reasoning = next?.off_allowed === false ? (next.reasoning_efforts?.[0] ?? 'off') : 'off';
    noteResets();
  }

  function setReasoning(role: ModelRole, value: string) {
    roles[role].reasoning = value;
    noteResets();
  }

  // Extraction's effort is what "Same as extraction" resolves to: a change
  // there must not leave a role inheriting a level its model does not publish.
  function noteResets() {
    const reset = resetStrandedInheritors(roles, models.catalog);
    if (!reset.length) resetNote = '';
    else
      resetNote = `${reset.map((r) => ROLES.find((x) => x.id === r)!.name).join(' and ')} reasoning reset to Off: ${roles.extraction.reasoning} is not published for ${reset.map((r) => roles[r].alias).join(' / ')}.`;
  }

  async function saveRoles(event: SubmitEvent) {
    event.preventDefault();
    // Only the writable fields; an unset role sends its level alone.
    const body = Object.fromEntries(
      ROLES.map(({ id }) => [id, roles[id].alias ? { alias: roles[id].alias, reasoning: roles[id].reasoning } : { reasoning: roles[id].reasoning }]),
    ) as Record<ModelRole, { alias?: string; reasoning: string }>;
    saving = true;
    rolesError = await save({ models: { roles: body } }, 'Models saved; the next question uses them.');
    saving = false;
    // The server's answer is the truth (it may reset stranded levels): resync.
    if (!rolesError) {
      roles = structuredClone($state.snapshot(models.roles));
      resetNote = '';
    }
  }
</script>

<SettingsPanel title="Models">
  {#snippet lead()}Three roles, each a base model from Kanata’s live list.{/snippet}
  {#snippet tabs()}
    <PillTabs
      id={uid}
      label="Models"
      bind:selected={tab}
      tabs={[
        { key: 'roles', label: 'Roles' },
        { key: 'context', label: 'Context windows' },
        { key: 'capacity', label: 'Capacity' },
      ]}
    />
  {/snippet}
  {#if !models.reachable}
    <p class="settings__box settings__box--risk" role="alert">Kanata's model list is unreachable, so the saved choices are shown but cannot be changed.</p>
  {/if}
  <div class="settings__tabpanel" role="tabpanel" id="{uid}-panel-roles" aria-labelledby="{uid}-tab-roles" hidden={tab !== 'roles'}>
    <form class="models__roles" id="{uid}-roles" data-fid="cfg-roles" onsubmit={saveRoles}>
      {#each ROLES as role (role.id)}
        {@const chosen = info(roles[role.id].alias)}
        {@const unset = !roles[role.id].alias}
        {@const fixed = roles[role.id].variant_of ? roles[role.id].fixed_effort : undefined}
        {@const unlisted = !chosen}
        {@const saved = models.roles[role.id]}
        <fieldset class="settings__card models__role" data-fid="cfg-role" aria-describedby="{uid}-{role.id}-job">
          <legend class="settings__cardtitle">{role.name}</legend>
          <p class="settings__cardnote" id="{uid}-{role.id}-job">{role.job}</p>
          <div class="field"
            ><span>Model</span>
            <Select
              label="Model"
              value={roles[role.id].alias}
              class={roles[role.id].alias !== saved.alias ? 'settings__changed' : ''}
              options={modelOptions(role.id, roles[role.id], models.catalog)}
              noun="models"
              onchange={(alias) => pick(role.id, alias)}
              disabled={!models.reachable}
            />
          </div>
          <div class="field"
            ><span>Reasoning</span>
            {#if fixed}
              <!-- A variant bakes its level in; it wins over any choice here. -->
              <Select label="Reasoning" value="fixed" options={[{ value: 'fixed', label: `Fixed: ${fixed}` }]} disabled />
            {:else}
              <Select
                label="Reasoning"
                value={roles[role.id].reasoning}
                class={roles[role.id].reasoning !== saved.reasoning ? 'settings__changed' : ''}
                options={reasoningChoices(role.id, chosen, roles.extraction.reasoning, roles[role.id].reasoning)}
                onchange={(level) => setReasoning(role.id, level)}
                disabled={!models.reachable || unlisted}
              />
            {/if}
          </div>
          {#if chosen}
            <!-- Treat unknown trust metadata and the -cloud suffix as external for data-flow warnings. -->
            {@const unzoned = chosen.trust_zone !== 'homelab' && chosen.trust_zone !== 'external'}
            {@const untrusted = chosen.leaves_homelab || chosen.trust_zone !== 'homelab' || chosen.id.endsWith('-cloud')}
            <ul class="settings__caps" aria-label="What {chosen.id} can do">
              <li class="capchip capchip--{untrusted ? 'ext' : 'home'}">
                {unzoned ? 'trust zone unknown' : untrusted ? 'leaves the homelab' : 'homelab'}
              </li>
              <li class="capchip">{chosen.function_tools ? 'calls tools' : 'no tools'}</li>
              <li class="capchip">{chosen.structured_output ? 'structured output' : 'free text only'}</li>
              <li class="capchip">{chosen.sampling_controls ? 'sampling' : 'fixed sampling'}</li>
              <li class="capchip">
                {chosen.reasoning_efforts === null ? 'reasoning: any level' : chosen.reasoning_efforts.length ? `reasoning: ${chosen.reasoning_efforts.join(', ')}` : 'no reasoning control'}
              </li>
              {#if chosen.admission}
                <li class="capchip capchip--mono">admits {chosen.admission.max_in_flight}{chosen.admission.adapter_max_in_flight != null ? ` (adapter ${chosen.admission.adapter_max_in_flight})` : ''}</li>
              {:else}
                <li class="capchip capchip--mono">no published limit</li>
              {/if}
            </ul>
            {#if untrusted}
              <p class="settings__box settings__box--warn settings__box--small settings__warn">
                <Icon name="alert-triangle" />
                <span>
                  {#if unzoned}
                    <strong>{chosen.id}</strong> publishes no trust zone, so it is treated as an external route.
                  {:else}
                    Requests to <strong>{chosen.id}</strong> go to an external provider.
                  {/if}
                  Raw member names, IDs, messages, and URLs leave the homelab with every request.
                </span>
              </p>
            {:else}
              <p class="settings__box settings__box--small"><Icon name="check" /><span>Stays in the homelab.</span></p>
            {/if}
          {:else if unset}
            <p class="settings__box settings__box--small">Not configured: this role has no model, so its work is skipped.</p>
          {:else}
            <p class="settings__box settings__box--warn settings__box--small settings__warn">
              <Icon name="alert-triangle" />
              <span>
                <strong>{roles[role.id].alias}</strong> is not in Kanata's list, so its availability is unknown and it is treated as external. If
                routed, raw member names, IDs, messages, and URLs are sent outside the homelab; requests may fail if the alias is unavailable.
              </span>
            </p>
          {/if}
        </fieldset>
      {/each}
    </form>
    {#if resetNote}<p class="settings__box settings__box--warn settings__warn" role="status"><Icon name="alert-circle" /><span>{resetNote}</span></p>{/if}
    {#if rolesError}<p class="field__error" role="alert">{rolesError}</p>{/if}
  </div>
  <div class="settings__tabpanel" role="tabpanel" id="{uid}-panel-context" aria-labelledby="{uid}-tab-context" hidden={tab !== 'context'}>
    <ContextWindows bind:this={contextEditor} bind:changes={contextChanges} bind:saving={contextSaving} form="{uid}-context" {models} {save} />
  </div>
  <div class="settings__tabpanel" role="tabpanel" id="{uid}-panel-capacity" aria-labelledby="{uid}-tab-capacity" hidden={tab !== 'capacity'}>
    <section class="settings__card" data-fid="cfg-card" aria-labelledby="{uid}-groups">
      <div class="settings__cardhead">
        <h4 class="settings__cardtitle" id="{uid}-groups">Capacity groups</h4>
        <span class="settings__cardnote">Each group shares its permits, held to the least Kanata admits for any of its models.</span>
      </div>
      <div class="models__wrap">
        <table class="settings__table models__groups">
          <caption class="settings__cardnote">
            {models.groups_source === 'config'
              ? 'Set in kanade.toml under [[models.groups]]; restart to apply.'
              : `Every model shares one group of ${permitsTotal} permit${permitsTotal === 1 ? '' : 's'} (models.permits in kanade.toml).`}
          </caption>
          <thead
            ><tr
              ><th scope="col">Group</th><th scope="col" class="num">Permits</th><th scope="col">Models</th><th scope="col" class="models__check">Startup check</th></tr
            ></thead
          >
          <tbody>
            {#each groups as g (g.group)}
              {@const cap = g.permits === null ? null : groupCap(models, g.group)}
              <tr>
                <th scope="row" class="mono">{g.group}</th>
                <td class="num mono"
                  >{g.permits ?? '—'}{#if cap !== null && g.permits !== null}
                    <!-- Usually full (permits match Kanata), so a busy bar keeps waving at 100%. -->
                    <WavyProgress
                      class="wavy--inline models__cap"
                      value={Math.min(g.permits, cap)}
                      max={cap}
                      wavy={waving.has(g.group)}
                      fullWave
                      tone={g.permits > cap ? 'warn' : 'accent'}
                      label="{g.group} permits of what Kanata admits"
                      text="{g.permits} of the {cap} Kanata admits{g.inUse ? `, ${g.inUse} in flight` : ''}"
                    />{/if}</td
                >
                <td><span class="settings__caps">{#each g.models as m (m)}<span class="capchip capchip--mono">{m}</span>{/each}</span></td>
                <td class="models__check">
                  {#each checks.byGroup.get(g.group) ?? [] as c, i (i)}
                    <span class="models__verdict"><span class="tone tone--{TONE[c.level]}"><Icon name={MARK[c.level]} label={c.level} /></span><span>{rowCheckText(c.message, g.group)}</span></span>
                  {:else}
                    <span class="note">—<span class="vh"> no check reported</span></span>
                  {/each}
                </td>
              </tr>
            {:else}
              <tr><td colspan="4" class="note">No model has a group, so no calls can run.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if checks.rest.length}
        <ul class="settings__checks" aria-label="Startup checks across groups">
          {#each checks.rest as c, i (i)}
            <li><span class="tone tone--{TONE[c.level]}"><Icon name={MARK[c.level]} label={c.level} /></span><span>{c.message}</span></li>
          {/each}
        </ul>
      {/if}
      {#if limits.uniform !== null}
        <p class="settings__cardnote">Kanata admits {limits.uniform} call{limits.uniform === 1 ? '' : 's'} at a time per model.</p>
      {:else if limits.all.length}
        <table class="settings__table models__groups">
          <caption class="settings__cardnote">What Kanata admits for the models in use</caption>
          <thead><tr><th scope="col">Model</th><th scope="col" class="num">Calls at a time</th></tr></thead>
          <tbody>
            {#each limits.inUse as l (l.alias)}
              <tr><th scope="row" class="mono">{l.alias}</th><td class="num">{l.max}{l.declared ? ' (declared)' : ''}</td></tr>
            {/each}
          </tbody>
        </table>
        {#if limits.all.length > limits.inUse.length}
          <details class="settings__more">
            <summary>Show all models</summary>
            <table class="settings__table models__groups">
              <caption class="vh">What Kanata admits for every listed model</caption>
              <thead><tr><th scope="col">Model</th><th scope="col" class="num">Calls at a time</th></tr></thead>
              <tbody>
                {#each limits.all as l (l.alias)}
                  <tr><th scope="row" class="mono">{l.alias}</th><td class="num">{l.max}{l.declared ? ' (declared)' : ''}</td></tr>
                {/each}
              </tbody>
            </table>
          </details>
        {/if}
      {/if}
      <p class="settings__cardnote">{keyLine(models.key_limits)}</p>
    </section>
  </div>
  {#snippet bar()}
    {#if tab === 'roles'}
      <SaveBar form="{uid}-roles" label="Save models" changes={pending} {saving} disabled={!models.reachable} ondiscard={discard} />
    {:else if tab === 'context'}
      <SaveBar form="{uid}-context" label="Save context windows" changes={contextChanges} saving={contextSaving} disabled={!models.reachable} ondiscard={() => contextEditor?.discard()} />
    {/if}
  {/snippet}
</SettingsPanel>

<style>
  /* Board B_CfgModels: three role cards side by side. */
  .models__roles {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 0.75rem;
    margin: 0;
  }

  .models__role {
    gap: 0.5rem;
    min-width: 0;
    margin: 0;
    padding: 0.75rem 0.875rem;
    border: 0;
  }

  .models__role > legend {
    float: left;
    width: 100%;
    margin: 0;
    padding: 0;
  }

  .models__role > legend + * {
    clear: both;
  }

  .models__groups {
    width: 100%;
  }

  /* Under the permit count: a short flat bar of what Kanata admits. */
  .models__groups :global(.models__cap) {
    inline-size: 4.5rem;
    min-inline-size: 0;
    margin-inline-start: auto;
    margin-block-start: 4px;
  }

  /* Phones scroll the groups table sideways rather than clip its verdicts. */
  .models__wrap {
    min-width: 0;
    overflow-x: auto;
  }

  /* Board B_CfgModels: the group's verdict beside its models. */
  .models__check {
    width: 18rem;
  }

  td.models__check {
    padding-block: 0.4rem;
  }

  .models__verdict {
    display: flex;
    gap: 0.45rem;
    align-items: flex-start;
    line-height: 1.35;
  }

  .models__verdict + .models__verdict {
    margin-top: 0.25rem;
  }

  /* The board's bare ✓: the mark in its tone's colour, no chip around it. */
  .models__verdict .tone {
    flex: none;
    min-height: 0;
    margin-top: 0.1rem;
    padding: 0;
    border: 0;
    background: none;
    box-shadow: none;
  }

  .settings__more summary {
    cursor: pointer;
    font-size: var(--fs-small);
    color: var(--dim-text);
    min-height: 1.5rem;
  }

  @media (max-width: 1099px) {
    .models__roles {
      grid-template-columns: minmax(0, 1fr);
    }
  }

  @media (max-width: 599px) {
    .models__check {
      width: auto;
      min-width: 11rem;
    }
  }
</style>
