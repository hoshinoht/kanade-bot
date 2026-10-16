<!--
  Under Party · characters' note (boards M_PartyTotal*, M_PhonePartyTotal* in
  docs/research/2026-10-16-party-polish): the run's Party total (`PartyTotal`,
  the saved estimates' figures, the same as the admin's and the reminder
  image's; as of when, "older than n days" once stale, who is counted at an
  assumed critical rate and who has no estimate), then Who carries over the
  party's saved readings with its key, since the portal has no members
  table. Parts are in party order, each boss its own party; only a reading's
  effective % counts, never the party card's figure. Estimates off: nothing.
-->
<script lang="ts">
  import type { MemberRun, RunPartyCharacters } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, PartyTotal, ShareChart, totalNotes, type ShareMember, type TotalReading } from '@kanade/ui';

  let { party, run, timeZone }: { party: RunPartyCharacters; run: MemberRun; timeZone: string } = $props();

  const bosses = $derived(run.bosses.map((boss) => ({ token: boss.token, title: `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}` })));
  const tokenOf = (boss: string, difficulty: string) => `${difficulty.toUpperCase()}${boss}`;
  // The character each boss brings: its own line when they differ, else the slot's.
  const ignOf = (slot: RunPartyCharacters['slots'][number], token: string) =>
    slot.bosses.find((b) => tokenOf(b.boss, b.difficulty) === token)?.character?.ign ?? slot.character?.ign ?? null;

  const readings = $derived(
    party.slots.flatMap((slot): TotalReading[] =>
      run.bosses.map((boss) => ({
        name: slot.you ? 'You' : slot.member.name,
        token: boss.token,
        ign: ignOf(slot, boss.token),
        reading: slot.scouter?.find((r) => tokenOf(r.boss, r.difficulty) === boss.token) ?? null,
      })),
    ),
  );
  // Party order; a member whose character has a reading is numbered, in turn.
  const members = $derived.by((): ShareMember[] => {
    let n = 0;
    return party.slots.map((slot) => {
      const readings = slot.scouter ?? [];
      return {
        id: slot.member.id,
        name: slot.member.name,
        igns: Object.fromEntries(run.bosses.flatMap((boss) => (ignOf(slot, boss.token) ? [[boss.token, ignOf(slot, boss.token)!]] : []))),
        n: readings.length ? ++n : null,
        effective: Object.fromEntries(readings.map((r) => [tokenOf(r.boss, r.difficulty), r.effective_percentage])),
        assumed: Object.fromEntries(readings.map((r) => [tokenOf(r.boss, r.difficulty), r.critical_rate_assumed])),
      };
    });
  });
  const you = $derived(party.slots.find((slot) => slot.you)?.member.id ?? null);
</script>

{#if party.party_total}
  <PartyTotal
    total={party.party_total}
    {bosses}
    {timeZone}
    notes={totalNotes(party.party_total, readings)}
    audience="member"
    fids={{ row: 'party-total-row', asOf: 'party-total-asof', notes: 'party-total-notes' }}
    data-fid="party-total"
  />
{/if}
{#if party.estimates && members.some((m) => m.n !== null)}
  <ShareChart {bosses} {members} {you} fids={{ keys: 'estimate-share-keys' }} data-fid="estimate-share" />
{/if}
