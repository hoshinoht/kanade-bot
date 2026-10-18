<!--
  My data (boards M_MyData, M_PhoneMyData): Discord's `/character data` as a
  dialog, "What Kanade keeps for you": every character with its state,
  values, as of, fetch and deletion dates, the notice version and its shared
  cards, an admin's waiting request first. Each entry has its buttons
  (Review… for a request, Link again… once stale, Unlink, which deletes at
  once with no confirm, as in Discord), then Unlink all and Done. Reading it
  needs no fresh sign-in, and neither does Unlink. A bottom sheet in the
  phone frame, the buttons under each entry.
-->
<script lang="ts">
  import type { MyData, MyDataEntry } from '@kanade/api-types';
  import { newIdempotencyKey } from '@kanade/client';
  import { asOfDate, dayMonth, dayTime, Icon, Modal, StatusChip, type Toaster } from '@kanade/ui';
  import { thousands, unlinkedAllWords, unlinkedWords, type MemberCharacters } from './characters.svelte';

  let {
    open = $bindable(false),
    store,
    phone,
    timeZone,
    toaster,
    onlink,
    onchanged,
    onclosed,
  }: {
    open: boolean;
    store: MemberCharacters;
    phone: boolean;
    timeZone: string;
    toaster: Toaster;
    /** Review… or Link again…: the notice for that character. */
    onlink: (characterId: string) => void;
    /** Something was unlinked: the tab re-reads. */
    onchanged: () => void;
    /** Closed and focus given back: the host may unmount it. */
    onclosed: () => void;
  } = $props();

  let data = $state<MyData | null>(null);
  let problem = $state('');
  let busy = $state('');

  async function read() {
    const result = await store.myData();
    if (result.ok) {
      data = result.value;
      problem = '';
    } else problem = result.message;
  }

  $effect(() => {
    if (!open) return;
    data = null;
    void read();
  });

  const TAGS: Record<MyDataEntry['state'], { tone: 'neutral' | 'ok' | 'risk' | 'warn'; icon: 'clock' | 'check' | 'alert-triangle' | 'link' }> = {
    pending: { tone: 'warn', icon: 'clock' },
    linked: { tone: 'ok', icon: 'check' },
    stale: { tone: 'risk', icon: 'alert-triangle' },
    expired: { tone: 'risk', icon: 'clock' },
    not_linked: { tone: 'neutral', icon: 'link' },
  };
  const STATE_WORDS: Record<MyDataEntry['state'], string> = { pending: 'waiting for you', linked: 'linked', stale: 'stale', expired: 'expired', not_linked: 'not linked' };

  /** copy.rs `DataEntry::render`, line by line. */
  function lines(entry: MyDataEntry): { strong?: boolean; text: string }[] {
    const out: { strong?: boolean; text: string }[] = [];
    if (entry.state === 'pending') {
      const at = entry.requested_at ? `Waiting since ${dayMonth(entry.requested_at, timeZone)}. ` : '';
      return [{ text: `${at}Nothing is fetched until you confirm.` }];
    }
    if (entry.state === 'stale') return [{ text: 'No current data: NEXON no longer finds it under that name. Link it again.' }];
    if (entry.state === 'not_linked') return [{ text: 'Nothing fetched or kept.' }];
    const d = entry.data;
    if (d) {
      const facts = [d.class, d.level !== null ? `Lv. ${d.level}` : null, d.combat_power !== null ? `CP ${thousands(d.combat_power)}` : null, d.world].filter(Boolean);
      if (facts.length) out.push({ strong: true, text: facts.join(' · ') });
      out.push({ text: `As of ${asOfDate(d.as_of)} (SGT) · fetched ${dayTime(d.fetched_at, timeZone)} · deleted by ${dayMonth(d.expires_at, timeZone)} unless refreshed` });
      if (entry.retired) out.push({ text: 'Refresh is paused while it is retired; what is kept is deleted within 30 days of its last fetch.' });
    } else out.push({ text: 'No current data yet; Kanade refreshes it once a day.' });
    const tail: string[] = [];
    if (entry.linked_at) tail.push(`Linked ${dayTime(entry.linked_at, timeZone)}${entry.notice !== null ? `, notice ${entry.notice}` : ''}`);
    if (entry.cards.length) {
      const places = [...new Set(entry.cards.map((card) => card.channel ?? 'a channel'))].join(', ');
      tail.push(`${entry.cards.length === 1 ? '1 shared card' : `${entry.cards.length} shared cards`} in ${places}`);
    }
    if (tail.length) out.push({ text: tail.join(' · ') });
    return out;
  }

  async function unlink(entry: MyDataEntry) {
    if (busy) return;
    busy = entry.character_id;
    const result = await store.unlink(entry.character_id);
    busy = '';
    toaster.show(result.ok ? { message: unlinkedWords(result.value), tone: 'ok' } : { message: `Couldn't unlink ${entry.ign}: ${result.message}`, tone: 'error' });
    onchanged();
    await read();
  }

  // One press of Unlink all is one action: after a 503 (some unlink failed, nothing recorded) Try again resends the same key.
  let allKey: string | null = null;
  // The 503's words, shown in the dialog (a modal leaves the toasts out of reach) until the same action finishes.
  let unfinished = $state('');

  async function unlinkAll() {
    if (busy) return;
    busy = 'all';
    allKey ??= newIdempotencyKey();
    const result = await store.unlinkAll(allKey);
    busy = '';
    unfinished = '';
    if (result.ok) {
      allKey = null;
      toaster.show({ message: unlinkedAllWords(result.value), tone: 'ok' });
    } else if (result.status === 503) {
      unfinished = result.message;
    } else {
      allKey = null;
      toaster.show({ message: `Couldn't unlink everything: ${result.message}`, tone: 'error' });
    }
    onchanged();
    await read();
  }

  const linked = $derived(Boolean(unfinished) || (data?.entries.some((e) => e.state !== 'pending' && e.state !== 'not_linked') ?? false));
</script>

<Modal
  bind:open
  eyebrow="NEXON data"
  title="What Kanade keeps for you"
  narrow
  className="mydata-dialog {phone ? 'own-sheet' : ''}"
  fid="mydata-dialog"
  fidParts={{}}
  onclose={onclosed}
>
  {#if data}
    <div class="mydata" class:mydata--phone={phone} data-fid="mydata">
      <p class="linknotice__lead">Linked characters, from the NEXON Open API. Nothing else from NEXON is kept.</p>
      {#if data.entries.length}
        <ul class="mydata__list" aria-label="Your characters' NEXON data">
          {#each data.entries as entry (entry.character_id)}
            {@const tag = TAGS[entry.state]}
            <li class="mydata__entry" data-fid="mydata-entry">
              <span class="mydata__head"
                ><span class="mono">{entry.ign}</span><StatusChip tone={tag.tone}
                  ><Icon name={tag.icon} />{STATE_WORDS[entry.state]}{entry.retired && entry.state === 'linked' ? ', retired' : ''}</StatusChip
                ></span
              >
              <span class="mydata__lines">
                {#each lines(entry) as line, i (i)}<span>{#if line.strong}<b>{line.text}</b>{:else}{line.text}{/if}</span>{/each}
              </span>
              {#if entry.state !== 'not_linked'}
                <span class="mydata__act">
                  {#if entry.state === 'pending'}
                    <button type="button" class="btn btn--primary" onclick={() => onlink(entry.character_id)}><Icon name="link" />Review…</button>
                  {:else}
                    {#if entry.state === 'stale'}
                      <button type="button" class="btn" onclick={() => onlink(entry.character_id)}><Icon name="link" />Link again…</button>
                    {/if}
                    <button
                      type="button"
                      class="btn btn--danger"
                      aria-label="Unlink {entry.ign}"
                      disabled={busy !== ''}
                      data-fid={entry.state === 'linked' ? 'mydata-unlink' : undefined}
                      onclick={() => void unlink(entry)}><Icon name="unlink" />{busy === entry.character_id ? 'Unlinking…' : 'Unlink'}</button
                    >
                  {/if}
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      {:else}
        <p class="linknotice__small">Nothing from NEXON is kept for you.</p>
      {/if}
      {#if unfinished}
        <p class="settings__box settings__box--warn settings__box--small" role="alert">
          <Icon name="alert-triangle" />Not everything was unlinked yet: {unfinished} Try again finishes it.
        </p>
      {/if}
      <div class="mydata__foot">
        <span>Unlink deletes that character’s NEXON data at once; shared cards then read “Character data removed”. Your roster characters keep their class and level.</span>
        {#if data.privacy_url}<a class="privacy-link" href={data.privacy_url} target="_blank" rel="noreferrer" data-fid="mydata-privacy">Privacy notice</a>{/if}
      </div>
      <p class="linknotice__attrib" data-fid="mydata-attrib">{data.attribution}</p>
    </div>
  {:else if problem}
    <p class="settings__box settings__box--warn" role="alert">
      <Icon name="alert-triangle" />Couldn’t load your data: {problem}
      <button type="button" class="btn btn--ghost" onclick={() => void read()}>Try again</button>
    </p>
  {:else}
    <p class="linknotice__small" role="status">Loading your data…</p>
  {/if}
  {#snippet footer(close)}
    {#if linked}
      <button type="button" class="btn btn--danger" data-fid="mydata-unlink-all" disabled={busy !== ''} onclick={() => void unlinkAll()}
        ><Icon name="unlink" />{busy === 'all' ? 'Unlinking…' : unfinished ? 'Try again' : 'Unlink all'}</button
      ><span class="rosterform__grow"></span>
    {/if}
    <button type="button" class="btn btn--primary" onclick={close}>Done</button>
  {/snippet}
</Modal>
