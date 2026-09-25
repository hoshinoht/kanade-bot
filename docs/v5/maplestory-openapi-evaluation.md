# MapleStory Open API and MapleScouter: evaluation

Status: **proposal** (research 2026-09-26). A v5 feature candidate, planned after
pseudonymization. Nothing is built yet.

Question: can Kanade, for a MapleSEA guild, use Nexon's official Open API and/or
MapleScouter to show character stats and boss readiness? What coverage, terms,
risks and first slice make sense?

## Verdict

**Go, with constraints.**

- **MapleSEA has an official Nexon Open API** (launched April 2025, data from
  2025-04-20), on the same platform as KMS and TMS: base URL
  `https://open.api.nexon.com/maplestorysea/v1/...`, auth header
  `x-nxopen-api-key`, about 26 endpoints (character, union, guild). GMS, JMS
  and CMS have none.
- **It has no boss data.** Boss HP, force and level requirements stay in
  `boss/knowledge`.
- **The terms are the binding constraint:** non-commercial; attribution "Data
  based on NEXON Open API"; retained data kept **at most 30 days** and
  refreshed at least every 30 days; a posted privacy policy; delete a player's
  data on request; no transfer of game data to third parties.
- **MapleScouter** (a Korean stat-equivalence / boss-cut calculator that also
  supports MSEA) has **no public API** and no terms page. Link to it; never
  scrape it.

## Access and limits

- A key comes from registering an application with a NEXON ID (up to 2 keys per
  app, 3 apps per game per ID; quota per application). Malaysia is not in the
  ToS exclusions and several SEA tools exist, so a key is very likely
  obtainable; the login flow was not tested.
- Development-stage key: **5 requests/s, 1,000/day** — ample for a guild
  (about 50 characters × 4 calls ≈ 200/day). The service stage (500/s,
  20M/day) is not needed.
- Freshness (SGT): the previous day's data from 02:00 SGT; game data appears
  about 15 minutes after an update; `date=YYYY-MM-DD` selects a snapshot; the
  `ocid` may change. Only characters that logged in after 2025-04-20 are known.
- Errors: 429 `OPENAPI00007` (limit); `00009` data being prepared; `00010`
  (400) and `00011` (503) maintenance — treat these as soft outages.

## Endpoints that matter (all under `/maplestorysea/v1/`)

| Need | Endpoint | Returns |
|---|---|---|
| Resolve a character | `id?character_name=` | `{ocid}` |
| Class, level, world, guild | `character/basic?ocid=` | name, world, class, level, guild, … |
| Combat power, HP, force | `character/stat?ocid=` | `final_stat: [{stat_name, stat_value}]` (strings) |
| Symbols / force | `character/symbol-equipment?ocid=` | per-symbol force and level |
| Guild | `guild/id?guild_name=&world_name=`, `guild/basic?oguild_id=` | member **names** only |

Also available: hyper-stat, ability, equipment, set effects, skills, link
skills, V/HEXA matrix, dojang, union. Not available for SEA: rankings,
starforce/cube history, account character lists, any boss data. Shapes come
from third-party client DTOs; SEA `stat_name` labels and symbol fields are
**unverified** until a real response is recorded.

## Feature ideas

1. **`/link character <name>`** (opt-in): `id` → `basic`; confirm the world is a
   SEA world and the guild matches; store `{discord_user, ocid, name, world}`;
   re-resolve by name when the `ocid` is rejected.
2. **Profile card** (class, level, CP, HP, force) from `basic`, `stat` and
   `symbol-equipment`, always with its as-of date and the attribution.
3. **Deterministic readiness gate** against `boss/knowledge`: level ≥
   `entry_level`, force ≥ `force.value` (and the "recommended" note where
   present), party size ≤ `party_max`; ✅ / ⚠️ / ❌ with reasons, plus a
   MapleScouter link for a damage estimate. It never claims clear-ability.
4. **Run-sheet and party-planning chips** (class, level, force per
   participant) from cached data only.
5. **Chatbot tool `check_readiness(boss, difficulty)`** computed in Rust; the
   model sees only the verdict and numbers, never the character name.
6. **Deferred:** guild roster sync (`guild/basic` names every member, including
   those who have not opted in, and needs one lookup each).

## Party clear estimate (user model, 2026-09-26)

MapleScouter supports MSEA characters and shows, per boss and difficulty, a
character's "cut" percentage. Its party-spec feature works only for KMS
characters (it breaks on SEA), so Kanade would provide the party view itself.

- **Scouter %:** clear time = 30 min × 100 / % (100% = 30 min, 150% = 20 min,
  200% = 15 min). The basis stays 30 min for every boss, including Black Mage.
- **Party estimate:** boss HP does not scale with party size, so party % ≈ the
  sum of the members' % for that boss and difficulty; estimated time = 30 × 100
  / party %.
- **Clearable:** party % ≥ 100 × 30 / time limit. Time limits today: 30 min for
  most bosses (100% needed), Black Mage 60 min (50%), Extreme Black Mage 30 min
  (100%). A future update lowers the limit to 20 min for all bosses (150%).
- **Caveats:** an estimate only — it ignores party buffs and synergies, deaths,
  invulnerable phases and mechanics. Show time and margin (e.g. "≈ 13.6 min ·
  1.47× the 20-min limit") and ✅ / ⚠️ / ❌, never a guarantee.
- **Uses:** run-sheet and planner verdicts per run; "who would make this party
  clear" hints; a chatbot readiness answer computed in Rust.
- **Boss knowledge:** add a per-difficulty time limit in minutes plus one
  setting for the 20-minute era; the 30-minute scouter basis is a constant.
- **Source of each member's %:** MapleScouter's terms (이용약관, effective
  2026-07-11, `https://maplescouter.com/ko/agreement`) forbid automated access
  to the site or its API (§15.1), using its calculations as another service's
  backend or data source (§15.2), reusing its API keys or endpoints (§15.3),
  and reverse engineering (§15.7); its calculation logic and processed data
  belong to the operator (§14); violators may be blocked without notice (§17).
  There is no non-commercial exception. The site does have a private backend
  (`api.maplescouter.com`, keyed with a key embedded in its bundle, custom 430
  rate limit, Cloudflare, `msea` region), but it is not a public API.
  - **Primary path:** ask the operators (maplescouter@gmail.com) for written
    permission or an API key; use their % only on the terms they set (e.g.
    once per character per day, cached, identifying User-Agent, credit
    "Data: MapleScouter (maplescouter.com)").
  - **Fallback now:** members enter their own observed clear times (clean) or a
    self-assessed % per boss and difficulty (retyping MapleScouter numbers is a
    grey area under §14), dated, with a 30-day expiry.
  - **Optional later:** Kanade's own independent estimate from Nexon Open API
    data (never a port of MapleScouter's logic; it will not match their %).
  - **Never:** scraping or calling their endpoints without permission.
  - MapleScouter's page also has a "Boss Timer" setting — record which timer a
    % was taken under, or require the 30-minute basis.

## Privacy, security and operations

- Opt-in linking only; `/unlink` hard-deletes at once (ToS §9.1).
- Store only fields in use (not login activity or images); purge snapshots
  after at most 30 days via the monotonic retention scheduler.
- Post a short privacy notice (§8.6) and show "Data based on NEXON Open API"
  wherever the data appears (embed footers, the admin app).
- Model boundary: character names identify people — add them to the
  pseudonymization alias set or keep them out of prompts. Send Nexon data only
  to homelab-zone models; treat external routes as off-limits for raw game data
  even when pseudonymized (conservative reading of §8.3).
- Key in a 0600 file (e.g. `KANADE_NEXON_API_KEY_FILE`), never in env or logs.
- One daily refresh batch after 02:30 SGT under 5 requests/s; on-demand refresh
  at most once a day per character; on 429/5xx back off and serve the cached
  copy (within the TTL) with its as-of date.
- Tests use fake HTTP stubs only; record DTO fixtures once from real responses.

## First slice (MVP)

- A `nexon` provider module: key file, rate limiter, 30-day TTL cache.
- `/link`, `/unlink` and `/me` (profile card with attribution).
- The deterministic readiness gate from `boss/knowledge`.
- A MapleScouter link-out.

**Defer:** roster sync, chatbot exposure (until character names are part of
pseudonymization), HEXA/equipment analysis, any damage or clear-time estimate,
MapleScouter beyond links, the service-stage key.

## Open questions

- Can a Malaysian NEXON ID register an application? (Untested.)
- Live SEA `final_stat` labels and symbol fields; "Sacred" vs "Authentic"
  force naming (our schema says `sacred`).
- Does a query without `date` return near-real-time data or the daily snapshot?
- MapleScouter's terms and deep-link URL format.
- The developer privacy policy linked from the ToS (not read).

## Sources

All accessed 2026-09-26.

| Source | Used for | Limits |
|---|---|---|
| MapleStorySEA, "Introducing MapleStory OpenAPI" (2025-04-21), <http://www.maplesea.com/news/view/SEA_OpenAPI> | SEA API exists; consent basis; SG/MY privacy law | Announcement-level detail |
| NEXON Open API guides, <https://openapi.nexon.com/guide/prepare-in-advance/>, <https://openapi.nexon.com/guide/request-api/> | Keys, quotas, header, error codes, attribution | No regional eligibility detail |
| NEXON Open API Terms (updated 2025-04-21), <https://openapi.nexon.com/support/terms/> | 30-day TTL, non-commercial, privacy policy, deletion, no third-party transfer | "Third party" open to interpretation |
| NEXON Open API MSEA page, <https://openapi.nexon.com/game/maplestorysea/?id=45> | Only "update at least every 30 days" | JavaScript-rendered; not verified |
| SpiralMoon, maplestory.openapi (v3.11.0, 2026-07-07), <https://github.com/SpiralMoon/maplestory.openapi> | MSEA paths, DTOs, freshness, region matrix | Third-party mirror |
| Kimhasa, nexon-open-api-sdk `docs/maplestory-sea.md`, <https://github.com/Kimhasa/nexon-open-api-sdk/blob/main/docs/maplestory-sea.md> | 26 endpoints, SGT, 02:00 refresh, worlds | Third party |
| Maple Scouter, <https://maplescouter.com/> (and `/input`, `robots.txt`) | Features, MSEA support, no API | JavaScript-rendered; no ToS found |
| Inven guide (2026-06), <https://www.inven.co.kr/board/maple/2304/47723> | Stat-equivalence and boss-cut semantics | Secondary, KMS-focused |
| MapleSecrets (2025-05), <https://maplesecrets.blogspot.com/2025/05/maplesea-openapi-related-projects.html> | MSEA region toggle; post-2025-04-20 characters only | Secondary |
