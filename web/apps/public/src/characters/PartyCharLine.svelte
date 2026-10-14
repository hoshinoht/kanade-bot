<!--
  One party member's character on a run they share with the member (boards
  M_RunParty, M_RunStates, M_PhoneRunParty): IGN · class · level · its
  source (`roster` for everyone else; the member's own NEXON level, "only you
  see this"), then the scouter's cached estimate per boss of the run, shown
  and never judged ("HCarling · Estimate 31% · ~96 min"; an assumed
  critical rate as "crit assumed", its full label in the title and the
  accessible name). No character, or no lookup in 24 h, says so; never 0%.
-->
<script lang="ts">
  import type { PartyCharacterSlot } from '@kanade/api-types';
  import { asOfDate, scouterChip } from '@kanade/ui';

  let { slot, tagged = false, boxed = false }: { slot: PartyCharacterSlot; tagged?: boolean; /** The phone slot: the chips on their own row box. */ boxed?: boolean } = $props();

  const character = $derived(slot.character);
  const source = $derived(
    character?.level_source === 'nexon' ? `NEXON${character.as_of ? ` · as of ${asOfDate(character.as_of).replace(/ \d{4}$/, '')}` : ''} · only you see this` : 'roster',
  );
</script>

{#snippet chips()}
  <ul class="ready__bosses" data-fid={tagged ? 'party-char-estimate' : undefined}>
    {#each slot.scouter ?? [] as reading (`${reading.difficulty}${reading.boss}`)}
      {@const chip = scouterChip(reading)}
      {@const more = `${chip.rating}${chip.assumed ? ` · ${chip.assumed}` : ''}`}
      <li class="ready__boss">
        <span class="ready__boss-name">{reading.difficulty.toUpperCase()}{reading.boss}</span><span class="status-chip status-chip--neutral ready__scouter" title={more}
          >{chip.text}<span class="vh"> · {more}</span></span
        >
      </li>
    {/each}
  </ul>
{/snippet}

{#if !character}
  <span class="party-char__none">No character on the roster yet</span>
{:else}
  <span class="party-char__line" data-fid={tagged ? 'party-char-line' : undefined}
    ><span class="mono">{character.ign}</span>{#if character.class}<span>{character.class}</span>{/if}<span class="mono">Lv. {character.level ?? '—'}</span><span
      class="party-char__src">{source}</span
    ></span
  >
  {#if slot.scouter?.length}
    {#if boxed}<span class="party-char">{@render chips()}</span>{:else}{@render chips()}{/if}
  {:else if slot.scouter}
    <span class="party-char__none">No estimate in the last 24 h</span>
  {/if}
{/if}
