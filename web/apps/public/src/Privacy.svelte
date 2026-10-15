<!--
  `/privacy` (boards P_Privacy, P_PhonePrivacy): what Kanade keeps when a
  member links a character to its NEXON Open API data. Public: shown signed
  out, signed in or closed, and it reads nothing. The `/character link`
  notice (version 2) links here; its words change only with a new notice.
-->
<script lang="ts">
  import type { Identity } from '@kanade/api-types';
  import GateCard from './GateCard.svelte';
  import GateWindow from './GateWindow.svelte';

  let { identity, zone, phone }: { identity: Identity | null; zone: string; phone: boolean } = $props();

  const ATTRIBUTION = 'Data based on NEXON Open API';
  const NOTICE = 'Notice 2 · 15 Oct 2026';
  const NEXON_PRIVACY = 'https://m.nexon.com/terms/1000';
</script>

{#snippet body()}
  <h1 class="gate__lead" id="gate-lead" data-fid="privacy-title">Character data and privacy</h1>
  <p>Kanade is this guild’s boss-schedule bot. Linking a character is optional; this covers what Kanade keeps when you do.</p>
  <dl class="privacy__list" data-fid="privacy-list">
    <dt>What is fetched</dt>
    <dd>
      Name, world, class, level, combat power and the character image link, from the NEXON Open API: for a character you link with <span class="mono"
        >/character link</span
      >, once you press Link, or one a guild admin links for you. Each fetch also copies its class and level to your roster character.
    </dd>
    <dt>Why</dt>
    <dd>Boss readiness: guild admins compare level and combat power with each boss when they plan runs.</dd>
    <dt>How long</dt>
    <dd>
      Refreshed once a day. Each item is deleted 30 days after it was fetched; a shared card is cleared after 29 days. The class and level copied to your roster
      character stay there as roster data.
    </dd>
    <dt>Who sees it</dt>
    <dd>
      Guild admins in Kanade’s admin app, and a channel when you post a <span class="mono">/character share</span> card there. It is never shared with anyone else,
      never sent to chat models, and never in backups or exports. The class and level copied to your roster character are roster data: guild members see them, as
      they see the rest of the roster.
    </dd>
    <dt>Your controls</dt>
    <dd>
      <span class="mono">/character data</span> shows everything kept about you. <span class="mono">/character unlink</span> deletes it at once and clears your shared
      cards; leaving the server does the same. The class and level on your roster character stay; an admin can change them.
    </dd>
    <dt>Estimates</dt>
    <dd>
      Admins can ask a separate scouter service for a clear-time estimate, and you can for your own characters. It receives the IGN on the roster and nothing from NEXON; the answer
      is kept in memory for 24 hours. Admins see every estimate; you see your own, and those of your party on runs you are on. Reminder cards show the party members’ IGNs
      and each one’s share of the estimate to everyone in the card’s channel.
    </dd>
    <dt>As is</dt>
    <dd>
      Data is shown as is and may be out of date. Kanade is not affiliated with or endorsed by NEXON. NEXON’s <a href={NEXON_PRIVACY} rel="noreferrer"
        >developer privacy policy</a
      >.
    </dd>
    <dt>Contact</dt>
    <dd>The guild admins on Discord.</dd>
  </dl>
{/snippet}

{#snippet foot()}
  <span class="privacy__attrib" data-fid="privacy-attrib">{ATTRIBUTION}</span><span>{NOTICE}</span>
{/snippet}

{#if phone}
  <GateCard bar="Privacy">
    <div class="gate-card__body privacy" data-fid="privacy-body">
      {@render body()}
      <p class="privacy__foot">{@render foot()}</p>
    </div>
  </GateCard>
{:else}
  <GateWindow {identity} bar="Privacy" {zone} short {foot}>
    <div class="privacy" data-fid="privacy-body">{@render body()}</div>
  </GateWindow>
{/if}
