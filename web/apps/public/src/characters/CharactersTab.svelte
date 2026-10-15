<!--
  Account › Characters (module `roster`; boards M_Chars, M_CharsMenu,
  M_CharStates, M_PhoneChars, M_PhoneCharStates): the member's own roster as
  the admins see it, with their NEXON consent. "Waiting for you" comes first
  (a link waiting for the member to confirm the notice, Review and link…),
  then Your characters (the
  admin roster panel, member wording: no Delete, a link chip on each row),
  then NEXON data (My data…, the Privacy notice, the attribution). With no
  characters only the roster's empty note shows; with `character_data` off,
  no chips, strip, estimates or NEXON section.

  Writes (Add, Edit, the row menu, Link) need a fresh sign-in: refused, they
  open "Confirm it's you", whose sign-in comes back here with the same flow
  open (`add=1`, `edit=<id>`, `link=<id>`; My data `data=1`).
-->
<script lang="ts">
  import type { CharacterRow, PublicSession, PublicSessionRow, SettingsActor } from '@kanade/api-types';
  import { CharacterPanel, dayMonth, Icon, type CharacterKit, type Reauth, type Toaster } from '@kanade/ui';
  import { untrack } from 'svelte';
  import type { Portal } from '../portal.svelte';
  import type { Route } from '../route.svelte';
  import ConfirmFresh from '../writes/ConfirmFresh.svelte';
  import { unlinkedWords } from './characters.svelte';
  import LinkNotice from './LinkNotice.svelte';
  import MyData from './MyData.svelte';

  let {
    portal,
    session,
    current,
    phone,
    toaster,
    timeZone,
    route,
  }: {
    portal: Portal;
    session: PublicSession;
    current: PublicSessionRow | null;
    phone: boolean;
    toaster: Toaster;
    timeZone: string;
    route: Route;
  } = $props();

  const store = $derived(portal.characters);
  const data = $derived(store.data);
  const rows = $derived(data?.characters ?? null);
  const pending = $derived(data?.pending ?? []);
  // An admin who linked one, by actor id, to name them ("Linked … by Ren").
  const linkers = $derived(new Map(pending.map((p) => [p.requested_by.id, p.requested_by_name])));
  // The NEXON attribution, as every link carries it (shown wherever NEXON data is spoken of).
  const attribution = $derived(Object.values(data?.links ?? {})[0]?.attribution ?? null);
  const name = (who: SettingsActor) =>
    who.kind === 'member' ? 'you' : who.kind === 'system' ? (who.id === 'nexon' ? 'NEXON' : 'Kanade') : (linkers.get(who.id) ?? 'an admin');
  /** A link or unlink changed what the strip shows: a new kit re-reads it. */
  let generation = $state(0);

  let panel = $state<CharacterPanel>();
  let noticeFor = $state<CharacterRow | null>(null);
  let noticeOpen = $state(false);
  let dataOpen = $state(false);
  // Mounted from the first open until it has closed (focus given back), as the roster's dialogs.
  let dataMounted = $state(false);
  $effect(() => {
    if (dataOpen) dataMounted = true;
  });
  type Asked = Reauth | { kind: 'link'; row: CharacterRow };
  let asked = $state<Asked | null>(null);
  let confirming = $state(false);

  function openNotice(row: CharacterRow) {
    noticeFor = row;
    noticeOpen = true;
  }

  function reauth(ask: Asked) {
    asked = ask;
    confirming = true;
  }

  const kit: CharacterKit = $derived.by(() => {
    void generation;
    return {
      audience: 'member',
      owner: 'you',
      timeZone,
      api: store.api,
      routes: store.routes,
      actor: name,
      updatedBy: (row) => name(row.updated_by),
      catalog: {
        get data() {
          return portal.bosses.rows;
        },
        loading: false,
        load: () => portal.bosses.load(),
      },
      onreauth: reauth,
      // Add, then the notice for a character that can be linked (MapleSEA with NEXON connected).
      onadded: (added) => {
        if (added.linkable) openNotice(added.row);
      },
      onlink: openNotice,
      onunlinked: (_row, done) => {
        toaster.show({ message: unlinkedWords(done), tone: 'ok' });
        void store.load();
      },
    };
  });

  // Back from "Confirm it's you" (or a link an admin shared): open what the address names, once.
  $effect(() => {
    if (!rows || !panel) return;
    const params = route.params;
    untrack(() => {
      const edit = params.get('edit');
      const link = params.get('link');
      if (params.get('add') === '1') panel!.edit(null);
      else if (edit) {
        const row = rows.find((c) => c.id === edit);
        if (row) panel!.edit(row);
      } else if (link) {
        const row = rows.find((c) => c.id === link);
        if (row) openNotice(row);
      } else if (params.get('data') === '1') dataOpen = true;
      else return;
      route.set({ add: '', edit: '', link: '', data: '' });
    });
  });

  const WHAT: Record<string, string> = { retire: 'Retiring', restore: 'Bringing back', main: 'Making it your main' };
  const confirmWords = $derived.by(() => {
    const a = asked;
    if (!a) return null;
    const back = '/account?tab=characters';
    switch (a.kind) {
      case 'add':
        return { what: 'Adding a character', next: `${back}&add=1`, kept: 'Nothing was added. After signing in you come back to Characters with', bold: 'Add character', tail: 'open again.' };
      case 'edit':
        return { what: `Saving ${a.row.ign}`, next: `${back}&edit=${encodeURIComponent(a.row.id)}`, kept: 'Nothing was saved. After signing in you come back to Characters with', bold: `Edit ${a.row.ign}`, tail: 'open again.' };
      case 'link':
        return { what: 'Linking a character', next: `${back}&link=${encodeURIComponent(a.row.id)}`, kept: 'Nothing was linked. After signing in you come back to Characters with the notice for', bold: a.row.ign, tail: 'open again.' };
      case 'change':
        return { what: `${WHAT[a.verb]} ${a.row.ign}`, next: back, kept: 'Nothing was changed. After signing in you come back to Characters; press', bold: a.verb === 'main' ? 'Make main' : a.verb === 'retire' ? 'Retire' : 'Bring back', tail: 'again.' };
    }
  });
</script>

<div class="account-col" data-fid="mychars-panel">
  {#if rows?.length && pending.length}
    <section class="account-sec mychars-pending" aria-labelledby="mc-wait" data-fid="mychars-pending">
      <div class="account-sec__head">
        <h3 class="cap" id="mc-wait">Waiting for you</h3>
        {#if !phone}<span class="account-sec__end account-sec__note">Nothing is fetched until you link</span>{/if}
      </div>
      <ul class="account-grp">
        {#each pending as request, i (request.character_id)}
          {@const row = rows.find((c) => c.id === request.character_id)}
          <li class="account-row" class:account-row--stack={phone} class:account-row--tight={phone} data-fid={i === 0 ? 'mychars-pending-row' : undefined}>
            {#snippet who()}
              <span class="account-lead account-lead--accent"><Icon name="link" /></span>
              <span class="account-row__text">
                <span class="account-row__title"><span class="mono">{request.ign}</span>{#if request.class}{` · ${request.class}`}{/if}</span>
                <span class="account-row__sub"
                  >Waiting since <span class="mono">{dayMonth(request.requested_at, timeZone)}</span> for you to confirm the NEXON notice. It lapses
                  <span class="mono">{dayMonth(request.expires_at, timeZone)}</span>.</span
                >
              </span>
            {/snippet}
            {#if phone}<span class="account-allow__head">{@render who()}</span>{:else}{@render who()}{/if}
            <button
              type="button"
              class="btn btn--primary btn--key"
              class:account-full={phone}
              data-fid={i === 0 ? 'mychars-review' : undefined}
              disabled={!row}
              onclick={() => row && openNotice(row)}><Icon name="link" />Review and link…</button
            >
          </li>
        {/each}
      </ul>
    </section>
  {/if}
  <section class="account-sec mychars-roster" aria-labelledby="mc-chars">
    <div class="account-sec__head">
      <h3 class="cap" id="mc-chars">Your characters</h3>
      {#if !phone && rows?.length}<span class="account-sec__end account-sec__note">IGN unique per region · your first is your main</span>{/if}
    </div>
    <CharacterPanel
      bind:this={panel}
      {kit}
      {rows}
      failed={store.error}
      onretry={() => void store.load()}
      {phone}
      {toaster}
      links={data?.character_data ? data.links : null}
      fids={{ linkState: 'mychars-link-state', next: 'estimate-next', assumed: 'estimate-assumed' }}
    />
  </section>
  {#if data?.character_data && rows?.length}
    <section class="account-sec" aria-labelledby="mc-data" data-fid="mychars-data">
      <div class="account-sec__head">
        <h3 class="cap" id="mc-data">NEXON data</h3>
        {#if !phone}<span class="account-sec__end account-sec__note">Linked characters only · refreshed daily · kept 30 days at most</span>{/if}
      </div>
      <ul class="account-grp">
        <li class="account-row" class:account-row--tight={phone}>
          <span class="account-lead"><Icon name="shield" /></span>
          <span class="account-row__text">
            <span class="account-row__title">What Kanade keeps for you</span>
            <span class="account-row__sub"
              >Everything kept, per character, with dates. Unlink deletes it at once; your roster characters keep their class and level.
              {#if data.privacy_url}<a href={data.privacy_url} target="_blank" rel="noreferrer" data-fid="mychars-privacy">Privacy notice</a>{/if}</span
            >
          </span>
          <button type="button" class="btn" data-fid="mychars-mydata-open" onclick={() => (dataOpen = true)}>My data…</button>
        </li>
      </ul>
      {#if attribution}<span class="mychars-data__attrib" data-fid="mychars-attrib">{attribution}</span>{/if}
    </section>
  {/if}
</div>

{#if noticeFor}
  <LinkNotice
    bind:open={noticeOpen}
    {store}
    character={noticeFor}
    {phone}
    onreauth={(row) => reauth({ kind: 'link', row })}
    ondone={() => {
      generation += 1;
      void store.load();
    }}
    onclosed={() => {
      if (!noticeOpen) noticeFor = null;
    }}
  />
{/if}
{#if dataMounted}<MyData
  bind:open={dataOpen}
  {store}
  {phone}
  {timeZone}
  {toaster}
  onlink={(id) => {
    const row = rows?.find((c) => c.id === id);
    if (!row) return;
    dataOpen = false;
    openNotice(row);
  }}
  onchanged={() => {
    generation += 1;
    void store.load();
  }}
  onclosed={() => (dataMounted = dataOpen)}
/>{/if}
{#if confirmWords}
  <ConfirmFresh bind:open={confirming} what={confirmWords.what} next={confirmWords.next} {session} {current} {phone} hint="Reading your characters and My data doesn’t need this.">
    {confirmWords.kept} <b>{confirmWords.bold}</b>&#32;{confirmWords.tail}
  </ConfirmFresh>
{/if}
