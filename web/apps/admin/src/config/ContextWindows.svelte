<!--
  Context windows: what each routed role runs with (the server's resolution,
  shown as returned), then the saved settings as sliders — cloud and local
  defaults, each role's reply reserve and optional cap, and per-model
  overrides held to the alias's published window. Saved whole in one PATCH;
  the server's refusals show inline and its notices join the toast.
-->
<script lang="ts">
  import type { ConfigView, ContextSettings, ModelInfo, ModelRole } from '@kanade/api-types';
  import { Icon, Select } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import { tick } from 'svelte';
  import { ROLES } from './capacity';
  import { clampNotes, isLocal, LOCAL_WARNING, MAX_CONTEXT_TOKENS, MAX_RESERVE, overrideMax, reserveHelp, SOURCE_LABELS, tokens } from './context';
  import { changes as listChanges, type Change } from './dirty';
  import { followSaved } from './follow.svelte';
  import type { Save } from './save';
  import TokenSlider from './TokenSlider.svelte';

  let {
    models,
    save,
    form,
    changes = $bindable([]),
    saving = $bindable(false),
  }: {
    models: ConfigView['models'];
    save: Save;
    /** The id the Models save bar submits. */
    form: string;
    changes?: Change[];
    saving?: boolean;
  } = $props();
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
  followSaved(
    () => fromSaved(models.context),
    () => draft,
    (saved) => (draft = saved),
  );
  let error = $state('');

  // The save bar's summary, field by field against the saved settings.
  $effect(() => {
    const was = fromSaved(models.context);
    const show = (v: unknown) => (v === null ? 'none' : typeof v === 'number' ? tokens(v) : String(v));
    const cap = (r: RoleDraft) => (r.capped ? r.cap : null);
    const overrides = (d: Draft) => Object.fromEntries(d.overrides.map((o) => [o.alias, o.window]));
    changes = listChanges([
      { label: 'Cloud default', from: was.cloud, to: draft.cloud, show },
      { label: 'Local default', from: was.local, to: draft.local, show },
      ...ROLES.flatMap(({ id, name }) => [
        { label: `${name} reserve`, from: was.roles[id].reserve, to: draft.roles[id].reserve, show },
        { label: `${name} cap`, from: cap(was.roles[id]), to: cap(draft.roles[id]), show },
      ]),
      {
        label: 'Overrides',
        from: overrides(was),
        to: overrides(draft),
        show: (v) => `${Object.keys(v as object).length}`,
      },
    ]);
  });

  /** The save bar's Discard: back to the saved settings. */
  export function discard() {
    draft = fromSaved(models.context);
    error = '';
  }
  let adding = $state('');
  let addSelect: { focus(): void } | undefined = $state();
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

<p class="settings__box" id="{uid}-h">
  A call's window holds the prompt and the reply: the reserve is kept for the reply, and the rest is the prompt budget. Saved values apply from each
  role's next call.
</p>

<div class="settings__card ctx__effective" data-fid="cfg-card">
  <div class="settings__cardhead">
    <h4 class="settings__cardtitle" id="{uid}-effective">In effect now</h4>
    <span class="settings__cardnote">What each role's next call uses.</span>
  </div>
  <table class="settings__table" aria-labelledby="{uid}-effective">
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
  <p class="settings__box settings__box--warn settings__warn"><Icon name="alert-triangle" /><span>{LOCAL_WARNING}</span></p>
{/if}

<form class="ctx__form" id={form} onsubmit={submit} aria-label="Context windows">
  <fieldset class="settings__card">
    <legend class="settings__cardtitle">Defaults</legend>
    <p class="note">Used when Kanata publishes no window for a model and it has no override.</p>
    <TokenSlider label="Cloud default" bind:value={draft.cloud} max={MAX_CONTEXT_TOKENS} disabled={!models.reachable} />
    <TokenSlider label="Local default" bind:value={draft.local} max={MAX_CONTEXT_TOKENS} local disabled={!models.reachable} />
  </fieldset>

  {#each ROLES as role (role.id)}
    {@const r = draft.roles[role.id]}
    <fieldset class="settings__card">
      <legend class="settings__cardtitle">{role.name} limits</legend>
      <TokenSlider label="{role.name} reply reserve" bind:value={r.reserve} max={MAX_RESERVE} disabled={!models.reachable} />
      <p class="note">{reserveHelp()}</p>
      <label class="ctx__check">
        <input type="checkbox" checked={r.capped} disabled={!models.reachable} onchange={(e) => toggleCap(role.id, e.currentTarget.checked)} />
        Cap the {role.name.toLowerCase()} window
      </label>
      {#if r.capped}
        <TokenSlider label="{role.name} window cap" bind:value={r.cap} max={MAX_CONTEXT_TOKENS} disabled={!models.reachable} />
      {/if}
    </fieldset>
  {/each}

  <fieldset class="settings__card">
    <legend class="settings__cardtitle">Per-model overrides</legend>
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
      <div class="field"
        ><span>Override model</span>
        <Select
          label="Override model"
          bind:value={adding}
          bind:this={addSelect}
          options={available.map((m) => ({ value: m.id, label: m.id }))}
          noun="models"
          disabled={!models.reachable || !available.length}
        />
      </div>
      <button type="button" class="btn" onclick={addOverride} disabled={!models.reachable || !adding}>Add override</button>
    </div>
  </fieldset>
</form>
{#if error}<p class="field__error" role="alert">{error}</p>{/if}

<style>
  .ctx__effective {
    overflow-x: auto;
  }

  .ctx__form {
    display: grid;
    gap: 0.875rem;
  }

  .ctx__form > fieldset {
    margin: 0;
    border: 0;
  }

  .ctx__form legend {
    float: left;
    width: 100%;
    padding: 0;
  }

  .ctx__form legend + * {
    clear: both;
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
