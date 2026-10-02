# Boss knowledge review: MapleSEA refresh (2026-10-02)

The previous import note (iSIingGunz import, 2026-09-24) is in git history.
This refresh follows `docs/v5/boss-knowledge-proposal.md`, which the user
approved. The research reports, with facts tables, terminology tables and
sources, are in the git-ignored `data/research/boss-guides/research-2026-10-02/`.
All text is our own paraphrase. `validate.py` passes for every file, and the
longest run shared with a cached guide is well under the 12-word limit.

## Decisions applied

- MapleSEA patch-note terms come first; the GMS/KMS/guide name appears once as
  "also called". `force.kind: sacred` is MapleSEA's Authentic Force.
- Values come from KMS, because MapleSEA follows KMS. HP rows are the KMS values
  before OVERDRIVE (what MapleSEA has now). A per-difficulty note gives the value
  after OVERDRIVE, using the exact per-boss cut from KMS 1.2.416 (for example
  Seren -32.1%, Kalos -33.2%, Lotus/Suu Extreme -32.2%, Black Mage Hard -64.9%),
  and the 20-minute limit. Drop the old values once MapleSEA ships OVERDRIVE
  (its countdown runs 14 Oct to 24 Nov 2026).
- Seren and Kalos have 8 lives on every difficulty.
- Bellona is a prep guide: she is not in MapleSEA yet (KMS 1.2.418).
- Meilin (new, `meilin.yaml`) is an active event prep guide: KMS Challengers
  Season 4 ended on 17 Sep 2026, and MapleSEA is expected around Nov 2026. The
  aliases Maerin and 메이린 let chat find her.
- Kai: the MapleSEA season ended at the 30 Sep 2026 maintenance. The file stays,
  and its availability is in the past tense. There is no automatic expiry; the
  user removes or edits event files by hand.
- Malicia (MapleSEA v254, from 15 Oct 2026): no guide until a reliable source
  exists.

## Schema additions (still schema_version 2)

- `strategies`: up to 4 named routes, each with `when`, `risk`, `damage`
  (damage requirement), `payoff` and 1-6 `steps`. Chat renders them as a
  `## Strategies` section after Tips.
- `event.aliases`: other names for an event boss. Chat falls back to event
  guides only when the catalog cannot resolve the name.

## Per boss

| Boss | Main changes | Strategies |
|---|---|---|
| Radiant Malefic Star | Swap cooldown and lock; solo-value scaling; forced-swap cutscene | Fully safe, Flexible standard, Hold +30%, Burst-cycle |
| Jupiter | Rupture naming; drift timing; Phase 3 shove; Auspicious success | One on Jupiter, Two on Jupiter then switch |
| Baldrix | Join-order rooms; 4/3 rifts resolved; Phase 2 Overflow tiers | Phase 1 room assignment, Clean Phase 2 |
| First Adversary | SEA v249 keeps a stage 4+ gauge through the Cycle; decay floor; party scaling | Parry the opener, Bind on entry, Sniping parry/avoid |
| Limbo | Purification Energy; v251 Origin/Ascent use and taller Phase 3 map | Skip the second fusion, Play through both |
| Seren | Chosen Serene naming; full facts; v244/v246/v251 changes | Midnight burst, Kill before Dawn, Solo rhythm |
| Kalos | Gatekeeper Kalos naming; full facts; v251 timers and shot counts | Left-first, Prep and hold, Threshold burst |
| Lotus | Annihilation gauge; Extreme facts; SEA v244 nerfs | Duo standard, Phase 3 drain |
| Carling | SEA names; per-death cost; +300 refill; Phase 3 zone rules | Balanced gauges, Do'oul turbo |
| Black Mage | 12 lives, limits, Origin-only binds, Laser Jail and platform rules | Phase 4 burst-only, Laser Jail invincibility |
| Bellona | Prep guide; gauge numbers, Berserk sequence, Death Count 8/8/5 | Survival first, Party life-sharing |
| Kai | Availability past tense | - |
| Meilin | New event prep guide | Four routes from KMS guides |

## Still unconfirmed (marked in the files)

- Radiant Malefic Star Normal HP 3.288q is not stated by any MapleSEA source.
- Jupiter Hard forced-merge parity rule (the guide and MapleTools disagree).
- First Adversary: whether the decay floor applies on Hard/Extreme, and whether a
  stage 3 gauge resets at the Cycle.
- MapleSEA names for several mechanics (Radiant Malefic Star, Baldrix Phases
  2-3, Lotus bombardment, Carling's Aura Overflow).
- Post-OVERDRIVE HP values are calculated from the published percentages.
