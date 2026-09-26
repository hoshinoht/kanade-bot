<!--
  Three model roles, each a base model from Kanata's live list with a
  reasoning level resolved against its published efforts (every level when it
  publishes none; inherit only when the extraction effort fits; no `off` where
  the model requires reasoning). Capacity groups are read-only (kanade.toml):
  one row per group, the server's startup check, and what Kanata admits.
-->
<script lang="ts">
  import type { ConfigView, ModelInfo, ModelRole, RoleModel } from '@kanade/api-types';
  import { CHECK_TONE, Icon } from '@kanade/ui';
  import { groupRows, isReasoningValid, kanataLimits, keyLine, modelOptions, reasoningChoices, resetStrandedInheritors, ROLES } from './capacity';
  import type { Save } from './save';

  let { models, env = [], save }: { models: ConfigView['models']; env?: ConfigView['env']; save: Save } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  let roles = $state<Record<ModelRole, RoleModel>>(structuredClone($state.snapshot(models.roles)));
  let rolesError = $state('');
  // Said in the section, next to the fields: which inheriting role was reset and why.
  let resetNote = $state('');

  const info = (alias: string): ModelInfo | undefined => models.catalog.find((m) => m.id === alias);
  // The server runs the startup check on every read and save; it is the truth.
  const checks = $derived(models.capacity_check);
  const MARK = { ok: 'check', warning: 'alert-circle', error: 'alert-triangle' } as const;
  const TONE = CHECK_TONE;
  // Each server verdict once, even if two groups report the same words.
  const verdicts = $derived(checks.filter((c, i) => checks.findIndex((o) => o.level === c.level && o.message === c.message) === i));
  const groups = $derived(groupRows(models));
  const limits = $derived(kanataLimits(models));
  const permitsTotal = $derived(groups[0]?.permits ?? 0);
  // External routes run only with this operator override (pseudonymisation is not in this build).
  const unmasked = $derived(env.find((e) => e.key === 'KANADE_ALLOW_EXTERNAL_UNMASKED')?.value === 'on');

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
    rolesError = await save({ models: { roles: body } }, 'Models saved; the next question uses them.');
    // The server's answer is the truth (it may reset stranded levels): resync.
    if (!rolesError) {
      roles = structuredClone($state.snapshot(models.roles));
      resetNote = '';
    }
  }
</script>

<h3 class="settings__title">Models</h3>
{#if !models.reachable}
  <p class="flash flash--error" role="alert">Kanata's model list is unreachable, so the saved choices are shown but cannot be changed.</p>
{/if}
<form onsubmit={saveRoles}>
  {#each ROLES as role (role.id)}
    {@const chosen = info(roles[role.id].alias)}
    {@const unset = !roles[role.id].alias}
    {@const fixed = roles[role.id].variant_of ? roles[role.id].fixed_effort : undefined}
    {@const unlisted = !chosen}
    <fieldset class="settings__role" aria-describedby="{uid}-{role.id}-job">
      <legend>{role.name}</legend>
      <p class="note" id="{uid}-{role.id}-job">{role.job}</p>
      <div class="filters">
        <label class="field"
          ><span>Model</span>
          <select value={roles[role.id].alias} onchange={(e) => pick(role.id, e.currentTarget.value)} disabled={!models.reachable}>
            {#each modelOptions(role.id, roles[role.id], models.catalog) as o (o.value)}<option value={o.value} disabled={o.disabled}>{o.label}</option>{/each}
          </select>
        </label>
        <label class="field"
          ><span>Reasoning</span>
          {#if fixed}
            <!-- A variant bakes its level in; it wins over any choice here. -->
            <select disabled><option>Fixed: {fixed}</option></select>
          {:else}
            <select value={roles[role.id].reasoning} onchange={(e) => setReasoning(role.id, e.currentTarget.value)} disabled={!models.reachable || unlisted}>
              {#each reasoningChoices(role.id, chosen, roles.extraction.reasoning, roles[role.id].reasoning) as c (c.value)}<option value={c.value}>{c.label}</option>{/each}
            </select>
          {/if}
        </label>
      </div>
      {#if chosen}
        <!-- Fail closed even if the server's flag is missing: any zone but homelab, or the -cloud suffix. -->
        {@const unzoned = chosen.trust_zone !== 'homelab' && chosen.trust_zone !== 'external'}
        {@const untrusted = chosen.leaves_homelab || chosen.trust_zone !== 'homelab' || chosen.id.endsWith('-cloud')}
        <ul class="settings__caps" aria-label="What {chosen.id} can do">
          <li class="tone tone--{untrusted ? 'danger' : 'success'}">
            {unzoned ? 'trust zone unknown' : untrusted ? 'leaves the homelab' : 'homelab'}
          </li>
          <li class="chip">{chosen.function_tools ? 'calls tools' : 'no tools'}</li>
          <li class="chip">{chosen.structured_output ? 'structured output' : 'free text only'}</li>
          <li class="chip">{chosen.sampling_controls ? 'sampling controls' : 'fixed sampling'}</li>
          <li class="chip">
            {chosen.reasoning_efforts === null ? 'reasoning: any level' : chosen.reasoning_efforts.length ? `reasoning: ${chosen.reasoning_efforts.join(', ')}` : 'no reasoning control'}
          </li>
          {#if chosen.admission}
            <li class="chip chip--mono">admits {chosen.admission.max_in_flight}{chosen.admission.adapter_max_in_flight != null ? ` (adapter ${chosen.admission.adapter_max_in_flight})` : ''}</li>
          {:else}
            <li class="chip chip--mono">no published limit</li>
          {/if}
        </ul>
        {#if untrusted}
          <p class="settings__warn">
            <Icon name="alert-triangle" />
            <span>
              {#if unzoned}
                <strong>{chosen.id}</strong> publishes no trust zone, so it is treated as leaving the homelab until Kanata says otherwise.
              {:else}
                Requests to <strong>{chosen.id}</strong> go to an external provider.
              {/if}
              {#if models.pii_pseudonymise}Member names and ids are pseudonymised before they leave.
              {:else if unmasked}<strong>It runs unmasked:</strong> <code>KANADE_ALLOW_EXTERNAL_UNMASKED</code> (models.allow_external_unmasked in
                kanade.toml) is on, so member names and chat leave as written. For provider testing only.
              {:else}Member names would leave as written, so the bot refuses this route unless the operator sets
                <code>KANADE_ALLOW_EXTERNAL_UNMASKED</code> (models.allow_external_unmasked in kanade.toml).{/if}
            </span>
          </p>
        {/if}
      {:else if unset}
        <p class="note">Not configured: this role has no model, so its work is skipped.</p>
      {:else}
        <p class="settings__warn">
          <Icon name="alert-triangle" />
          <span>
            <strong>{roles[role.id].alias}</strong> is not in Kanata's list, so requests with it will fail. It is treated as leaving the
            homelab. Pick a listed alias, or fix the list on Kanata's side.
          </span>
        </p>
      {/if}
    </fieldset>
  {/each}
  {#if resetNote}<p class="settings__warn" role="status"><Icon name="alert-circle" /><span>{resetNote}</span></p>{/if}
  <div class="settings__actions">
    <button class="btn btn--primary" type="submit" disabled={!models.reachable}>Save models</button>
  </div>
</form>
<p class="field__error" role="alert">{rolesError}</p>

<h4 class="settings__subtitle">Capacity groups</h4>
<p class="note">Each group shares its permits among its models, held to the least Kanata admits for any of them.</p>
<div class="table-wrap settings__groups">
  <table>
    <caption>
      {models.groups_source === 'config'
        ? 'Set in kanade.toml under [[models.groups]]; restart to apply.'
        : `Every model shares one group of ${permitsTotal} permit${permitsTotal === 1 ? '' : 's'} (models.permits in kanade.toml).`}
    </caption>
    <thead><tr><th scope="col">Group</th><th scope="col" class="num">Permits</th><th scope="col">Models</th></tr></thead>
    <tbody>
      {#each groups as g (g.group)}
        <tr>
          <th scope="row" class="mono">{g.group}</th>
          <td class="num">{g.permits ?? '—'}</td>
          <td><span class="chips">{#each g.models as m (m)}<span class="chip chip--mono">{m}</span>{/each}</span></td>
        </tr>
      {:else}
        <tr><td colspan="3" class="note">No model has a group, so no calls can run.</td></tr>
      {/each}
    </tbody>
  </table>
</div>
{#if verdicts.length}
  <ul class="settings__checks" aria-label="Startup check">
    {#each verdicts as c, i (i)}
      <li><span class="tone tone--{TONE[c.level]}"><Icon name={MARK[c.level]} label={c.level} /></span><span>{c.message}</span></li>
    {/each}
  </ul>
{/if}
{#if limits.uniform !== null}
  <p class="note">Kanata admits {limits.uniform} call{limits.uniform === 1 ? '' : 's'} at a time per model.</p>
{:else if limits.all.length}
  <div class="table-wrap settings__groups">
    <table>
      <caption>What Kanata admits for the models in use</caption>
      <thead><tr><th scope="col">Model</th><th scope="col" class="num">Calls at a time</th></tr></thead>
      <tbody>
        {#each limits.inUse as l (l.alias)}
          <tr><th scope="row" class="mono">{l.alias}</th><td class="num">{l.max}{l.declared ? ' (declared)' : ''}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if limits.all.length > limits.inUse.length}
    <details class="settings__more">
      <summary>Show all models</summary>
      <div class="table-wrap settings__groups">
        <table>
          <caption class="vh">What Kanata admits for every listed model</caption>
          <thead><tr><th scope="col">Model</th><th scope="col" class="num">Calls at a time</th></tr></thead>
          <tbody>
            {#each limits.all as l (l.alias)}
              <tr><th scope="row" class="mono">{l.alias}</th><td class="num">{l.max}{l.declared ? ' (declared)' : ''}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </details>
  {/if}
{/if}
<p class="note">{keyLine(models.key_limits)}</p>

<style>
  .settings__groups table {
    width: auto;
    min-width: 18rem;
  }

  .settings__more summary {
    cursor: pointer;
    font-size: var(--fs-small);
    color: var(--dim-text);
    min-height: 1.5rem;
  }

  .settings__checks li {
    display: flex;
    align-items: flex-start;
    gap: 0.45rem;
  }
</style>
