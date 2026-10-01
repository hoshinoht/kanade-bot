<!--
  Context windows: what each routed role runs with (the server's resolution,
  shown as returned), then the saved settings as sliders — cloud and local
  defaults, each role's reply reserve and optional cap, and per-model
  overrides held to the alias's published window. Saved whole in one PATCH;
  the server's refusals show inline and its notices join the toast.
-->
<script lang="ts">
  import type { ConfigView, ContextSettings, ModelInfo, ModelRole } from '@kanade/api-types';
  import { Icon, PendingLabel } from '@kanade/ui';
  import { tick } from 'svelte';
  import { ROLES } from './capacity';
  import { clampNotes, isLocal, LOCAL_WARNING, MAX_CONTEXT_TOKENS, overrideMax, SOURCE_LABELS, tokens } from './context';
  import type { Save } from './save';
  import TokenSlider from './TokenSlider.svelte';

  let { models, save }: { models: ConfigView['models']; save: Save } = $props();
  const uid = $props.id();

  type RoleDraft = { reserve: number | null; cap: number | null; capped: boolean };
  type Draft = {
    cloud: number | null;
    local: number | null;
    roles: Record<ModelRole, RoleDraft>;
    overrides: { alias: string; window: number | null }[];
  };

  function fromSaved(saved: ContextSettings): Draft {
    const role = (r: ContextSettings[ModelRole]): RoleDraft => ({ reserve: r.reserve, cap: r.cap, capped: r.cap !== null });
    return {
      cloud: saved.cloud_default,
      local: saved.local_default,
      roles: { extraction: role(saved.extraction), chat: role(saved.chat), rewrite: role(saved.rewrite) },
      overrides: Object.entries(saved.overrides).map(([alias, window]) => ({ alias, window })),
    };
  }

  // svelte-ignore state_referenced_locally
  let draft = $state(fromSaved(models.context));
  let error = $state('');
  let saving = $state(false);
  let adding = $state('');
  let addSelect: HTMLSelectElement | undefined = $state();
  let overrideList: HTMLUListElement | undefined = $state();

  const info = (alias: string): ModelInfo | undefined => models.catalog.find((m) => m.id === alias);
  const available = $derived(models.catalog.filter((m) => !draft.overrides.some((o) => o.alias === m.id)));
  const anyLocalWarning = $derived(ROLES.some((r) => models.roles[r.id].context?.local_warning));

  $effect(() => {
    if (!available.some((m) => m.id === adding)) adding = available[0]?.id ?? '';
  });

  function toggleCap(role: ModelRole, on: boolean) {
    const r = draft.roles[role];
    r.capped = on;
    // Start a new cap at what the role runs with now, so turning it on changes nothing yet.
    if (on && r.cap === null) r.cap = models.roles[role].context?.window ?? MAX_CONTEXT_TOKENS;
  }

  async function addOverride() {
    const alias = adding;
    if (!alias) return;
    const model = info(alias);
    const start = model?.context_tokens ?? (isLocal(model) ? draft.local : draft.cloud) ?? MAX_CONTEXT_TOKENS;
    draft.overrides.push({ alias, window: Math.min(start, overrideMax(model)) });
    await tick();
    overrideList?.querySelector<HTMLInputElement>(`li[data-alias="${CSS.escape(alias)}"] input[type="number"]`)?.focus();
  }

  async function removeOverride(index: number) {
    draft.overrides.splice(index, 1);
    await tick();
    addSelect?.focus();
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const role = (r: RoleDraft) => ({ reserve: r.reserve!, cap: r.capped ? r.cap : null });
    const context: ContextSettings = {
      cloud_default: draft.cloud!,
      local_default: draft.local!,
      chat: role(draft.roles.chat),
      extraction: role(draft.roles.extraction),
      rewrite: role(draft.roles.rewrite),
      overrides: Object.fromEntries(draft.overrides.map((o) => [o.alias, o.window!])),
    };
    saving = true;
    error = await save({ models: { context } }, 'Context windows saved; the next call uses them.');
    saving = false;
    // The saved object is the truth: resync (a refusal keeps the edits to fix).
    if (!error) draft = fromSaved(models.context);
  }
</script>

<h4 class="settings__subtitle" id="{uid}-h">Context windows</h4>
<p class="note">
  A call's window holds the prompt and the reply: the reserve is kept for the reply, and the rest is the prompt budget. Saved values apply from each
  role's next call.
</p>

<div class="table-wrap ctx__effective">
  <table>
    <caption>In effect now</caption>
    <thead>
      <tr>
        <th scope="col">Role</th>
        <th scope="col">Model</th>
        <th scope="col" class="num">Window</th>
        <th scope="col" class="num">Reserve</th>
        <th scope="col" class="num">Prompt budget</th>
        <th scope="col">Source</th>
        <th scope="col">Notes</th>
      </tr>
    </thead>
    <tbody>
      {#each ROLES as role (role.id)}
        {@const routed = models.roles[role.id]}
        {@const ctx = routed.context}
        <tr>
          <th scope="row">{role.name}</th>
          {#if ctx}
            <td class="mono">{routed.alias}</td>
            <td class="num">{tokens(ctx.window)}</td>
            <td class="num">{tokens(ctx.reserve)}</td>
            <td class="num">{tokens(ctx.prompt_budget)}</td>
            <td>{SOURCE_LABELS[ctx.source]}</td>
            <td>
              {#each clampNotes(ctx, models.context[role.id].reserve) as note (note)}<span class="ctx__note">{note}</span>{/each}
              {#if ctx.local_warning}<span class="ctx__note ctx__note--warn"><Icon name="alert-triangle" />past 16k on a local model</span>{/if}
            </td>
          {:else}
            <td colspan="6" class="note">Not configured: no model, so no context.</td>
          {/if}
        </tr>
      {/each}
    </tbody>
  </table>
</div>
{#if anyLocalWarning}
  <p class="settings__warn"><Icon name="alert-triangle" /><span>{LOCAL_WARNING}</span></p>
{/if}

<form onsubmit={submit} aria-labelledby="{uid}-h">
  <fieldset class="settings__role">
    <legend>Defaults</legend>
    <p class="note">Used when Kanata publishes no window for a model and it has no override.</p>
    <TokenSlider label="Cloud default" bind:value={draft.cloud} max={MAX_CONTEXT_TOKENS} disabled={!models.reachable} />
    <TokenSlider label="Local default" bind:value={draft.local} max={MAX_CONTEXT_TOKENS} local disabled={!models.reachable} />
  </fieldset>

  {#each ROLES as role (role.id)}
    {@const r = draft.roles[role.id]}
    <fieldset class="settings__role">
      <legend>{role.name} limits</legend>
      <TokenSlider label="{role.name} reply reserve" bind:value={r.reserve} max={MAX_CONTEXT_TOKENS} disabled={!models.reachable} />
      <label class="ctx__check">
        <input type="checkbox" checked={r.capped} disabled={!models.reachable} onchange={(e) => toggleCap(role.id, e.currentTarget.checked)} />
        Cap the {role.name.toLowerCase()} window
      </label>
      {#if r.capped}
        <TokenSlider label="{role.name} window cap" bind:value={r.cap} max={MAX_CONTEXT_TOKENS} disabled={!models.reachable} />
      {/if}
    </fieldset>
  {/each}

  <fieldset class="settings__role">
    <legend>Per-model overrides</legend>
    <p class="note">Replaces the published or default window for one exact alias; it cannot exceed what Kanata publishes.</p>
    {#if draft.overrides.length}
      <ul class="ctx__overrides" bind:this={overrideList}>
        {#each draft.overrides as row, index (row.alias)}
          {@const model = info(row.alias)}
          <li data-alias={row.alias}>
            <div class="ctx__override-head">
              <strong class="mono">{row.alias}</strong>
              <button type="button" class="btn btn--ghost" aria-label="Remove the {row.alias} override" onclick={() => removeOverride(index)}>Remove</button>
            </div>
            <ul class="settings__caps" aria-label="What Kanata publishes for {row.alias}">
              {#if model}
                <li class="chip chip--mono">{model.context_tokens ? `published max ${tokens(model.context_tokens)}` : 'no published window'}</li>
                <li class="chip chip--mono">{model.max_output_tokens ? `max output ${tokens(model.max_output_tokens)}` : 'no published max output'}</li>
                <li class="chip">{isLocal(model) ? 'local' : 'cloud'}</li>
              {:else}
                <li class="chip">not in Kanata's list</li>
              {/if}
            </ul>
            <TokenSlider label="{row.alias} window" bind:value={row.window} max={overrideMax(model)} local={isLocal(model)} disabled={!models.reachable} />
          </li>
        {/each}
      </ul>
    {:else}
      <p class="note">No overrides: every model uses its published window or a default.</p>
    {/if}
    <div class="filters ctx__add">
      <label class="field"
        ><span>Override model</span>
        <select bind:value={adding} bind:this={addSelect} disabled={!models.reachable || !available.length}>
          {#each available as m (m.id)}<option value={m.id}>{m.id}</option>{/each}
        </select>
      </label>
      <button type="button" class="btn" onclick={addOverride} disabled={!models.reachable || !adding}>Add override</button>
    </div>
  </fieldset>

  <div class="settings__actions">
    <button class="btn btn--primary" type="submit" disabled={!models.reachable}
      ><PendingLabel pending={saving} label="Saving…">Save context windows</PendingLabel></button
    >
  </div>
</form>
<p class="field__error" role="alert">{error}</p>

<style>
  .ctx__effective table {
    width: auto;
    min-width: 18rem;
  }

  .ctx__note {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    margin-right: 0.5rem;
    font-size: var(--fs-small);
    color: var(--dim-text);
  }

  .ctx__note--warn {
    color: var(--warn-text);
    font-weight: 600;
  }

  .ctx__check {
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    min-height: 2.75rem;
    margin-top: 0.35rem;
    font-size: var(--fs-body-sm);
    cursor: pointer;
  }

  .ctx__check input {
    width: 1.1rem;
    height: 1.1rem;
    margin: 0;
  }

  .ctx__overrides {
    display: grid;
    gap: 0.7rem;
    margin: 0.5rem 0 0;
    padding: 0;
    list-style: none;
  }

  .ctx__overrides > li {
    padding: 0.55rem 0.7rem 0.6rem;
    border: 2px solid var(--line-soft);
    border-radius: var(--r-sm);
  }

  .ctx__override-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.4rem;
    overflow-wrap: anywhere;
  }

  .ctx__add {
    margin-top: 0.7rem;
  }
</style>
