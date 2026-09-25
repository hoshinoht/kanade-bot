<!--
  Three model roles, each an alias from Kanata's live list with a reasoning
  level resolved against that alias's published efforts (every level when it
  publishes none; inherit only when the extraction effort fits). Capacity
  groups are shown read-only until the server can store them, with the
  server's own startup check.
-->
<script lang="ts">
  import type { ConfigView, ModelInfo, ModelRole, RoleModel } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import { isReasoningValid, reasoningChoices, resetStrandedInheritors, ROLES } from './capacity';
  import type { Save } from './save';

  let { models, save }: { models: ConfigView['models']; save: Save } = $props();
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
  const TONE = { ok: 'confirmed', warning: 'planned', error: 'at_risk' } as const;

  function pick(role: ModelRole, alias: string) {
    roles[role].alias = alias;
    // A level the new alias does not publish would strand the role; the
    // server would reset it with a notice, so reset it here instead.
    const current = roles[role].reasoning;
    const resolved = current === '' ? roles.extraction.reasoning : current;
    if (!isReasoningValid(info(alias), resolved)) roles[role].reasoning = 'off';
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
    rolesError = await save({ models: { roles } }, 'Models saved; they take effect when the bot restarts.');
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
    {@const unlisted = !chosen}
    <fieldset class="settings__role" aria-describedby="{uid}-{role.id}-job">
      <legend>{role.name}</legend>
      <p class="note" id="{uid}-{role.id}-job">{role.job}</p>
      <div class="filters">
        <label class="field"
          ><span>Model</span>
          <select value={roles[role.id].alias} onchange={(e) => pick(role.id, e.currentTarget.value)} disabled={!models.reachable}>
            {#if unlisted}<option value={roles[role.id].alias} selected>{roles[role.id].alias} (not listed)</option>{/if}
            {#each models.catalog as m (m.id)}
              <option value={m.id} disabled={role.id === 'chat' && !m.function_tools}>{m.id}{role.id === 'chat' && !m.function_tools ? ' (no tools)' : ''}</option>
            {/each}
          </select>
        </label>
        <label class="field"
          ><span>Reasoning</span>
          <select value={roles[role.id].reasoning} onchange={(e) => setReasoning(role.id, e.currentTarget.value)} disabled={!models.reachable || unlisted}>
            {#each reasoningChoices(role.id, chosen, roles.extraction.reasoning, roles[role.id].reasoning) as c (c.value)}<option value={c.value}>{c.label}</option>{/each}
          </select>
        </label>
      </div>
      {#if chosen}
        <!-- Fail closed even if the server's flag is missing: any zone but homelab, or the -cloud suffix. -->
        {@const unzoned = chosen.trust_zone !== 'homelab' && chosen.trust_zone !== 'external'}
        {@const untrusted = chosen.leaves_homelab || chosen.trust_zone !== 'homelab' || chosen.id.endsWith('-cloud')}
        <ul class="settings__caps" aria-label="What {chosen.id} can do">
          <li class="chip {untrusted ? 'chip--no' : 'chip--yes'}">
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
              {#if models.pii_pseudonymise}Member names and ids are pseudonymised before they leave (<code>KANADE_PII_PSEUDONYMISE</code> is on; set in the environment).
              {:else}<strong>Pseudonymisation is off</strong>, so member names and chat leave as written. Only the operator can turn it on, in the environment (<code>KANADE_PII_PSEUDONYMISE</code>).{/if}
            </span>
          </p>
        {/if}
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
<p class="note">
  Each alias belongs to exactly one group with a number of permits. At startup the bot holds every group to the least Kanata admits for
  its aliases, and all groups together to the key's limit — and refuses to start over either. The key is shared with the owner's other
  clients.
</p>
<!-- The server answers 422 read_only for groups until it can store them: show them, don't offer an editor. -->
<p class="note" id="{uid}-groups-ro">Capacity groups are not editable here yet; they are set with the deployment.</p>
{#if models.groups.length}
  <div class="table-wrap">
    <table aria-describedby="{uid}-groups-ro">
      <caption class="vh">Capacity group per model</caption>
      <thead><tr><th scope="col">Model</th><th scope="col">Group</th><th scope="col" class="num">Permits</th></tr></thead>
      <tbody>
        {#each models.groups as g, index (index)}
          <tr><th scope="row" class="mono">{g.model}</th><td class="mono">{g.group}</td><td class="num">{g.permits ?? '—'}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
{:else}
  <p class="note">No capacity groups are declared.</p>
{/if}
<ul class="settings__checks" aria-label="Startup check">
  {#each checks as c, i (i)}
    <li class="status--{TONE[c.level]}"><Icon name={MARK[c.level]} label={c.level} /><span>{c.message}</span></li>
  {/each}
</ul>
<p class="note">
  Kanata admits per alias ({models.alias_limits.map((l) => `${l.alias} ${l.max_in_flight}${l.source === 'declared' ? ' (declared)' : ''}`).join(' · ')});
  the key admits {models.key_limits.max_in_flight}{models.key_limits.shared ? ', shared' : ''}.
</p>
