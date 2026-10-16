<!--
  One party member's character on a run they share with the member (boards
  M_RunParty, M_RunStates, M_PhoneRunParty): IGN · class · level · its
  source (`roster` for everyone else; the member's own NEXON level, "only you
  see this"), then the scouter's cached estimate per boss of the run, shown
  and never judged ("HCarling · Estimate 31% · ~96 min"; an assumed
  critical rate as "crit assumed", its full label in the title and the
  accessible name), "Estimate · older than n days" under readings at least
  the saved `estimate_days` old. No character, or no estimate yet, says so;
  never 0%.
-->
<script lang="ts">
  import type { PartyCharacterSlot } from '@kanade/api-types';
  import { asOfDate, olderThan, scouterChip } from '@kanade/ui';

  let {
    slot,
    freshDays,
    tagged = false,
    boxed = false,
  }: {
    slot: PartyCharacterSlot;
    /** `RunPartyCharacters.fresh_days`: the "older than n days" marker's n. */
    freshDays: number | null;
    tagged?: boolean;
    /** The phone slot: the chips on their own row box. */
    boxed?: boolean;
  } = $props();

  const character = $derived(slot.character);
  // The source qualifies a level: none without one ("Lv. — roster" said nothing).
  const sourceOf = (c: NonNullable<PartyCharacterSlot['character']>) =>
    c.level_source === 'nexon' ? `NEXON${c.as_of ? ` · as of ${asOfDate(c.as_of).replace(/ \d{4}$/, '')}` : ''} · only you see this` : 'roster';
  const older = $derived(freshDays !== null && slot.scouter?.some((reading) => reading.stale) ? olderThan(freshDays) : '');
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
  {#if older}<span class="party-char__none">Estimate{older}</span>{/if}
{/snippet}

{#snippet line(c: NonNullable<PartyCharacterSlot['character']>, boss: string | null, fid: boolean)}
  <span class="party-char__line" data-fid={fid ? 'party-char-line' : undefined}
    >{#if boss}<span class="party-char__boss">{boss}</span>{/if}<span class="mono">{c.ign}</span>{#if c.class}<span>{c.class}</span>{/if}{#if c.level !== null}<span
        class="mono">Lv. {c.level}</span
      ><span class="party-char__src">{sourceOf(c)}</span>{/if}</span
  >
{/snippet}

{#if !character && !slot.bosses.length}
  <span class="party-char__none">No character on the roster yet</span>
{:else}
  {#if slot.bosses.length}
    <!-- They bring another character to a later boss: one read-only line per boss. -->
    {#each slot.bosses as b, i (`${b.difficulty}${b.boss}`)}
      {#if b.character}{@render line(b.character, `${b.difficulty.toUpperCase()}${b.boss}`, tagged && i === 0)}{:else}<span class="party-char__none"
          >{b.difficulty.toUpperCase()}{b.boss} · no character</span
        >{/if}
    {/each}
  {:else if character}
    {@render line(character, null, tagged)}
  {/if}
  {#if slot.scouter?.length}
    {#if boxed}<span class="party-char">{@render chips()}</span>{:else}{@render chips()}{/if}
  {:else if slot.scouter}
    <span class="party-char__none">No estimate yet</span>
  {/if}
{/if}
