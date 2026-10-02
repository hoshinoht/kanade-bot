# Boss knowledge refresh: proposal (2026-10-02)

Status: **proposal, awaiting user review.** Nothing in `boss/knowledge/` or
`src/` has changed yet.

Goal: make `get_boss_strategy` answers more accurate and useful for a MapleSEA
group. Each guide should give (1) general guidelines that apply to every run and
(2) named strategy options with their risk, damage requirement and payoff, the
way Radiant Malefic Star has safe and greedy altar routes.

Full research (facts tables, SEA terminology tables, ready-to-paste YAML and
sources) is in the git-ignored
`data/research/boss-guides/research-2026-10-02/`:
`report-rms-jupiter-baldrix.md`, `report-fa-limbo-kai.md`,
`report-seren-kalos-lotus.md`, `report-karling-bm-bellona.md`,
`report-meilin.md` + `meilin.yaml`, and `DECISIONS.md`. Every proposed text is
our own paraphrase; it still has to pass `validate.py`'s 12-word guard.

## 1. User decisions recorded

- Region is MapleSEA. SEA patch-note names are primary; GMS/KMS/guide names
  appear once as "also called".
- Seren (all three difficulties) and Kalos (all four) have **8 lives** in SEA.
- **Meilin** (KMS Challengers Season 4, ended in KMS on 17 Sep 2026) stays
  **active** as preparation; SEA is expected to release it around Nov 2026.
  The chatbot must be able to answer it.
- **Kai**: the SEA season ended at the 30 Sep 2026 maintenance; hide it but keep
  the file.
- **Malicia** (SEA v254, from 15 Oct 2026): no guide until a reliable one exists.
- **Bellona** stays in the catalog as a prep guide, labelled "not in MapleSEA yet", like Meilin.
- **Values come from KMS**, because SEA follows KMS. That covers HP for every boss, with no derived
  SEA estimates (so Carling's estimate is dropped). HP and time limits list **both**
  KMS values, labelled: the value before OVERDRIVE (what SEA has now) and the value
  "after OVERDRIVE".
- **No automatic event expiry.** There is no `event.status` or `event.ends` field;
  the user adjusts event guides by hand.

## 2. Schema, renderer and chat changes

| Change | Why |
|---|---|
| Optional top-level `strategies: [{name, when, risk: low\|medium\|high, damage: low\|medium\|high, payoff, steps[1-6]}]` (max 4 per boss) | The user's main ask: named routes with trade-offs. `render_guide` adds a `## Strategies` section after Tips (and the portal Bosses page can show it). |
| `event.aliases: [..]` (no status or expiry field; the user hides events by hand) | `get_boss_strategy` resolves only catalog bosses today (`src/chat/tools/read/mod.rs:184`), so Kai and Meilin can't be answered. Fall back to event documents by key or alias. |
| Render `event.availability` as the first line of an event guide | The bot must say Meilin is not in SEA yet. |
| Spec/force wording: "Authentic Force" in text (the `force.kind: sacred` enum stays) | SEA calls Sacred Force "Authentic Force" and the symbols "Authentic Symbol". |
| Boss-policy prompt: one line saying strategies are options, not orders, and to name the trade-off | Stops the model presenting a high-risk route as the default. |
| `fetch.py`: add the iSIingGunz Lotus doc (`1ozaBBT0D7rZJr_KurQYNHLdKJU7PWw437bq5auyom5c`); the text is already cached as `lotus.txt` | Keeps the anti-copy guard covering Lotus. |

`validate.py`, `schema.json`, `README.md`, the API DTO (`src/api/dto/bosses.rs`)
and chat tests need matching updates. The v4 copy stays frozen.

## 3. Facts that apply to every guide

- **OVERDRIVE has not shipped in SEA.** KMS 1.2.416 (Jul 2026) cut HP about 32%
  on most weekly bosses (Black Mage Hard about 65%) and set every weekly or
  monthly boss to 20 minutes. SEA is on v254, with an OVERDRIVE countdown
  running 14 Oct to 24 Nov 2026. Current HP values and 30-minute limits are
  right for SEA today. Per the user's decision, each difficulty lists both
  values, e.g. `hp` rows for the current value plus a note giving the
  post-OVERDRIVE value and limit. Drop the old value once SEA ships OVERDRIVE.
- SEA v251: Symbol Growth Level 11 gives +20% damage against that area's boss
  (Cernium vs Seren, Hotel Arcs vs Kalos, Odium vs First Adversary, and so on).
- SEA v249: a cap of 12 boss kills per week. The interaction key is the
  "NPC/Gather key".
- Sol Hecate is in SEA (v252), so the guides' "post Sol Hecate" HEXA specs now
  apply.

## 4. MapleSEA glossary (headline terms)

| Boss | SEA term | Also called (GMS/KMS/guide) |
|---|---|---|
| Seren | **Chosen Serene**, Mithra, Mithra's Power check | Chosen Seren, Ray of Light |
| Kalos | **Gatekeeper Kalos**, Will of Kalos, Watcher's Roar | Kalos the Guardian, Guardian's Roar ("FMA") |
| Lotus | **annihilation gauge** | Purge Gauge |
| First Adversary | **Will of the Adversary**, Power of Order, Cycle of Power, Sword Shower | Enemy's Will, Greatsword Shower |
| Carling | **Carling**: Gunggi / Do'oul / Hondon, Mental Strength, Balance of Season | Kaling: Goongi / Dool / Chaos, Willpower |
| Black Mage | Curse of Creation / Destruction, Aion of Creation, Yaldabaoth of Destruction, Egg of the Beginning | Aeonian Rise, Tanadian Ruin, Genesis Crux |
| Limbo | **Purification Energy** | Spirit Purification, Purification Aura |
| Jupiter | **Rupture** (from an SEA achievement name), Grand Authentic Symbol: Geardrock | Disruption, Geardock |
| Baldrix | Magic Erosion (supported by an SEA achievement name), Talahart | Magic Encroachment |
| Radiant Malefic Star | SEA mechanic names unconfirmed | Predation/Hunger, Awakener/designated player |

## 5. Per-boss changes

| Boss | Current state | Main fixes | Strategies to add |
|---|---|---|---|
| **Radiant Malefic Star** | Good, but routes squeezed into two tips | Swap skill has a 30 s cooldown and is locked during Essence/altar; hit numbers are solo (a trio takes ~⅓); forced swap has a ~5 s cutscene; Normal HP 3.288q almost certainly correct for SEA | Fully safe (low risk / high damage), Flexible standard (medium/medium), Hold +30% (high / low), Burst-cycle (very high / lowest) |
| **Jupiter** | Good | Lead with "Rupture"; drift starts ~5 s after leaving blue; Phase 3 Command shoves the needle; Auspicious success gives everyone -10; bind resistance per side, merged boss inherits the longer | One on Jupiter (iSIingGunz) vs Two on Jupiter, switch in Phase 3 (Inven) |
| **Baldrix** | Good | Rooms follow party **join order**, not list order; "4/3 rifts" = 4 on Normal, 3 on Hard (+500 and reset); Phase 2 Overflow → Erosion table | Phase 1 room assignment; Clean Phase 2 (zero Marks → shield vs first Ragnarok) |
| **First Adversary** | Stale | SEA v249 keeps a stage 4+ gauge through the Cycle (the "unconfirmed" note is wrong); decay never drops below the current stage; party scaling (losses not reduced on Extreme); SEA names | Parry the opener vs Bind on entry; Burst on cooldown; Sniping parry vs avoid |
| **Limbo** | Mostly right | "Purification Energy"; v251 allows it during Origin/Ascent and makes the Phase 3 map ~32% taller; drop the "per MapleTools" hedge; replace the unsourced "stagger Purifications" tip with "everyone presses on full"; Hard hit values | Skip the second Specter C fusion vs Play through both |
| **Seren** | Thin v4 file, no difficulty facts | Full difficulty facts; 8 lives; SEA v251 (binding in Dawn removes deer, orb HP -90%), v244 (Midnight regrowth), v246 (gauge reset at Phase 2); crouch rules | Midnight burst (party), Kill before Dawn, Solo rhythm play |
| **Kalos** | Thin | Full facts; 8 lives; SEA v251 timers 80/70 s on Easy/Normal and fewer shots; weapon positions and hit counts; Phase 2 dome tests; 2-4 curse | Left-first control, Prep and hold (Chaos/Extreme), Threshold burst |
| **Lotus** (Extreme) | Thin | Annihilation naming; Extreme facts (285, 380%, party of 2, no force); SEA v244 removed the Phase 3 ceiling cannon and cut bombing; safe spots for horizontal bombardment | Duo standard, Phase 3 drain discipline |
| **Carling** | Thin, GMS names | SEA names; per-death cost by party size; clean Phase 1 restores **300**; Phase 3 zone rules (v251); KMS HP before and after OVERDRIVE | Balanced gauges vs Do'oul turbo break build (high risk, low damage) |
| **Black Mage** | Thin | 12 lives, 60/30 min, Origin-only binds from Phase 2, PDR 300, Laser Jail safe spots, Phase 3 platform rules, SEA knight names | Phase 4 affinity default (Destruction), Phase 4 burst-only, Laser Jail invincibility |
| **Bellona** | KMS-based, no facts | **Not in MapleSEA**: say so up front; Death Count 8/8/5, Insane gauge numbers, Berserk weapon sequence, Phase 2 below-50% additions | Survival first, Party life-sharing (mark as interpretation) |
| **Kai** | Event, season ended | Availability in the past tense; the user hides it by hand | none |
| **Meilin** (new) | — | New `meilin.yaml` (key `Meilin`, aliases Maerin/메이린), KMS facts, "expected in SEA around Nov 2026" | Full mechanics, Invincibility skip, Pure damage check, Min-spec Order rerolls |

## 6. Open questions

Resolved 2026-10-02: Bellona is a prep guide; values come from KMS, with both
the pre- and post-OVERDRIVE values; no event expiry field. Still open, worth an
in-game look: Radiant Malefic Star Normal HP; Jupiter Hard forced-merge parity
rule; whether First Adversary's decay floor applies on Hard/Extreme.

## 7. Artwork (private, git-ignored slots)

Candidates are in the session scratchpad (`art/`), sourced from Namu Wiki, the
official KMS promo page and Orange Mushroom. Meilin has a clean entry image
(778×556, from Namu's entry animation), an icon (66×67) and an upscaled portrait
(247×267). Kai has an icon and portrait. The entry animations for the other
bosses (First Adversary onward) are being collected. Nothing goes into `boss/`
until the user approves.

## 8. Implementation plan (after approval)

1. Schema v3 additions (`strategies`, `event.aliases`), with
   `validate.py`, README and `fetch.py` (Lotus) updated.
2. Rust: `render_guide` strategies section; event fallback in
   `get_boss_strategy`; `availability` line; API DTO; chat and API tests
   (invented fixtures only).
3. Knowledge files: bosses in the order RMS, Jupiter, Baldrix, FA, Limbo, Seren,
   Kalos, Lotus, Carling, BM, Bellona, then Kai and Meilin. Run `validate.py`
   after each, plus a manual pass against the research reports.
4. `REVIEW.md` rewritten for this import, a CHANGELOG entry, and a workplan
   decision-register line.
5. Full CI-equivalent checks (fmt, clippy, tests, build), then commit only on
   request.
