<!--
  A section's sticky save bar (spec "When changes apply"): a dirty dot and the
  first change ("field old → new", "+n more" after), Discard, and the key
  Save button, which submits the section's form through `form=`. Clean, it
  says "✓ All changes saved" and the key is disabled.
-->
<script lang="ts">
  import { PendingLabel } from '@kanade/ui';
  import { dirtyReport, type Change } from './dirty';

  let {
    form,
    label,
    changes,
    saving = false,
    disabled = false,
    ondiscard,
    clean = 'All changes saved',
    fid = 'cfg-savebar',
    saveFid = 'cfg-save',
    class: extra = '',
  }: {
    form: string;
    label: string;
    changes: Change[];
    saving?: boolean;
    disabled?: boolean;
    ondiscard: () => void;
    /** What the bar says with nothing to save. */
    clean?: string;
    /** Fidelity tags of the bar and its key (the call site's names). */
    fid?: string;
    saveFid?: string;
    class?: string;
  } = $props();

  const report = dirtyReport();
  $effect(() => {
    report?.set(form, changes.length > 0);
    return () => report?.set(form, false);
  });
  const first = $derived(changes[0]);
</script>

<div class={['savebar', extra]} data-fid={fid} class:savebar--dirty={!!first}>
  <p class="savebar__summary" role="status">
    {#if first}
      <span class="savebar__dot" aria-hidden="true"></span>
      <span
        >{changes.length} unsaved change{changes.length === 1 ? '' : 's'} · {first.label}
        <span class="savebar__from">{first.from || '—'}</span> → <b class="savebar__to">{first.to || '—'}</b>{first.unit ? ` ${first.unit}` : ''}{#if changes.length > 1}<span class="savebar__more"
            >&nbsp;+{changes.length - 1} more</span
          >{/if}</span
      >
    {:else}
      <span class="savebar__clean"><span class="savebar__tick" aria-hidden="true">✓</span> {clean}</span>
    {/if}
  </p>
  {#if first}<button class="btn savebar__discard" type="button" disabled={saving} onclick={ondiscard}>Discard</button>{/if}
  <button class="btn btn--primary settings__key savebar__save" data-fid={saveFid} type="submit" {form} disabled={!first || disabled} aria-disabled={saving}
    ><PendingLabel pending={saving} label="Saving…">{label}</PendingLabel></button
  >
</div>
