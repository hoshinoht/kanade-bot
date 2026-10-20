<!--
  The Edit character dialog's strip (module `character_data`; boards
  B_CharLinked, B_CharLink, B_CharUnlink, B_CharStates*, phone twins; the
  portal's M_CharEdit, M_CharStates2): the roster values beside NEXON's
  (each fetch copies NEXON's class and level into the roster, which keeps
  them), the link state and its actions. The attribution shows wherever
  NEXON data does. Module off: the link route answers the generic 404 and
  nothing is drawn. A scouter estimate row follows while estimates are on.

  Admin: Link… (active at once with its first fetch; the member is not
  asked), Unlink… (deletes the NEXON data now) and Refresh (always allowed;
  the shared call budget limits it), each confirmed inline rather than in a
  second dialog. Member (their own character): Link…, Review and link… (a
  link waiting for them) or Link again… open the NEXON notice (the host's),
  Unlink deletes at once with no confirm, as in Discord, and there is no
  Refresh.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { CharacterRow, Unlinked } from '@kanade/api-types';
  import { dayTime } from '../account';
  import Icon from '../components/Icon.svelte';
  import LiveRegion from '../components/LiveRegion.svelte';
  import StatusChip from '../components/StatusChip.svelte';
  import '@kanade/ui/styles/screens/character-data.scss';
  import { asOfDate, combatPower, PENDING_DAYS, STRIP_TAGS, stripState } from './chardata';
  import EstimateRow from './EstimateRow.svelte';
  import { CharacterLinkView, type CharacterFids, type CharacterKit } from './kit.svelte';
  import { dayMonth } from './roster';

  let {
    kit,
    character,
    sheet,
    fids = {},
  }: {
    kit: CharacterKit;
    /** The saved row (the manual values shown are what the roster holds, not the form's edits). */
    character: CharacterRow;
    /** The phone sheet: actions under the values, 44 px. */
    sheet: boolean;
    fids?: CharacterFids;
  } = $props();

  const uid = $props.id();
  const member = $derived(kit.audience === 'member');
  const memberName = $derived(kit.owner);
  const timeZone = $derived(kit.timeZone);
  const updatedBy = $derived(kit.updatedBy(character));
  const view = $derived(new CharacterLinkView(kit.api.link(character.id), kit.routes));
  $effect(() => {
    void view.load();
  });
  const link = $derived(view.data);
  const phase = $derived(link ? stripState(link, character) : null);
  const tag = $derived(phase ? STRIP_TAGS[phase] : null);
  const data = $derived(link?.data ?? null);
  const retired = $derived(character.retired_at !== null);
  const lapses = $derived(link?.requested_at ? dayMonth(new Date(Date.parse(link.requested_at) + PENDING_DAYS * 86_400_000).toISOString(), timeZone) : '');
  // An admin's link carries no consent of the member's (no notice version).
  const adminLinked = $derived(link?.state !== 'pending' && link?.consent_version === null && link?.requested_by?.kind === 'admin');
  // What Unlink deletes, in words: " (Lv. 283 · CP 48.2M, as of 29 Sep 2026)".
  const unlinkWhat = $derived.by(() => {
    if (!data) return '';
    const values = [data.level !== null ? `Lv. ${data.level}` : '', data.combat_power !== null ? `CP ${combatPower(data.combat_power)}` : ''].filter(Boolean).join(' · ');
    return ` (${values ? `${values}, ` : ''}as of ${asOfDate(data.as_of)})`;
  });
  // "Linked 21 Sep by Mika (notice 1) · refreshed daily · …": who made it, and what happens next.
  const linkedNote = $derived.by(() => {
    if (!link?.linked_at) return '';
    const by = adminLinked && link.requested_by ? kit.actor(link.requested_by) : memberName;
    const parts = [`Linked ${dayMonth(link.linked_at, timeZone)} by ${by}${link.consent_version !== null ? ` (notice ${link.consent_version})` : ''}`];
    if (phase === 'linked') parts.push('refreshed daily', 'deleted 30 days after the last refresh');
    else if (phase === 'paused' && data) parts.push(`last refresh ${dayTime(data.fetched_at, timeZone)}`);
    else if (phase === 'stale') parts.push('it never retries on its own');
    return parts.join(' · ');
  });

  let confirm = $state<'link' | 'unlink' | null>(null);
  let busy = $state<'link' | 'unlink' | 'refresh' | null>(null);
  let problem = $state('');
  let said = $state('');
  let strip = $state<HTMLElement>();

  // Why Link… is off: no key, a retired character, or one outside MapleSEA (the API's refusals).
  const linkOff = $derived.by(() => {
    if (!link?.connected) return member ? 'Character data isn’t connected yet. Link opens once the admins connect it.' : 'Links are refused until the NEXON API key is set.';
    if (retired) return `${character.ign} is retired: bring it back to link it.`;
    if (character.world !== 'msea') return 'Only MapleSEA characters can be linked.';
    return '';
  });
  // Why Refresh is off: a retired character, or no key. An admin's Refresh is otherwise always allowed.
  const refreshOff = $derived.by(() => {
    if (!link?.connected) return 'Refresh waits for the NEXON API key.';
    if (retired && data)
      return `${character.ign} is retired, so its daily refresh is paused and the data lapses after 30 days.${member ? ' Bring it back to refresh again.' : ''}`;
    if (retired) return `${character.ign} is retired: bring it back to refresh it.`;
    return '';
  });

  async function open(kind: 'link' | 'unlink') {
    problem = '';
    confirm = kind;
    await tick();
    // Link takes focus; a risk confirmation starts on Cancel.
    strip?.querySelector<HTMLElement>(kind === 'link' ? '[data-confirm-go]' : '[data-confirm-cancel]')?.focus();
  }

  async function close(opener: 'link' | 'unlink') {
    confirm = null;
    await tick();
    strip?.querySelector<HTMLElement>(`[data-action="${opener}"]`)?.focus();
  }

  async function run(action: 'link' | 'unlink' | 'refresh') {
    if (busy) return;
    busy = action;
    problem = '';
    said = '';
    const result = await view.act(action);
    busy = null;
    confirm = null;
    if (!result.ok) {
      problem = result.message;
      return;
    }
    if (member && action === 'unlink') kit.onunlinked?.(character, result.value as Unlinked);
    else
      said =
        action === 'link'
          ? `Linked ${character.ign}.${result.ok && (result.value as { data?: unknown }).data ? ' Its NEXON data is in.' : ' NEXON didn’t answer just now; Kanade tries again later.'}`
          : action === 'unlink'
            ? `Unlinked ${character.ign}: its NEXON data is deleted.`
            : `Refreshed ${character.ign} from NEXON.`;
    await tick();
    // The next action of the new state (Refresh after a link, Link… after an unlink), else the heading.
    (strip?.querySelector<HTMLElement>('[data-action]:not([aria-disabled="true"])') ?? strip?.querySelector<HTMLElement>(`#${uid}-h`))?.focus();
  }
</script>

{#snippet linkButton()}
  <button
    type="button"
    class="btn chardata__btn"
    data-fid="chardata-link"
    data-action="link"
    aria-disabled={linkOff ? 'true' : undefined}
    aria-describedby={linkOff ? `${uid}-why` : undefined}
    onclick={() => {
      if (!linkOff) void open('link');
    }}><Icon name="link" />Link…</button
  >
{/snippet}

<!-- The member's Link opens the host's notice (Review and link… for a link waiting for them, Link again… once stale). -->
{#snippet memberLink(label: string, primary: boolean)}
  <button
    type="button"
    class="btn chardata__btn"
    class:btn--primary={primary && !linkOff}
    data-fid="chardata-link"
    data-action="link"
    aria-disabled={linkOff ? 'true' : undefined}
    aria-describedby={linkOff ? `${uid}-why` : undefined}
    onclick={() => {
      if (!linkOff) kit.onlink?.(character);
    }}><Icon name="link" />{label}</button
  >
{/snippet}

{#snippet refreshButton()}
  {@const off = Boolean(refreshOff) || busy === 'refresh'}
  <button
    type="button"
    class="btn chardata__btn"
    data-fid="chardata-refresh"
    data-action="refresh"
    aria-disabled={off ? 'true' : undefined}
    aria-describedby={refreshOff ? `${uid}-why` : undefined}
    onclick={() => {
      if (!off) void run('refresh');
    }}><Icon name="refresh-cw" />{busy === 'refresh' ? 'Refreshing…' : 'Refresh'}</button
  >
{/snippet}

{#snippet unlinkButton()}
  {#if member}
    <button type="button" class="btn btn--danger chardata__btn" data-fid="chardata-unlink" data-action="unlink" disabled={Boolean(busy)} onclick={() => void run('unlink')}
      ><Icon name="unlink" />{busy === 'unlink' ? 'Unlinking…' : 'Unlink'}</button
    >
  {:else}
    <button type="button" class="btn btn--danger chardata__btn" data-fid="chardata-unlink" data-action="unlink" disabled={Boolean(busy)} onclick={() => void open('unlink')}
      ><Icon name="unlink" />Unlink…</button
    >
  {/if}
{/snippet}

{#snippet actions()}
  {#if member}
    {#if phase === 'not_connected' || phase === 'not_linked'}
      {@render memberLink('Link…', true)}
    {:else if phase === 'pending'}
      {@render memberLink('Review and link…', true)}
    {:else if phase === 'stale'}
      {@render memberLink('Link again…', false)}{@render unlinkButton()}
    {:else}
      {@render unlinkButton()}
    {/if}
  {:else if phase === 'not_connected' || phase === 'not_linked'}
    {@render linkButton()}
  {:else if phase === 'linked' || phase === 'paused' || phase === 'expired'}
    {@render refreshButton()}{@render unlinkButton()}
  {:else}
    {@render unlinkButton()}
  {/if}
{/snippet}

{#snippet nexonNone(question: string, sub: string)}
  <div class="chardata__col chardata__col--none" data-fid="chardata-nexon">
    <span class="cap">NEXON</span><span class="chardata__q">{question}</span>{#if sub}<span class="chardata__sub">{sub}</span>{/if}
  </div>
{/snippet}

{#snippet confirmBox()}
  {#if confirm === 'link'}
    <div class="chardata__confirm" role="group" aria-labelledby="{uid}-ch" data-fid="chardata-confirm">
      <b class="chardata__confirm-title" id="{uid}-ch">Link {character.ign} now?</b>
      <span
        >Kanade fetches its NEXON data now (name, world, class, level, combat power, image link) and refreshes it daily; it is kept 30 days at most.
        {memberName} isn’t asked first. Its class and level are copied to the roster and stay after an unlink.</span
      >
      <span class="chardata__sub">Kanade doesn’t message {memberName} about it; {memberName} sees it in <span class="mono">/character data</span>.</span>
      <div class="chardata__actions">
        <button type="button" class="btn chardata__btn" data-confirm-cancel onclick={() => void close('link')}>Cancel</button>
        <button type="button" class="btn btn--primary chardata__btn" data-confirm-go data-fid="chardata-request" disabled={busy === 'link'} onclick={() => void run('link')}
          ><Icon name="link" />Link</button
        >
      </div>
    </div>
  {:else if confirm === 'unlink'}
    <div class="chardata__confirm chardata__confirm--risk" role="group" aria-labelledby="{uid}-ch" data-fid="chardata-confirm">
      {#if phase === 'pending'}
        <b class="chardata__confirm-title" id="{uid}-ch">Withdraw {character.ign}’s waiting link?</b>
        <span>Nothing was fetched, so nothing is deleted. {memberName}’s <span class="mono">/character link</span> no longer lists it.</span>
      {:else}
        <b class="chardata__confirm-title" id="{uid}-ch">Unlink {character.ign}?</b>
        <span
          >Deletes the NEXON data kept for {character.ign} now{unlinkWhat}. Any card {memberName} shared in Discord is edited to “Character data removed”. The roster keeps
          the class and level copied from NEXON.</span
        >
        <span>No Undo: to bring it back, {memberName} links again with <span class="mono">/character link</span>.</span>
      {/if}
      <div class="chardata__actions">
        <button type="button" class="btn chardata__btn" data-confirm-cancel onclick={() => void close('unlink')}>Cancel</button>
        <button
          type="button"
          class="btn btn--primary btn--danger chardata__btn"
          data-fid="chardata-unlink-confirm"
          disabled={busy === 'unlink'}
          onclick={() => void run('unlink')}><Icon name="unlink" />{phase === 'pending' ? 'Withdraw' : 'Unlink'}</button
        >
      </div>
    </div>
  {/if}
{/snippet}

<!-- The member's own words for each state (M_CharEdit, M_CharStates2). -->
{#snippet memberMessage()}
  {#if phase === 'not_connected'}
    <p class="chardata__msg" id="{uid}-why">{linkOff}</p>
  {:else if phase === 'not_linked'}
    <p class="chardata__msg" id="{uid}-why">{linkOff || 'Link shows the NEXON notice first; nothing is fetched until you press Link there.'}</p>
  {:else if phase === 'pending'}
    <p class="chardata__msg" id="{uid}-why">
      {linkOff || `Review and link shows the NEXON notice. Cancel there keeps it waiting${lapses ? `; it lapses ${lapses}` : ''}.`}
    </p>
  {:else if phase === 'stale'}
    <p class="chardata__msg" id="{uid}-why">
      {#if linkOff}{linkOff}{:else}<b>Link it again</b> once the IGN above is right. Until then readiness uses the manual level.{/if}
    </p>
  {:else if phase === 'expired'}
    <p class="chardata__msg">No refresh reached NEXON in 30 days. Kanade tries again with the next daily batch; nothing is shown until then.</p>
  {:else if phase === 'paused'}
    <p class="chardata__msg">{refreshOff}</p>
  {:else if link?.refresh.last_error && data}
    <p class="chardata__msg">The last refresh failed ({link.refresh.last_error}); these values are as of {asOfDate(data.as_of)}.</p>
  {:else if phase === 'linked' && data}
    <p class="chardata__sub">
      Refreshed <span class="mono">{dayTime(data.fetched_at, timeZone)}</span> with the daily batch. Unlink deletes it now; your roster character keeps its class and level.
    </p>
  {/if}
{/snippet}

{#snippet adminMessage()}
  {#if busy === 'refresh'}
    <p class="chardata__sub" role="status">
      <Icon name="refresh-cw" /> Asking NEXON for {character.ign} (2 calls). The values change when it answers.
    </p>
  {:else if phase === 'not_connected'}
    <p class="chardata__msg" id="{uid}-why">
      An operator adds the key to the secrets directory and restarts Kanade.{#if kit.configHref}&nbsp;Status: <a href={kit.configHref}>Config › Character data</a>.{/if}
    </p>
  {:else if phase === 'not_linked'}
    <p class="chardata__msg" id="{uid}-why">{linkOff || `Link… links it now and fetches its NEXON data; ${memberName} isn’t asked first.`}</p>
  {:else if phase === 'pending'}
    <p class="chardata__msg">
      Waiting for {memberName} to confirm the NEXON notice with <span class="mono">/character link</span>; nothing is fetched until then. Unlink withdraws it.
    </p>
  {:else if phase === 'stale'}
    <p class="chardata__msg">
      <b>{memberName} needs to link again</b> with <span class="mono">/character link</span> once the IGN above is right. Until then readiness uses the manual level.
    </p>
  {:else if phase === 'expired'}
    <p class="chardata__msg" id="{uid}-why">
      {#if refreshOff}{refreshOff}{:else}No refresh has reached NEXON in 30 days{#if kit.configHref}&nbsp;(see <a href={kit.configHref}>Config › Character data</a>){/if}. Refresh tries
        now.{/if}
    </p>
  {:else if refreshOff}
    <p class={phase === 'paused' ? 'chardata__msg' : 'chardata__sub'} id="{uid}-why">{refreshOff}</p>
  {:else if link?.refresh.last_error && data}
    <p class="chardata__msg">The last refresh failed ({link.refresh.last_error}); these values are as of {asOfDate(data.as_of)}.</p>
  {:else if phase === 'linked' && data}
    <p class="chardata__sub">
      Refreshed <span class="mono">{dayTime(data.fetched_at, timeZone)}</span>. Refresh asks NEXON now (2 calls from the shared 800-call daily budget).
    </p>
  {/if}
{/snippet}

{#if link && phase && tag}
  <section class="chardata" class:chardata--sheet={sheet} aria-labelledby="{uid}-h" data-fid="roster-external" bind:this={strip}>
    <div class="chardata__head">
      <h3 class="cap chardata__title" id="{uid}-h" tabindex="-1">Character data</h3>
      <StatusChip tone={tag.tone} data-fid="chardata-state"><Icon name={tag.icon} />{tag.label}</StatusChip>
      <span class="chardata__grow"></span>
      {#if !sheet && !confirm}<span class="chardata__actions">{@render actions()}</span>{/if}
    </div>
    <div class="chardata__cols">
      <div class="chardata__col" data-fid="chardata-manual">
        <span class="cap">Roster</span>
        <span class="chardata__value">{#if character.level !== null}<span class="mono">Lv. {character.level}</span>{/if}{character.class ?? ''}</span>
        <span class="chardata__sub">updated {dayMonth(character.updated_at, timeZone)} by {updatedBy}</span>
      </div>
      {#if data}
        <div class="chardata__col chardata__col--nexon" data-fid="chardata-nexon">
          <span class="cap">NEXON</span>
          <span class="chardata__value"
            >{#if data.level !== null}<span class="mono">Lv. {data.level}</span>{/if}{data.class ?? ''}{#if data.combat_power !== null}<span class="chardata__dot" aria-hidden="true"
                >·</span
              ><span class="mono"><abbr title="Combat Power">CP</abbr> {combatPower(data.combat_power)}</span>{/if}</span
          >
          <span class="chardata__sub" data-fid="chardata-asof">as of {asOfDate(data.as_of)} (SGT)</span>
          {#if phase === 'paused'}<span class="chardata__sub">deleted {dayMonth(data.expires_at, timeZone)} unless it is brought back</span>{/if}
        </div>
      {:else if phase === 'not_connected'}
        {#if member}
          {@render nexonNone('Not connected', 'Kanade has no NEXON key yet. Nothing is fetched or kept.')}
        {:else}
          {@render nexonNone('Character data isn’t connected yet', 'Links are refused until the NEXON API key is set.')}
        {/if}
      {:else if phase === 'not_linked'}
        {@render nexonNone('Not linked', member ? 'Nothing fetched or kept.' : `Nothing from NEXON is kept for ${character.ign}.`)}
      {:else if phase === 'pending'}
        {@render nexonNone(
          member ? 'Waiting for you to link it' : `Waiting for ${memberName} to confirm with /character link`,
          `Since ${link.requested_at ? dayTime(link.requested_at, timeZone) : ''} · nothing fetched yet`,
        )}
      {:else if phase === 'stale'}
        {@render nexonNone('No current data', `NEXON no longer finds ${character.ign} under that name (renamed?), so nothing is shown.`)}
      {:else if phase === 'expired'}
        {@render nexonNone('NEXON data expired and was deleted', 'NEXON data is kept 30 days at most after a refresh.')}
      {:else}
        {@render nexonNone('No data from NEXON yet', 'The next refresh fills it in.')}
      {/if}
    </div>

    {#if member}{@render memberMessage()}{:else}{@render adminMessage()}{/if}

    {#if problem}<p class="settings__box settings__box--risk settings__box--small" role="alert"><Icon name="alert-triangle" />{problem}</p>{/if}
    {@render confirmBox()}
    {#if sheet && !confirm}<div class="chardata__actions">{@render actions()}</div>{/if}

    {#if !(member && phase === 'not_connected')}
      <div class="chardata__foot" data-fid="chardata-note">
        <span>
          {#if member && phase === 'not_linked'}MapleSEA characters only.
          {:else if member && phase === 'pending'}Nothing is fetched until you press Link.
          {:else if phase === 'not_connected'}Nothing from NEXON is kept for {character.ign}.
          {:else if phase === 'not_linked'}{memberName} can also link it with <span class="mono">/character link</span>.
          {:else if phase === 'pending' && link.requested_at}It lapses {lapses} unless
            {memberName} links it first.
          {:else if link.linked_at}{linkedNote}
          {/if}
        </span>
        {#if link.privacy_url}<a href={link.privacy_url} target="_blank" rel="noreferrer" data-fid="chardata-privacy">Privacy</a>{/if}
      </div>
    {/if}
    {#if data}<span class="chardata__attrib" data-fid="roster-external-attrib">{link.attribution}</span>{/if}
    <LiveRegion message={said} />
    {#if !retired}<EstimateRow {kit} {character} {sheet} {fids} />{/if}
  </section>
{:else if view.error}
  <p class="settings__box settings__box--risk settings__box--small" role="alert">
    <Icon name="alert-triangle" />Couldn’t load character data: {view.error}
    <button type="button" class="btn btn--ghost" onclick={() => void view.load()}>Try again</button>
  </p>
{/if}
