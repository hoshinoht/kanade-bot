# Boss knowledge review: iSIingGunz import (2026-09-24)

This is a proposal for review; nothing here is committed. The baseline is the
v4 copy in `legacy/python/boss/knowledge/`, which was copied here unchanged
before the upgrade, so compare with:

```sh
git diff --no-index legacy/python/boss/knowledge boss/knowledge
```

Source: iSIingGunz's public Google Docs guides, fetched 2026-09-24 with
`scripts/boss_knowledge/fetch.py`. Facts are taken from them, the prose is our
own paraphrase, and each file credits the doc in `sources`. The anti-copy guard
passes: the longest run of words shared with any guide is about 6, against a
limit of 12. Items marked **[check]** still need your decision or a check.

## Decisions applied (2026-09-24)

- **Region is MapleSEA.** MapleSEA-applicable values are primary. MapleSEA
  patch notes were checked on 2026-09-24 and cited as `kind: official` where
  used (v247 First Adversary, v251 Radiant Malefic Star, v251 Challengers
  Season 3, v252 Jupiter).
- **Mechanic names** follow the guide, with the old v4 names kept as "also
  called" (Disruption, Auspicious/Ruinous, Alchemical Entity, Bond Initiative,
  brands, Enemy's Will, Immortal Will, Magic Erosion, Squeezing Magic,
  Purification Aura, designated player, altars).
- **Limbo**: "Risk is personal in Phases 1 and 3, shared in Phase 2" is
  restored and cited to MapleTools.
- **Accepted by the parent**: the `force` field, the `official` source kind,
  site-level titles and authors on migrated sources, and `researched_as_of`
  2026-09-24. The v5 loader will allow event bosses that declare `event`, and
  renderers show only the selected difficulty's facts.

## MapleSEA vs KMS values

| Item | Status |
|---|---|
| Radiant Malefic Star Normal HP 3.288q | **Uncertain.** KMS cut Normal HP by about 30% in Feb 2026 (4.697q → 3.288q). MapleSEA launched the boss on 25 Jun 2026 (v251), and its notes list no HP and no HP adjustment. The post-cut value stays primary because SEA shipped about 4 months later on a newer build, but this is not confirmed. The pre-cut value and specs are kept in `difficulties[Normal].notes`. **[check]** in-game. |
| First Adversary: stage 4+ gauge kept through the Cycle | KMS 1.2.407 only. The MapleSEA v247 launch notes say the Cycle resets the gauge, so the note marks it unconfirmed for SEA. |
| HEXA-converted specs (all bosses) | These are iSIingGunz's estimates from KMS; Baldrix's are explicitly KMS release-era. They are not region-specific game values, so they are left as-is. Treat them as rough guides. |
| HP and Sacred Force (all bosses) | From the guides. The MapleSEA notes publish neither, and the level, party size and time limit they do publish match (Jupiter 295, Malefic Star 280, First Adversary 270, all 1-3 players and 30 minutes). |
| Kai | The MapleSEA season is 3 Jun 2026 until the 30 Sep 2026 maintenance, per v251. Levels 270/280 match. Kai is one clear per week per Maple ID and separate from the weekly boss clear limit. These dates are now in `event.availability`. |

Seen in the SEA v251 notes but outside this import: Seren (Normal/Hard) and
Kalos (Easy/Normal) death counts went from 5 to 8, and Limbo's Purification
can now be used while casting Origin and Ascent skills. The v4-derived
Seren/Kalos files do not state death counts, so nothing is wrong, but a later
pass could add these.

## All files

- Schema v2: `_meta.yaml` `schema_version: 2`. Sources were migrated to
  objects in all 11 v4 files; the pre-existing sources keep `fetched:
  2026-09-05` (the v4 research date, not re-fetched).
- Upgraded bosses hold about 2.3-3.1k characters of prose, with 5-6 bullets
  per section. The per-difficulty facts are complete.
- Deliberately left out: rewards, soul weapons, release-event rewards, and
  per-pattern damage/cooldown tables.

## Jupiter

- Added per-difficulty facts: level 295 entry and boss level, 380% PDR, 3
  players, 810 Sacred Force, HP 10.266q (Normal) and 49.4q (Hard), and HEXA
  specs (Normal trio 80k, duo 95k, solo 117k; Hard trio 130k, duo 142k).
- Core has exact values: Commands (-10 when centred, +60/80/100 when off), timers
  (Commands 50/40/30s, separated state 150/120/90s), Ether Warp cooldowns,
  Balanced's Deflection needle, and Bond Initiative stages.
- Danger names each merged form's 200% HP pattern and the Auspicious Command
  failure (+50 to the party).
- Conflict (guide preferred): the neutral band is 401-600, not v4's 400-600.
- Uncertain: the Hard odd/even forced-merge rule comes from the guide only
  **[check]**. The specs assume 860 SAC, level 300 and a maxed Geardock symbol,
  and the guide has no Hard solo figure.

## Radiant Malefic Star

- Added per-difficulty facts: level 280, 380% PDR, 3 players, Sacred Force
  400/550, HP 3.288q (Normal, see the table above) and 14.74q (Hard), and HEXA
  specs.
- Core has exact values: altar ±250 and ±5-15% final damage within
  -10%..+30%, Essence timers, Reality stages at 250/500/750, and the 5-life
  count.
- Danger adds the forced-swap cost (160/200 Predation), the -10% on entering
  Reality, and the patterns that spawn two Essences.
- Names: designated player (also called the Awakener), altars (also called
  cores). No conflicts with v4.

## First Adversary

- Added all four difficulties: entry level 270; boss levels 270/280/285/290;
  Sacred Force 220/320/340/460; per-phase HP; HEXA specs.
- Core covers the final-damage scale, the 2:18 Cycle with staged Will loss,
  the parry cooldown, and the per-phase parry rules.
- Names: Enemy's Will (also called Adversary's Will) and Immortal Will (also
  called Indomitable Will). **[check]** The MapleSEA patch notes use "Will of
  the Adversary" and "Immortal Will", so SEA players may know the first as Will
  of the Adversary. The text keeps the guide name primary, as instructed, and
  mentions the SEA wording.
- Uncertain: the Extreme spec is vague in the source (about 112-113k with a
  Lynn).

## Baldrix

- Added per-difficulty facts: level 290, 380% PDR, 3 players, 700 Sacred
  Force, per-phase HP, and HEXA ranges (KMS release-era).
- Core now covers Phases 2 and 3 (Overflow/Magic Mark, Squeezing Magic,
  afterimages, Soul Execution, Ragnarok); v4 described Phase 1 only.
- Conflict (guide preferred): above 750, a room's penalty spreads to every
  room and locks in at 1000. v4 described these penalties as per-room. Sleipnir
  is also called the Minion.
- Uncertain: I read "4/3 fully opened rifts" as Normal/Hard **[check]**. Magic
  Mark thresholds by party size were left out.

## Limbo

- Added per-difficulty facts: level 285, 380% PDR, 3 players, 500 Sacred Force
  (550 recommended), and HP per phase segment. The guide has no HEXA spec.
- Core has the exact Risk rules, with ownership cited to MapleTools (the guide
  does not state it), plus the Passageway (-150 Erosion).
- Danger covers Specter D's closing walls, Black's contamination pools,
  White's Walls of Thought and Monster of Truth, and Specter C's fusion
  windows.
- The source records the guide's own update date, 2025-04-04.

## Kai (new, event)

- New `kai.yaml` with `event.availability` set to the MapleSEA window
  (Challengers World only, 3 Jun until the 30 Sep 2026 maintenance), solo only.
  Normal: level 270, HP 63t per phase. Hard: level 280, HP 241.5t per phase.
  380% PDR, 20-minute limit, 5 lives.
- Cites the guide and the MapleSEA v251 Challengers notes. No Sacred Force is
  listed, so `force` is omitted.
- Note: the season ends on 30 Sep 2026, six days after this review, so the
  portal may want to hide Kai after that date.

## Unchanged (sources migrated only)

Bellona, Black Mage, Carling, Kalos, Lotus, Seren.
