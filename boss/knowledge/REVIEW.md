# Boss knowledge review: MapleSEA refresh (2026-10-02)

The previous import note (iSIingGunz import, 2026-09-24) is in git history.
This refresh follows `docs/notes/boss-knowledge-proposal.md`, which the user
approved. The research reports, with facts tables, terminology tables and
sources, are in the git-ignored `data/research/boss-guides/research-2026-10-02/`.
All text is our own paraphrase. `validate.py` passes for every file, and the
longest run shared with a cached guide is well under the 12-word limit.

## Update 2026-10-05: Destiny, Union Champion, Carling and First Adversary

Research: git-ignored `data/research/boss-guides/research-2026-10-05/`
(`report-destiny.md`, `report-boss-champion.md`, `report-carling-fa-verify.md`).
Started from a user-supplied Carling/First Adversary draft; only claims a source
supports were kept.

- **Destiny** entries (solo, Destiny Mode, MapleSEA v244/v252): Seren Hard
  FD -80% (2,000 Resolve), Kalos Chaos Death Count 3 (2,500), Carling Hard
  FD +20% (3,000), First Adversary Hard FD -20% and +50% Power of Order loss
  (10,000), Limbo Hard starting at 800 Erosion (12,500), Baldrix Hard +30%
  damage taken (15,000). Resolve per clear is a top-level note per boss.
- **Champion** entries (Union Champion trials; "Boss Champion" was confirmed by
  the user to mean this): Lotus Hard (B), Black Mage Hard (S), Seren Hard (SS),
  Kalos Normal (SSS). Verus Hilla (A) has no knowledge file. MapleSEA notes do
  not list the trial bosses, so they are KMS's; Hamcelot's Trial is not in SEA.
- **Carling**: Tiger/Bird/Dog callouts; strategies are Balanced gauges, Tiger +
  Bird break (MapleBossLab: ~88% of 322 Destiny builds) and Bird + Dog break.
  Tiger + Dog break is a note (disfavoured since v251; owner decision 2026-10-05). Dropped from the draft: the
  unsourced "44 of 56 clears" statistic and Tiger + Bird as proven on Extreme.
- **First Adversary**: Stage 5 opener added (sourced only for Easy/Normal);
  Parry the opener now bursts at Stage 4; "Sniping, avoid it" became a tip;
  Stage 5 tornado danger.
- Schema: `difficulties` up to 7, `sources` up to 14; API contract
  `docs/v5/api-schemas/bosses.json` accepts Champion/Destiny.
- Open: Destiny Mode HP after OVERDRIVE; Jupiter Resolve labels.
- Confirmed by the owner (2026-10-05): SEA Union Champion trials are Lotus B,
  Black Mage S, Seren SS, Kalos SSS, each 20 minutes except Black Mage's 45;
  Extreme Black Mage sets the Phase 4 state at random every 30 s.

## Update 2026-10-05: fuller phase cards and per-party specs

- Phase cards in all 11 phased guides gained items (up to 5 per phase), each
  restating a fact already in the same file (core, danger, tips, difficulty
  notes, mechanics or strategies); no new facts.
- `recommended_spec.parties` rows wherever a spec names more than one party
  size (Baldrix Hard, Bellona Hard, First Adversary Easy-Hard, Jupiter, Radiant
  Malefic Star, Lotus, Seren Extreme). `value` was dropped where a row repeats it.
  Authentic Force specs (Seren Normal/Hard, every Kalos difficulty) stay text
  only, by user decision.
- Confirmed by the user (2026-10-05): Baldrix Hard needs about 108k-113k
  HEXA-converted stat for a trio (KMS release era), about 115k or more for a
  duo and about 131k solo (duo and solo from KMS plus MapleSEA and MapleScouter
  research).
- User placements (2026-10-05): Carling's Bird hide-and-seek is Phase 1; Specter
  A and Limbo Black both have the bubble fight (Grasp of Truth, Dimensional
  Collapse).
- Confirmed by the user (2026-10-05): at 1000 Illusion Magic in Reality a 40 s
  countdown runs and a forced swap follows when it reaches 0.

## Update 2026-10-05: strategy accuracy pass (MapleSEA v254)

Applied the private strategy review (`docs/notes/boss-guides/`, local only).
Conditional routes were qualified, not upgraded; user-confirmed facts above
were left unchanged.

- **Limbo**: a fusion is a ~5 s disappearance followed by a ~30 s pattern
  with attackable periods; the next one triggers 20 points below the HP
  reached. The second-fusion skip is Normal-sourced, unverified on Hard.
  Destiny's 800 Erosion is a start value that recovery can lower.
- **Baldrix**: Phase 2 purple pillars/orbs are safe inside, blue ones are
  avoided (replaces "take purple attacks"); Phase 3 Ragnarok is on a 125 s
  cooldown (MapleSEA v246), separate from the 120 s rotations.
- **Radiant Malefic Star**: altar arithmetic is unresolved (105/115/130% vs
  105/110/120% after three rights from 0). Keep +30% (DPM classes) and Match
  altars to bursts (burst classes) follow iSIingGunz's video guide
  (2026-10-09) and stay high-risk advanced routes.
- **Black Mage**: the ~10 s / ~2 s i-frame figures are unverified; the
  Destruction +10% is the English wiki value; burst-only is for ordinary
  Hard with time slack.
- **Carling, Seren, Kalos, First Adversary, Jupiter**: kill orders, margins,
  timings and side assignments are labelled as party conventions or
  personal targets.
- Owner decisions 2026-10-05: Black Mage Champion stays 45 min and the
  Extreme random 30 s state change stays unqualified (both owner-confirmed);
  Carling Tiger + Dog is disfavoured since v251, not dead.
- Still open: SEA altar modifier after three rights; Hard Limbo fusion skip;
  Black Mage i-frame coverage and SEA Destruction bonus.

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

## Update 2026-10-16: source check

- **Radiant Malefic Star (§3)**: Removed the disputed 750 qualifier for the
  Floating Disaster threshold. Sources: seotbeo, 찬란한 흉성 패턴 정리,
  https://m.blog.naver.com/seotbeo/224143006235; MapleStory Wiki, Malefic Star,
  https://maplestorywiki.net/w/Malefic_Star.
- **Jupiter (§6)**: Hard forced merges alternate by ordinal when centred (3rd
  Auspicious, 6th Ruinous), not by gauge parity. Sources: seotbeo, 유피테르 패턴
  정리, https://m.blog.naver.com/seotbeo/224176249929; MapleTools Jupiter guide
  (already cited in `jupiter.yaml`).
- **First Adversary (§7)**: Confirmed the Power of Order decay floor has no
  difficulty exception. Sources: MapleSEA v247,
  https://www.maplesea.com/updates/view/v247_Patch/; seotbeo, 최초의 대적자 패턴
  정리, https://m.blog.naver.com/seotbeo/223971770435.
- **First Adversary (§8)**: Confirmed that Cycle of Power resets every gauge
  below 600 to 500. Sources: MapleSEA v247,
  https://www.maplesea.com/updates/view/v247_Patch/; MapleSEA v249,
  https://www.maplesea.com/updates/view/v249_Patch_IBF/; seotbeo, 최초의 대적자
  패턴 정리, https://m.blog.naver.com/seotbeo/223971770435.
- **Limbo (§10)**: Confirmed the Phase 2 fusion and second-fusion skip on Hard,
  as well as Normal. Sources: namu mirror, 림보/보스 몬스터/공격 패턴,
  https://m.namu.moe/w/림보(메이플스토리)/보스%20몬스터/공격%20패턴; seotbeo,
  림보 패턴 정리, https://m.blog.naver.com/seotbeo/223545358052.
- **Limbo (§11)**: Confirmed 550 Authentic Force gives the full 1.25× tier
  (+5% per 10 above the requirement). Sources: KMS in-game Authentic Force
  guide, https://gi.maplestory.nexon.com/Guide/GameInformation/Skill/AuthenticForce;
  MapleStory Wiki, Limbo/Monster, https://maplestorywiki.net/w/Limbo/Monster;
  KPRobin, MapleSEA Arcane/Authentic Force list,
  https://kprobin.blogspot.com/2019/01/arcane-force-extra-damage-and-monster.html.
- **Black Mage (§13)**: Confirmed Destruction grants +10% final damage. Sources:
  MapleStory Wiki, Black Mage/Monster,
  https://maplestorywiki.net/w/Black_Mage/Monster; namu, Black Mage attack
  patterns and status effects,
  https://en.namu.wiki/w/검은%20마법사/보스%20몬스터/공격%20패턴 and
  https://en.namu.wiki/w/메이플스토리/시스템/상태이상; FMKorea,
  https://www.fmkorea.com/10017612529; Naver,
  https://m.blog.naver.com/PostView.naver?blogId=kiss_0153&logNo=222167352174.
- **Black Mage (§14)**: Removed the disputed Entry level note; the file's
  `entry_level: 255` remains. Sources: MapleSEA v183,
  https://www.maplesea.com/updates/view/v183_black_mage_adventures_anew_patch_notes/;
  MapleStory Wiki, Black Mage/Monster,
  https://maplestorywiki.net/w/Black_Mage/Monster; MapleTools,
  https://mapletools.app/bosses/blackmage.
- **Baldrix (§15)**: Replaced the broad unconfirmed-name note with the confirmed
  MapleSEA v246 names Ragnarok and Triad Thunder while leaving other names
  unconfirmed. Source: MapleSEA v246,
  https://www.maplesea.com/updates/view/v246_Patch1/.
- **Kalos (§20)**: Confirmed the ball needs two hits per member on Normal and
  updated the shot-count ledger. Sources: MapleSEA v251 Content 2,
  https://www.maplesea.com/updates/view/v251_Content2/; namu mirror, 감시자
  칼로스, https://m.namu.moe/w/감시자%20칼로스.
- **Seren (§21)**: Renamed Mithra's Power to MapleSEA's Power of Sun and kept
  the former names as aliases. Sources: MapleSEA v235,
  https://www.maplesea.com/updates/view/v235_Patch_Notes/; namu mirror,
  선택받은 세렌, https://m.namu.moe/w/선택받은%20세렌; Orange Mushroom KMS
  v1.2.355,
  https://orangemushroom.net/2021/10/14/kms-ver-1-2-355-haste-tactical-relay/.
