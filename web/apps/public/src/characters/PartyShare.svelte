<!--
  Under Party · characters' note (boards M_RunShare, M_RunShareMulti,
  M_PhoneRunShare in docs/research/2026-10-15-estimate-share-chart): the run's
  party total per boss from its newest finished lookup (minutes, %,
  technically possible; the admin Estimate panel's columns), then Who carries
  over the party's cached readings with its key, since the portal has no
  members table. Parts are in party order; only a reading's effective %
  counts, never the party card's figure.
-->
<script lang="ts">
  import type { MemberRun, RunPartyCharacters } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, Icon, minutes, percent, ShareChart, StatusChip, type ShareMember } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-data.scss';

  let { party, run }: { party: RunPartyCharacters; run: MemberRun } = $props();

  const totals = $derived(
    (party.totals?.bosses ?? []).flatMap((total) => {
      const boss = run.bosses.find((b) => b.key === total.boss && b.difficulty === total.difficulty);
      return boss ? [{ boss, total }] : [];
    }),
  );
  // Party order; a member whose character has a reading is numbered, in turn.
  const members = $derived.by((): ShareMember[] => {
    let n = 0;
    return party.slots.map((slot) => {
      const readings = slot.character ? (slot.scouter ?? []) : [];
      return {
        id: slot.member.id,
        name: slot.member.name,
        ign: slot.character?.ign ?? null,
        n: readings.length ? ++n : null,
        effective: Object.fromEntries(readings.map((r) => [`${r.difficulty.toUpperCase()}${r.boss}`, r.effective_percentage])),
        assumed: readings.some((r) => r.critical_rate_assumed),
      };
    });
  });
  const you = $derived(party.slots.find((slot) => slot.you)?.member.id ?? null);
  const bosses = $derived(run.bosses.map((boss) => ({ token: boss.token, title: `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}` })));
</script>
{#if totals.length}
  <div class="estimate__combined" class:estimate__combined--one={totals.length === 1} data-fid="estimate-combined">
    {#each totals as { boss, total } (boss.token)}
      <div class="chardata__col">
        <span class="estimate__boss"><b>{boss.name}</b><span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span></span>
        <span class="chardata__value"
          >{#if total.estimated_minutes !== null}<span class="mono">{minutes(total.estimated_minutes)}</span><span class="chardata__sub"
              >· <span class="mono">{percent(total.combined_percentage)}</span></span
            >{:else}—<span class="chardata__sub">no total</span>{/if}</span
        >
        <span class="estimate__chips">
          {#if total.technically_possible === true}<StatusChip tone="ok"><Icon name="check" />technically possible</StatusChip>
          {:else if total.technically_possible === false}<StatusChip tone="warn"><Icon name="alert-triangle" />not technically possible</StatusChip>{/if}
          {#if !total.complete}<StatusChip tone="warn"><Icon name="alert-triangle" />not complete</StatusChip>{/if}
        </span>
      </div>
    {/each}
  </div>
{/if}
{#if party.estimates && members.some((m) => m.n !== null)}
  <ShareChart {bosses} {members} keys {you} fids={{ missing: 'estimate-share-missing', keys: 'estimate-share-keys' }} data-fid="estimate-share" />
{/if}
