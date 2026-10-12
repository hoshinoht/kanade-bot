<!--
  The Edit character dialog's strip (module `character_data`; boards
  B_CharLinked, B_CharLink, B_CharUnlink, B_CharStates*, phone twins): the
  manual roster values beside NEXON's (never copied over), the link state,
  Link… (a pending request the member confirms in Discord), Unlink… (deletes
  the NEXON data now) and Refresh (always allowed; the shared call budget limits it), each confirmed
  inline rather than in a second dialog. The attribution shows wherever NEXON
  data does. Module off: the link route answers the generic 404 and nothing
  is drawn. A MapleScouter estimate row follows while estimates are on.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { CharacterRow, SettingsActor } from '@kanade/api-types';
  import { Icon, LiveRegion, StatusChip } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-data.scss';
  import EstimateRow from './EstimateRow.svelte';
  import { asOfDate, combatPower, dayMonth, dayTime, PENDING_DAYS, STRIP_TAGS, stripState } from './format';
  import { CharacterLinkView } from './routes.svelte';

  let {
    character,
    memberName,
    sheet,
    timeZone,
    updatedBy,
    actor,
  }: {
    /** The saved row (the manual values shown are what the roster holds, not the form's edits). */
    character: CharacterRow;
    memberName: string;
    /** The phone sheet: actions under the values, 44 px. */
    sheet: boolean;
    timeZone: string;
    updatedBy: string;
    actor: (who: SettingsActor) => string;
  } = $props();

  const uid = $props.id();
  const view = $derived(new CharacterLinkView(character.id));
  $effect(() => {
    void view.load();
  });
  const link = $derived(view.data);
  const phase = $derived(link ? stripState(link, character) : null);
  const tag = $derived(phase ? STRIP_TAGS[phase] : null);
  const data = $derived(link?.data ?? null);
  const retired = $derived(character.retired_at !== null);
  // What Unlink deletes, in words: " (Lv. 283 · CP 48.2M, as of 29 Sep 2026)".
  const unlinkWhat = $derived.by(() => {
    if (!data) return '';
    const values = [data.level !== null ? `Lv. ${data.level}` : '', data.combat_power !== null ? `CP ${combatPower(data.combat_power)}` : ''].filter(Boolean).join(' · ');
    return ` (${values ? `${values}, ` : ''}as of ${asOfDate(data.as_of)})`;
  });
  // "Linked 21 Sep by Mika (notice 1) · refreshed daily · …": who confirmed it, and what happens next.
  const linkedNote = $derived.by(() => {
    if (!link?.linked_at) return '';
    const parts = [`Linked ${dayMonth(link.linked_at, timeZone)} by ${memberName}${link.consent_version !== null ? ` (notice ${link.consent_version})` : ''}`];
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
    if (!link?.connected) return 'Links are refused until the NEXON API key is set.';
    if (retired) return `${character.ign} is retired: bring it back to link it.`;
    if (character.world !== 'msea') return 'Only MapleSEA characters can be linked.';
    return '';
  });
  // Why Refresh is off: a retired character, or no key. An admin's Refresh is otherwise always allowed.
  const refreshOff = $derived.by(() => {
    if (!link?.connected) return 'Refresh waits for the NEXON API key.';
    if (retired && data) return `${character.ign} is retired, so its daily refresh is paused and the data lapses after 30 days.`;
    if (retired) return `${character.ign} is retired: bring it back to refresh it.`;
    return '';
  });

  async function open(kind: 'link' | 'unlink') {
    problem = '';
    confirm = kind;
    await tick();
    // Request link takes focus; a risk confirmation starts on Cancel.
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
    said =
      action === 'link'
        ? `Asked ${memberName} to link ${character.ign}. Nothing is fetched until they confirm.`
        : action === 'unlink'
          ? `Unlinked ${character.ign}: its NEXON data is deleted.`
          : `Refreshed ${character.ign} from NEXON.`;
    await tick();
    // The next action of the new state (Unlink… after a request, Link… after an unlink), else the heading.
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
  <button type="button" class="btn btn--danger chardata__btn" data-fid="chardata-unlink" data-action="unlink" disabled={Boolean(busy)} onclick={() => void open('unlink')}
    ><Icon name="unlink" />Unlink…</button
  >
{/snippet}

{#snippet actions()}
  {#if phase === 'not_connected' || phase === 'not_linked'}
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
      <b class="chardata__confirm-title" id="{uid}-ch">Ask {memberName} to link {character.ign}?</b>
      <span
        >Nothing is fetched until {memberName} confirms with <span class="mono">/character link</span> in Discord. {memberName} then sees what is kept (name, world, class,
        level, combat power, image link), that it is kept 30 days at most, and how to unlink.</span
      >
      <span class="chardata__sub">Kanade doesn’t message {memberName} about it; tell them in Discord.</span>
      <div class="chardata__actions">
        <button type="button" class="btn chardata__btn" data-confirm-cancel onclick={() => void close('link')}>Cancel</button>
        <button type="button" class="btn btn--primary chardata__btn" data-confirm-go data-fid="chardata-request" disabled={busy === 'link'} onclick={() => void run('link')}
          ><Icon name="link" />Request link</button
        >
      </div>
    </div>
  {:else if confirm === 'unlink'}
    <div class="chardata__confirm chardata__confirm--risk" role="group" aria-labelledby="{uid}-ch" data-fid="chardata-confirm">
      {#if phase === 'pending'}
        <b class="chardata__confirm-title" id="{uid}-ch">Withdraw the request for {character.ign}?</b>
        <span>Nothing was fetched, so nothing is deleted. {memberName}’s <span class="mono">/character link</span> no longer lists it.</span>
      {:else}
        <b class="chardata__confirm-title" id="{uid}-ch">Unlink {character.ign}?</b>
        <span
          >Deletes the NEXON data kept for {character.ign} now{unlinkWhat}. Any card {memberName} shared in Discord is edited to “Character data removed”. The roster character stays as
          it is.</span
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
        <span class="cap">Manual · roster</span>
        <span class="chardata__value">{#if character.level !== null}<span class="mono">Lv. {character.level}</span>{/if}{character.class ?? ''}</span>
        <span class="chardata__sub">manual, updated {dayMonth(character.updated_at, timeZone)} by {updatedBy}</span>
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
        {@render nexonNone('Character data isn’t connected yet', 'Links are refused until the NEXON API key is set.')}
      {:else if phase === 'not_linked'}
        {@render nexonNone('Not linked', `Nothing from NEXON is kept for ${character.ign}.`)}
      {:else if phase === 'pending'}
        {@render nexonNone(
          `Waiting for ${memberName} to confirm with /character link`,
          `Asked ${link.requested_at ? dayTime(link.requested_at, timeZone) : ''}${link.requested_by ? ` by ${actor(link.requested_by)}` : ''} · nothing fetched yet`,
        )}
      {:else if phase === 'stale'}
        {@render nexonNone('No current data', `NEXON no longer finds ${character.ign} under that name (renamed?), so nothing is shown.`)}
      {:else if phase === 'expired'}
        {@render nexonNone('NEXON data expired and was deleted', 'NEXON data is kept 30 days at most after a refresh.')}
      {:else}
        {@render nexonNone('No data from NEXON yet', 'The next refresh fills it in.')}
      {/if}
    </div>

    {#if busy === 'refresh'}
      <p class="chardata__sub" role="status">
        <Icon name="refresh-cw" /> Asking NEXON for {character.ign} (2 calls). The values change when it answers.
      </p>
    {:else if phase === 'not_connected'}
      <p class="chardata__msg" id="{uid}-why">
        An operator sets the key in the environment and restarts Kanade. Status: <a href="/config?section=character-data">Config › Character data</a>.
      </p>
    {:else if phase === 'not_linked'}
      <p class="chardata__msg" id="{uid}-why">{linkOff || `Link… asks ${memberName} to confirm in Discord. Nothing is fetched until they do.`}</p>
    {:else if phase === 'pending'}
      <p class="chardata__msg">
        Kanade doesn’t message {memberName}; tell them in Discord. Their <span class="mono">/character link</span> lists this request first. Unlink withdraws it.
      </p>
    {:else if phase === 'stale'}
      <p class="chardata__msg">
        <b>{memberName} needs to link again</b> with <span class="mono">/character link</span> once the IGN above is right. Until then readiness uses the manual level.
      </p>
    {:else if phase === 'expired'}
      <p class="chardata__msg" id="{uid}-why">
        {#if refreshOff}{refreshOff}{:else}No refresh has reached NEXON in 30 days (see <a href="/config?section=character-data">Config › Character data</a>). Refresh tries
          now.{/if}
      </p>
    {:else if refreshOff}
      <p class={phase === 'paused' ? 'chardata__msg' : 'chardata__sub'} id="{uid}-why">{refreshOff}</p>
    {:else if link.refresh.last_error && data}
      <p class="chardata__msg">The last refresh failed ({link.refresh.last_error}); these values are as of {asOfDate(data.as_of)}.</p>
    {:else if phase === 'linked' && data}
      <p class="chardata__sub">
        Refreshed <span class="mono">{dayTime(data.fetched_at, timeZone)}</span>. Refresh asks NEXON now (2 calls from the shared 800-call daily budget).
      </p>
    {/if}

    {#if problem}<p class="settings__box settings__box--risk settings__box--small" role="alert"><Icon name="alert-triangle" />{problem}</p>{/if}
    {@render confirmBox()}
    {#if sheet && !confirm}<div class="chardata__actions">{@render actions()}</div>{/if}

    <div class="chardata__foot" data-fid="chardata-note">
      <span>
        {#if phase === 'not_connected'}Nothing from NEXON is kept for {character.ign}.
        {:else if phase === 'not_linked'}{memberName} can also link it alone with <span class="mono">/character link</span>.
        {:else if phase === 'pending' && link.requested_at}The request lapses {dayMonth(new Date(Date.parse(link.requested_at) + PENDING_DAYS * 86_400_000).toISOString(), timeZone)} unless
          {memberName} links it first.
        {:else if link.linked_at}{linkedNote}
        {/if}
      </span>
      {#if link.privacy_url}<a href={link.privacy_url} target="_blank" rel="noreferrer" data-fid="chardata-privacy">Privacy</a>{/if}
    </div>
    {#if data}<span class="chardata__attrib" data-fid="roster-external-attrib">{link.attribution}</span>{/if}
    <LiveRegion message={said} />
    {#if !retired}<EstimateRow {character} {sheet} {timeZone} {actor} />{/if}
  </section>
{:else if view.error}
  <p class="settings__box settings__box--risk settings__box--small" role="alert">
    <Icon name="alert-triangle" />Couldn’t load character data: {view.error}
    <button type="button" class="btn btn--ghost" onclick={() => void view.load()}>Try again</button>
  </p>
{/if}
