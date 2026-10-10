<!-- A saved Config section or a cleared Limits window (HistoryPage.settings):
     the same pane or phone dialog as a change, view-only — both are outside
     the change chain, so there is no Revert and no member-revert box. -->
<script lang="ts">
  import type { SettingsChangeRow } from '@kanade/api-types';
  import { enter, Modal } from '@kanade/ui';
  import { SURFACE_LABELS, localAt } from './describe';
  import { sectionHref, sectionLabel, settingCount, settingFields, windowClear } from './settings';

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
  // A cleared Limits window rather than a Config save.
  const clear = $derived(windowClear(change));
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
      <p class="cap">{clear ? 'Limits' : 'Config'} · {SURFACE_LABELS[change.surface] ?? change.surface} · {localAt(change.at, timezone)}</p>
      {#if wide}<button class="btn btn--ghost history-detail__close" type="button" aria-label="Close change details" onclick={onclose}>×</button>{/if}
    </div>
    {#if clear}
      <h2>{clear.member}'s chat window cleared</h2>
      <p class="history-detail__meta">by {actor} · {clear.used} answer{clear.used === 1 ? '' : 's'} in the window · view only</p>
    {:else}
      <h2>{label} settings saved</h2>
      <p class="history-detail__meta">by {actor} · revision <span class="mono">{change.revision}</span> · {settingCount(change)} · view only</p>
    {/if}
  </div>

  <div class="history-detail__body">
    <section class="history-detail__rows" aria-labelledby="history-config-rows-{change.id}">
      <h3 class="cap" id="history-config-rows-{change.id}">{clear ? 'Window' : `Settings · ${change.values.length}`}</h3>
      <article class="history-diff" data-fid="history-diff">
        <p class="mono">{clear ? `limits/window/${clear.memberId}` : `config/${change.section}`}</p>
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

    {#if clear}
      <p class="history-detail__note">
        Allowance then: <span class="mono">{clear.limit} per {clear.perS}s</span>{clear.overridden ? ' (own allowance)' : ''}. Window clears are kept for the record; they are not part of the
        change chain and cannot be undone.
      </p>
      <div class="history-detail__actions" data-fid="history-actions">
        <a class="btn" href={sectionHref(change.section)}>Open Limits</a>
      </div>
    {:else}
      <p class="history-detail__note">Config saves are kept for the record; they are not part of the change chain and cannot be reverted here.</p>
      <div class="history-detail__actions" data-fid="history-actions">
        <a class="btn" href={sectionHref(change.section)}>Open {label} in Config</a>
      </div>
    {/if}
  </div>
{/snippet}

{#if wide}
  <aside class="side-pane side-pane--history" class:is-leaving={leaving} inert={leaving} aria-label="Change details" data-fid="history-pane" onanimationend={onleft} {@attach enter(id)}>
    {@render content()}
  </aside>
{:else}
  <Modal open={!leaving} title={clear ? `${clear.member}'s chat window` : `${label} settings`} eyebrow="History" narrow className="history-detail" onclose={onclose}>
    {@render content()}
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Close</button>
    {/snippet}
  </Modal>
{/if}
