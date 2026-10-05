<!-- A saved Config section (HistoryPage.settings): the same pane or phone
     dialog as a change, view-only — saves are outside the change chain, so
     there is no Revert and no member-revert box. -->
<script lang="ts">
  import type { SettingsChangeRow } from '@kanade/api-types';
  import { enter, Modal } from '@kanade/ui';
  import { SURFACE_LABELS, localAt } from './describe';
  import { sectionHref, sectionLabel, settingCount, settingFields } from './settings';

  let {
    wide,
    change,
    timezone,
    actor,
    onclose,
    onraw,
    leaving = false,
    onleft,
  }: {
    wide: boolean;
    change: SettingsChangeRow;
    timezone: string;
    /** Who saved it, as the timeline names them. */
    actor: string;
    onclose: () => void;
    onraw: (change: SettingsChangeRow) => void;
    leaving?: boolean;
    onleft?: (event: AnimationEvent) => void;
  } = $props();
  const id = $derived(change.id);
  const label = $derived(sectionLabel(change.section));
  const fields = $derived(settingFields(change));
</script>

<svelte:window
  onkeydown={(event) => {
    if (wide && !leaving && event.key === 'Escape' && !document.querySelector('dialog[open]')) {
      event.preventDefault();
      onclose();
    }
  }}
/>

{#snippet content()}
  <div class="history-detail__head" data-fid="history-pane-head">
    <div class="history-detail__top">
      <p class="cap">Config · {SURFACE_LABELS[change.surface] ?? change.surface} · {localAt(change.at, timezone)}</p>
      {#if wide}<button class="btn btn--ghost history-detail__close" type="button" aria-label="Close change details" onclick={onclose}>×</button>{/if}
    </div>
    <h2>{label} settings saved</h2>
    <p class="history-detail__meta">by {actor} · revision <span class="mono">{change.revision}</span> · {settingCount(change)} · view only</p>
  </div>

  <div class="history-detail__body">
    <section class="history-detail__rows" aria-labelledby="history-config-rows-{change.id}">
      <h3 class="cap" id="history-config-rows-{change.id}">Settings · {change.values.length}</h3>
      <article class="history-diff" data-fid="history-diff">
        <p class="mono">config/{change.section}</p>
        <dl class="history-diff__fields">
          {#each fields as field (field.name)}
            <div class="history-diff__field">
              <dt class="mono" title={field.name}>{field.name}</dt>
              <dd class="history-diff__was" title={field.was}><span class="vh">was </span><s>{field.was}</s></dd>
              <dd class="history-diff__arrow" aria-hidden="true">→</dd>
              <dd class="history-diff__now" title={field.now}><span class="vh">now </span>{field.now}</dd>
            </div>
          {/each}
        </dl>
      </article>
    </section>

    <button class="linklike history-detail__raw" data-fid="history-raw" type="button" onclick={() => onraw(change)}>Show raw JSON</button>

    <p class="history-detail__note">Config saves are kept for the record; they are not part of the change chain and cannot be reverted here.</p>
    <div class="history-detail__actions" data-fid="history-actions">
      <a class="btn" href={sectionHref(change.section)}>Open {label} in Config</a>
    </div>
  </div>
{/snippet}

{#if wide}
  <aside class="side-pane side-pane--history" class:is-leaving={leaving} inert={leaving} aria-label="Change details" data-fid="history-pane" onanimationend={onleft} {@attach enter(id)}>
    {@render content()}
  </aside>
{:else}
  <Modal open={!leaving} title="{label} settings" eyebrow="History" narrow className="history-detail" onclose={onclose}>
    {@render content()}
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Close</button>
    {/snippet}
  </Modal>
{/if}
