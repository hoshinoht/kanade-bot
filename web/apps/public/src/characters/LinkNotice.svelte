<!--
  The NEXON notice for one of the member's own characters (boards
  M_LinkNotice, M_PhoneLinkNotice, M_LinkOutcomes): the current notice word for word
  as `/character link` sends it (the server's `LinkNotice`): the lead, What
  Kanade fetches, Why, How long, Who sees it, the "as is" line with the
  Privacy notice, the attribution, and Link {IGN} / Cancel. Link needs a
  fresh sign-in (nothing is linked without it); its outcome, in copy.rs
  words, replaces the notice. Cancel keeps a link waiting for the member
  and says so. A bottom sheet in the phone frame.
-->
<script lang="ts">
  import type { CharacterRow, LinkNotice, LinkOutcome } from '@kanade/api-types';
  import { asOfDate, Icon, Modal, regionLabel, type IconName } from '@kanade/ui';
  import { thousands, type MemberCharacters } from './characters.svelte';

  let {
    open = $bindable(false),
    store,
    character,
    phone,
    onreauth,
    ondone,
    onclosed,
  }: {
    open: boolean;
    store: MemberCharacters;
    character: CharacterRow;
    phone: boolean;
    /** Link was refused for want of a fresh sign-in: nothing was linked. */
    onreauth: (row: CharacterRow) => void;
    /** Something may have changed (linked, or the notice could not be shown): re-read. */
    ondone: () => void;
    /** Closed and focus given back: the host may unmount it. */
    onclosed: () => void;
  } = $props();

  type Told = { warn: boolean; icon: IconName; lead: string; text: string; facts?: [string, string][]; attribution?: string };
  let notice = $state<LinkNotice | null>(null);
  let problem = $state('');
  let changed = $state(false);
  let told = $state<Told | null>(null);
  let busy = $state<'' | 'link' | 'cancel'>('');

  async function read() {
    notice = null;
    problem = '';
    const result = await store.notice(character.id);
    if (result.ok) notice = result.value;
    else problem = result.message;
  }

  $effect(() => {
    if (!open) return;
    told = null;
    changed = false;
    void read();
  });
  function outcome(result: LinkOutcome, ign: string): Told {
    const data = result.link?.data ?? null;
    switch (result.state) {
      case 'linked': {
        const facts: [string, string][] = [];
        if (data?.class) facts.push(['Class', data.class]);
        if (data?.level != null) facts.push(['Level', String(data.level)]);
        if (data?.combat_power != null) facts.push(['Combat power', thousands(data.combat_power)]);
        if (data?.world) facts.push(['World', data.world]);
        if (data) facts.push(['As of', `${asOfDate(data.as_of)} (SGT)`]);
        return {
          warn: false,
          icon: 'check',
          lead: `Linked ${ign}`,
          facts,
          text: 'Refreshed daily, kept 30 days at most; its class and level are copied to your roster character. See all of it in My data; remove it with Unlink.',
          attribution: result.link?.attribution,
        };
      }
      case 'linked_later':
        return { warn: false, icon: 'check', lead: `Linked ${ign}.`, text: 'NEXON didn’t answer just now, so nothing is kept yet; Kanade tries again later.' };
      case 'busy':
        return { warn: true, icon: 'clock', lead: 'Kanade has used today’s NEXON lookups.', text: 'Try again tomorrow; nothing was linked.' };
      case 'unavailable':
        return { warn: true, icon: 'wifi-off', lead: 'NEXON isn’t answering right now.', text: 'Try again later; nothing was linked.' };
      case 'not_found':
        return { warn: true, icon: 'alert-triangle', lead: `NEXON couldn’t find ${ign} in MapleSEA.`, text: 'Check the IGN in Edit, then try again. Nothing was kept.' };
      case 'other_region':
        return {
          warn: false,
          icon: 'info',
          lead: 'Only MapleSEA characters can be linked.',
          text: `A ${regionLabel(character.world)} character keeps its manual class and level; Link is not offered.`,
        };
    }
  }

  async function link() {
    if (!notice || busy) return;
    busy = 'link';
    const result = await store.confirm(character.id, notice.version);
    busy = '';
    if (result.ok) {
      told = outcome(result.value, character.ign);
      ondone();
      return;
    }
    if (result.reauth) {
      open = false;
      onreauth(character);
      return;
    }
    if (result.code === 'notice_changed') {
      // Consent is to the words shown: read the new ones before anything is linked.
      changed = true;
      await read();
      return;
    }
    problem = result.message;
  }

  async function cancel(close: () => void) {
    if (busy) return;
    busy = 'cancel';
    const result = await store.cancel(character.id);
    busy = '';
    if (result.ok && result.value.request_kept) {
      told = { warn: false, icon: 'info', lead: 'Nothing was linked.', text: `${character.ign} is still waiting for you; Review and link when you’re ready.` };
      return;
    }
    close();
  }
</script>

<Modal
  bind:open
  eyebrow="NEXON Open API · MapleSEA"
  title={notice?.title ?? `Link ${character.ign} to its NEXON Open API data?`}
  narrow
  className="linknotice-dialog {phone ? 'own-sheet' : ''}"
  dismissible={!busy}
  fid="link-notice-dialog"
  fidParts={{}}
  onclose={onclosed}
>
  {#if told}
    <div class="settings__box linknotice__told" class:settings__box--warn={told.warn} role="status">
      <b class="linknotice__told-lead"><Icon name={told.icon} />{told.lead}</b>
      <span class="chardata__sub"
        >{#if told.facts?.length}{#each told.facts as [label, value], i (label)}{#if i}&nbsp;·&nbsp;{/if}<b>{label}</b> {value}{/each}<br />{/if}{told.text}
        {#if told.attribution}<span class="mono">{told.attribution}</span>{/if}</span
      >
    </div>
  {:else if notice}
    <div class="linknotice" data-fid="link-notice">
      {#if changed}<p class="settings__box settings__box--warn settings__box--small" role="status"><Icon name="info" />The notice changed. Read it again before linking.</p>{/if}
      <p class="linknotice__lead" data-fid="link-notice-lead">{notice.lead}</p>
      {#each [[notice.what, 'link-notice-what'], [notice.why, 'link-notice-why'], [notice.how_long, 'link-notice-how-long'], [notice.who, 'link-notice-who']] as const as [part, fid] (fid)}
        <div class="linknotice__sec" data-fid={fid}>
          <h3 class="cap">{part.heading}</h3>
          <!-- Discord commands keep their code face, as in Discord. -->
          <p>{#each part.text.split(/(\/character \w+)/) as piece, i (i)}{#if i % 2}<span class="mono">{piece}</span>{:else}{piece}{/if}{/each}</p>
        </div>
      {/each}
      <p class="linknotice__small" data-fid="link-notice-asis">
        {notice.as_is}
        {#if notice.privacy_url}<a class="privacy-link" href={notice.privacy_url} target="_blank" rel="noreferrer" data-fid="link-notice-privacy">Privacy notice</a>{/if}
      </p>
      <p class="linknotice__attrib" data-fid="link-notice-attrib">{notice.attribution}</p>
      {#if problem}<p class="settings__box settings__box--risk settings__box--small" role="alert"><Icon name="alert-triangle" />{problem}</p>{/if}
    </div>
  {:else if problem}
    <p class="settings__box settings__box--warn" role="alert"><Icon name="alert-triangle" />{problem}</p>
  {:else}
    <p class="linknotice__small" role="status">Loading the notice…</p>
  {/if}
  {#snippet footer(close)}
    {#if told || !notice}
      <button type="button" class="btn btn--primary" onclick={close}>Done</button>
    {:else}
      <span class="linknotice__ver">notice {notice.version}</span>
      <button type="button" class="btn" data-fid="link-notice-cancel" disabled={busy !== ''} onclick={() => void cancel(close)}>{notice.cancel_label}</button>
      <button type="button" class="btn btn--primary btn--key" data-fid="link-notice-link" disabled={busy !== ''} onclick={() => void link()}
        ><Icon name="link" />{busy === 'link' ? 'Linking…' : notice.link_label}</button
      >
    {/if}
  {/snippet}
</Modal>
