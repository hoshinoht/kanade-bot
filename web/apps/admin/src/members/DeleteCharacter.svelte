<!--
  Delete one character for good (boards B_RosterDelete, B_PhoneRosterDelete):
  an alertdialog that names the character, says it cannot be brought back
  here and that History keeps an audit row, offers Retire instead for an
  active one, and starts on Cancel. Delete is the roster's only confirmed action.
-->
<script lang="ts">
  import type { CharacterRow } from '@kanade/api-types';
  import { Icon, Modal } from '@kanade/ui';
  import '@kanade/ui/styles/screens/settings.scss';
  import { regionLabel } from './characters';

  let {
    open = $bindable(false),
    memberName,
    character,
    sheet,
    busy,
    error,
    ondelete,
    onretire,
    returnFocus,
  }: {
    open: boolean;
    memberName: string;
    character: CharacterRow;
    sheet: boolean;
    busy: boolean;
    /** Why the last Delete or Retire failed, in words. */
    error: string;
    ondelete: () => void;
    onretire: () => void;
    returnFocus: () => HTMLElement | null;
  } = $props();

  const uid = $props.id();
  const role = $derived(character.retired_at !== null ? 'retired' : character.role === 'main' ? 'main' : 'alt');

  /** Focus starts on Cancel once the dialog is showing, never on Delete. */
  function startHere(button: HTMLButtonElement) {
    if (open) requestAnimationFrame(() => button.focus());
  }
</script>

<Modal
  bind:open
  title="Delete {character.ign}?"
  eyebrow="{memberName} · character"
  narrow
  alert
  describedby="{uid}-text"
  dismissible={!busy}
  className="rosterconfirm {sheet ? 'rosterconfirm--phone' : ''}"
  fid="roster-confirm"
  fidParts={{ title: 'roster-confirm-title' }}
  {returnFocus}
>
  <div class="rosterconfirm__body">
    <div class="rosterconfirm__object" data-fid="roster-confirm-object">
      <span class="rosterconfirm__who">
        <b>{character.ign}</b>
        <span class="rosterform__hint">{[character.class, `${memberName}'s ${role}`, sheet ? '' : regionLabel(character.world)].filter(Boolean).join(' · ')}</span>
      </span>
      {#if character.level}<span class="rosterrow__lv mono"><small>Lv.</small> {character.level}</span>{/if}
    </div>
    <p id="{uid}-text" class="rosterconfirm__text" data-fid="roster-confirm-text">
      This removes {character.ign} from {memberName}'s characters for good; it can't be brought back from here.
    </p>
    <p class="settings__box" data-fid="roster-confirm-audit"><Icon name="info" /><span>History keeps an audit row: who deleted it, when, and its last IGN, class and level.</span></p>
    {#if character.retired_at === null && !sheet}<p class="rosterform__hint">To keep it but stop offering it for runs, retire it instead.</p>{/if}
    {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  </div>
  {#snippet footer(close)}
    {#if character.retired_at === null}
      <button type="button" class="btn rosterconfirm__retire" data-fid="roster-confirm-retire" disabled={busy} onclick={onretire}>Retire instead</button>
      <span class="rosterform__grow"></span>
    {/if}
    <div class="rosterconfirm__pair">
      <button type="button" class="btn" data-fid="roster-confirm-cancel" disabled={busy} onclick={close} {@attach startHere}>Cancel</button>
      <button type="button" class="btn btn--risk btn--key" data-fid="roster-confirm-delete" disabled={busy} onclick={ondelete}>Delete</button>
    </div>
  {/snippet}
</Modal>
