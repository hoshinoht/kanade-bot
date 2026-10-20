<!--
  Config → Channels → Digest channel: where the weekly digest posts. One
  pick from the guild's channels (or None) with its own Save, which sends
  `posting.channel_id` alone; until a save it follows its env seed, as the
  id lists do. A refusal stays inline under the card.
-->
<script lang="ts">
  import type { Channel, IdListSource } from '@kanade/api-types';
  import { PendingLabel, Select } from '@kanade/ui';
  import '@kanade/ui/styles/core/select.scss';
  import { dirtyReport } from './dirty';

  let {
    channelId,
    source,
    channels,
    save,
  }: {
    channelId: string | null;
    source: IdListSource;
    /** The guild directory; `null` while it loads. */
    channels: Channel[] | null;
    /** Saves the pick (`null` clears it); resolves to '' or the refusal. */
    save: (id: string | null) => Promise<string>;
  } = $props();
  const uid = $props.id();
  const NONE = '';

  // An untouched draft follows the saved value; a refused save keeps it.
  let draft = $state<string | null>(null);
  const picked = $derived(draft ?? channelId ?? NONE);
  const dirty = $derived(draft !== null && draft !== (channelId ?? NONE));
  let error = $state('');
  let saving = $state(false);

  // A saved channel the directory no longer lists stays pickable, shown by id.
  const options = $derived([
    { value: NONE, label: 'None (no digest)' },
    ...(channels ?? []).map((c) => ({ value: c.id, label: c.name })),
    ...(channelId && channels && !channels.some((c) => c.id === channelId) ? [{ value: channelId, label: channelId }] : []),
  ]);

  const report = dirtyReport();
  $effect(() => {
    report?.set(`channels/${uid}`, dirty);
    return () => report?.set(`channels/${uid}`, false);
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!dirty || saving) return;
    saving = true;
    error = await save(picked || null);
    saving = false;
    if (!error) draft = null;
  }

  function discard() {
    draft = null;
    error = '';
  }
</script>

<form class="settings__card digestchannel" data-fid="cfg-card" aria-labelledby="{uid}-t" onsubmit={submit}>
  <div class="settings__cardhead">
    <h4 class="settings__cardtitle" id="{uid}-t">Digest channel</h4>
    <span class="capchip digestchannel__source" data-source={source}
      >{source === 'saved' ? 'Saved here' : 'From'} <code class="mono">KANADE_POST_CHANNEL_ID</code>{source === 'saved' ? ' is ignored' : ''}</span
    >
  </div>
  <p class="settings__cardnote">The weekly digest posts here, naming people rather than pinging them. With none, no digest is posted.</p>
  <Select
    size="field"
    label="Digest channel"
    value={picked}
    {options}
    noun="channels"
    invalid={!!error}
    onchange={(value) => (draft = value)}
  />
  {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  <div class="digestchannel__actions">
    {#if dirty}<button class="btn" type="button" disabled={saving} onclick={discard}>Discard</button>{/if}
    <button class="btn btn--primary settings__key" type="submit" aria-disabled={!dirty || saving}
      ><PendingLabel pending={saving} label="Saving…">Save digest channel</PendingLabel></button
    >
  </div>
</form>

<style>
  .digestchannel__source code {
    font-size: inherit;
  }

  .digestchannel__actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>
