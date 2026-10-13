<!--
  Config › Character data (module `character_data`; boards B_CfgCharHealth,
  …Alert, …Setup, …Refused and phone twins): read-only health of the NEXON
  Open API (key, the 24-hour call budget, the daily batch, links), of
  MapleScouter, and of the sweeper that strips shared Discord cards, plus
  what is set in the environment. Informational: it never changes /healthz.
-->
<script lang="ts">
  import type { CharacterDataHealth } from '@kanade/api-types';
  import { dayMonth, dayTime, Icon, StatusChip, WavyProgress, type IconName } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-data.scss';
  import { directory } from '../names/directory.svelte';
  import { hourMinute, nexonTag, scouterTag, sweeperTag } from '../chardata/format';
  import SettingsPanel from './SettingsPanel.svelte';

  let { health, timeZone }: { health: CharacterDataHealth; timeZone: string } = $props();

  const nexon = $derived(nexonTag(health, timeZone));
  const scouter = $derived(scouterTag(health));
  const sweeper = $derived(sweeperTag(health));
  const stopped = $derived(health.last_batch?.stopped ? ` (${health.last_batch.stopped})` : '');
  const links = $derived(health.links && health.links.active + health.links.pending + health.links.stale > 0 ? health.links : null);

  const POST_WHY = {
    stuck: 'Kanade can’t edit or delete it. Remove it by hand.',
    unknown: 'The send didn’t confirm and the card wasn’t found after 2 tries. Check the channel and delete it if it is there.',
    overdue: 'It is still up 30 days after it was posted. Check the channel and delete it if it is there.',
  } as const;
</script>

{#snippet tagChip(t: { label: string; tone: 'neutral' | 'ok' | 'risk' | 'warn'; icon: IconName })}
  <StatusChip tone={t.tone}><Icon name={t.icon} />{t.label}</StatusChip>
{/snippet}

{#snippet when(iso: string | null)}
  {#if iso}<span class="mono">{dayTime(iso, timeZone)}</span>{:else}none yet{/if}
{/snippet}

{#snippet nexonCard()}
  <section
    class="chardata-health__card chardata-health__card--{nexon.tone === 'warn' || nexon.tone === 'risk' ? nexon.tone : 'plain'}"
    aria-labelledby="chardata-nexon-h"
    data-fid="chardata-nexon-health"
  >
    <div class="chardata-health__head"><h4 class="chardata-health__title" id="chardata-nexon-h">NEXON Open API</h4><span class="chardata__grow"></span>{@render tagChip(nexon)}</div>
    {#if health.nexon === 'backoff'}
      <p class="chardata__msg">
        The Open API asked Kanade to wait{stopped}. {#if health.backoff_until}Calls resume at <span class="mono">{hourMinute(health.backoff_until, timeZone)}</span>.{/if} Strips keep
        showing data under 30 days old.
      </p>
    {:else if health.nexon === 'no_key'}
      <p class="chardata__msg">
        No API key is set, so links are refused with “Character data isn’t connected yet”. Set <span class="mono">KANADE_NEXON_API_KEY_FILE</span> to the secret file and restart.
      </p>
    {:else if health.nexon === 'key_refused'}
      <p class="chardata__msg">
        NEXON refused the API key{stopped}. All calls are stopped until the key file changes and Kanade restarts. Strips keep showing data under 30 days old; links and refreshes
        wait.
      </p>
    {/if}
    <dl class="chardata-health__facts">
      {#if health.nexon !== 'no_key'}
        <dt>API key</dt>
        <dd>{health.nexon === 'key_refused' ? 'set · refused by NEXON' : 'set · read from a secret file, never shown'}</dd>
      {/if}
      <dt>Budget, 24 h</dt>
      <dd>
        {#if health.calls_24h === null}unknown (the data file could not be read){:else if health.nexon === 'no_key'}<span class="mono">{health.calls_24h}</span> of
          <span class="mono">{health.budget}</span> calls{:else}<span class="chardata-health__budget"
            ><span><span class="mono">{health.calls_24h}</span> of <span class="mono">{health.budget}</span> calls</span><WavyProgress
              value={health.calls_24h}
              max={health.budget}
              wavy={false}
              label="Calls in the last 24 hours"
              tone={health.calls_24h >= health.budget * 0.9 ? 'warn' : 'accent'}
            /></span
          >{/if}
      </dd>
      {#if health.last_batch}
        <dt>Last batch</dt>
        <dd>
          {@render when(health.last_batch.at)} · {health.last_batch.refreshed} refreshed · {health.last_batch.failed} failed {#if health.last_batch.stopped}· stopped at
            {health.last_batch.stopped}{/if}
        </dd>
      {/if}
      {#if health.nexon === 'ok'}
        <dt>Next batch</dt>
        <dd>daily at <span class="mono">04:00</span> (SGT)</dd>
      {/if}
      <dt>Links</dt>
      <dd>
        {#if links}<b class="mono">{links.active}</b> linked · <b class="mono">{links.pending}</b> pending · <b class="mono">{links.stale}</b> stale{:else}none{/if}
      </dd>
    </dl>
  </section>
{/snippet}

{#snippet scouterCard()}
  <section class="chardata-health__card chardata-health__card--{scouter.tone === 'risk' ? 'risk' : 'plain'}" aria-labelledby="chardata-scouter-h" data-fid="chardata-scouter-health">
    <div class="chardata-health__head"><h4 class="chardata-health__title" id="chardata-scouter-h">MapleScouter</h4><span class="chardata__grow"></span>{@render tagChip(scouter)}</div>
    {#if health.scouter.state === 'off'}
      <p class="chardata__msg"><span class="mono">KANADE_SCOUTER_URL</span> is unset, so estimates are off and no Estimate rows are drawn.</p>
    {:else}
      {#if health.scouter.state === 'verification_required'}
        <p class="chardata__msg">
          MapleScouter asked for a verification check. Lookups say “Scouter needs a manual check” until someone runs the scouter setup on its host.
        </p>
      {:else if health.scouter.state === 'unreachable'}
        <p class="chardata__msg">No answer from the scouter URL. Lookups say “Lookup failed.” until it answers.</p>
      {/if}
      <dl class="chardata-health__facts">
        <dt>URL</dt>
        <dd class="mono">{health.scouter.url}</dd>
        {#if health.scouter.state === 'ok'}
          <dt>Last lookup</dt>
          <dd>{@render when(health.scouter.last_lookup_at)}</dd>
        {/if}
        <dt>Last error</dt>
        <dd>{#if health.scouter.last_error}{health.scouter.last_error} {#if health.scouter.last_error_at}at {@render when(health.scouter.last_error_at)}{/if}{:else}none{/if}</dd>
      </dl>
    {/if}
  </section>
{/snippet}

{#snippet sweeperCard()}
  <section class="chardata-health__card chardata-health__card--{sweeper.tone === 'risk' ? 'risk' : 'plain'}" aria-labelledby="chardata-sweeper-h" data-fid="chardata-sweeper-health">
    <div class="chardata-health__head"><h4 class="chardata-health__title" id="chardata-sweeper-h">Sweeper</h4><span class="chardata__grow"></span>{@render tagChip(sweeper)}</div>
    <dl class="chardata-health__facts">
      <dt>Last run</dt>
      <dd>{@render when(health.last_sweep)} · hourly</dd>
      <dt>Shared cards</dt>
      <dd>
        {#if health.posts && health.posts.due + health.posts.unknown + health.posts.stuck + health.posts.overdue + health.posts.items.length > 0}<b class="mono"
            >{health.posts.due}</b
          > due · <b class="mono">{health.posts.unknown}</b> unknown · <b class="mono">{health.posts.stuck}</b> stuck · <b class="mono">{health.posts.overdue}</b> overdue{:else}none
          need stripping{/if}
      </dd>
    </dl>
    {#each health.posts?.items ?? [] as post (post.reference)}
      <div class="chardata-health__post" data-fid="chardata-post">
        <span
          ><b>{post.state}</b> · {directory.label('channel', post.channel_id)} · ref <span class="mono">{post.reference}</span> · posted {dayMonth(post.posted_at, timeZone)}</span
        >
        <span>{POST_WHY[post.state]}</span>
        {#if post.jump_url}<a href={post.jump_url} target="_blank" rel="noreferrer">Open in Discord <Icon name="external-link" /></a>{/if}
      </div>
    {/each}
  </section>
{/snippet}

{#snippet envCard()}
  <section class="chardata-health__card chardata-health__env" aria-labelledby="chardata-env-h" data-fid="chardata-env">
    <h4 class="chardata-health__title" id="chardata-env-h">Set in the environment</h4>
    <dl class="chardata-health__facts">
      <dt>Module</dt>
      <dd><span class="mono">on</span><code class="capchip capchip--mono">KANADE_MODULE_CHARACTER_DATA</code></dd>
      <dt>NEXON key file</dt>
      <dd>
        <span class="mono">{health.nexon === 'no_key' ? 'unset' : 'set (contents never shown)'}</span><code class="capchip capchip--mono">KANADE_NEXON_API_KEY_FILE</code>
      </dd>
      <dt>Scouter URL</dt>
      <dd><span class="mono">{health.scouter.url ?? 'unset'}</span><code class="capchip capchip--mono">KANADE_SCOUTER_URL</code></dd>
    </dl>
  </section>
{/snippet}

<SettingsPanel title="Character data">
  {#snippet lead()}Read-only and informational: it never changes /healthz.{/snippet}
  <div class="chardata-health">
    <div class="chardata-health__col">{@render nexonCard()}{@render envCard()}</div>
    <div class="chardata-health__col">{@render scouterCard()}{@render sweeperCard()}</div>
  </div>
</SettingsPanel>
