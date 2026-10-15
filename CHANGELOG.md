# Changelog

What changed in each Kanade release, newest first. Upgrading across several releases? Read each one's upgrade notes on the way.

## 1.0.0-beta.8

_In development_

### New features
- A linked character's roster class and level now take NEXON's values on every refresh, recorded in History as NEXON, and stay on the roster after an unlink or once the NEXON data expires, so other members see the current level.
- Added "Who carries", a bar per boss showing each member's share of the party's estimate, to the run sheet's Clear-time estimate panel and the member portal's run Party · characters, where the run's total (minutes, %, technically possible) now shows too.
- Redesigned day-of and countdown reminder cards now carry a "Who carries" image showing each boss's estimate split between the party members by IGN (or "no estimate yet"), from estimates looked up in the last 24 hours and fixed at the time the card is posted. The image is left off while an estimate lookup for anyone in the card's party is still queued or running, so members still being looked up never show as "no estimate". The admin Reminders card preview shows the same image, drawn from the estimates cached at the time you open it.

### Changes
- Estimates are now labelled “Estimate” on the portals and in Discord.
- An admin's Link… (and Add character for a MapleSEA character) now links it at once and fetches its NEXON data, without waiting for the member to confirm; link requests still waiting from before become active on the first start.
- The admin app, the member portal and Discord now say that unlinking keeps the class and level on the roster, and "Waiting for you" now only shows links waiting for the member to confirm the notice.
- The NEXON link notice and the Privacy page now say that a linked character's class and level are copied to the roster, stay after an unlink and are seen by guild members; members who linked a character themselves are asked to confirm the new notice before it refreshes again, while characters an admin linked stay linked.

### Fixes
- Fixed Config never showing the "Manage Messages missing" warning when the bot lacks Manage Messages in a channel it uses.
- Fixed phones cutting off a member's runs in the Week's Answers view and the Reminders count running past the window edge.
- Fixed low-contrast "still a question" chips on the selected Inbox item in the Sumire dark and Catppuccin light colourways.
- Fixed Config › Persona showing only a reply profile's first line when you open its whole prompt.
- Fixed Fixed timings asking Keep or Update for a run that had already ended this week, which then made the save fail.
- Fixed Config › Models resetting a "Same as extraction" role to Off when its model requires reasoning; it now takes the model's lowest level, so the save goes through.
- Fixed History listing every reminder card of a moved run instead of one "reminders re-placed" line.
- Fixed a retired character's NEXON data disappearing at once; it now stays, with its deletion date, until it expires.
- Fixed the member portal's Move page starting a sentence with a lowercase "an admin".
- Fixed the Inbox toast for a refused edited approval blaming the member, when another admin had changed the request.
- Fixed screen readers reading the tick on picked party members in the Fixed timing editor as part of the name.
- Fixed Fixed timings asking Keep or Update on saves that keep the day and time, or about runs changed only in their party, which made the save fail; it now asks only about runs the new day or time would move.
- Fixed the bot skipping that day's morning card when it starts after the ping time; a due card for a run still ahead now posts, up to 12 hours late.
- Fixed four security issues in the bot's TLS library (RUSTSEC-2026-0346 to 0349) by updating it.
- Fixed the bot proposing to move runs earlier when a message moved them "after 0930 17th": dates like "17th" or "17/10" now count, "0930" is morning, and runs already after (or before) a stated time get no proposal.

### Removed
- Removed the one-off `kanade import v4` command; v4 data already imported stays readable.

## 1.0.0-beta.7

_13 Oct 2026_

Character data from NEXON and a simpler setup: members link and manage their MapleSEA characters, admins see who brings which character to a run and how ready they are, and a fresh host needs only a few settings to start. This is the first release on `master`; v4 is retired.

> [!IMPORTANT]
> - This release adds store migrations: to go back to an older image, restore the pre-deploy tarball ([runbook](deploy/README.md#deploy-an-update)), as an older image refuses the migrated store.
> - Character data is off by default and needs the roster module: `modules.character_data` without `modules.roster` refuses startup. It keeps its data on its own volume, which is never backed up.
> - If an earlier build of this release ran with character data on, move the character-data file into `db/` before the first start, or every link is lost ([runbook](deploy/README.md#deploy-an-update)).
> - Compose now runs from `deploy/` without `-f`, and the base file no longer holds a host's reverse proxy, admin host, trusted proxy or Discord sign-in secrets: copy `deploy/compose.override.example.yaml` to `deploy/compose.override.yaml` for those, move any `.env.v5` settings into `kanade.toml`, and delete `discord.expect_v4_stopped`, which now stops startup ([runbook](deploy/README.md#deploy-an-update)).

### New features
- Added the optional character data module, linking roster characters to NEXON Open API (MapleSEA) data: class, level, Combat Power and image.
- NEXON data is kept at most 30 days, refreshed daily and never stored in History, backups, logs or model prompts; Config shows Character data health.
- Live NEXON calls need the NEXON API key file (`KANADE_NEXON_API_KEY_HOST_FILE` in Compose); without it, links are refused.
- Added `/character link`, `/character unlink`, `/character data` and `/character share`, so members consent to, remove, view and share their NEXON data.
- Share cards are cleared after 29 days or on unlink, and admin link requests a member doesn't confirm expire after 14 days.
- Edit character now has a Character data strip with NEXON values beside the roster's, and Link…, Unlink… and Refresh.
- Add character can look up a MapleSEA character on NEXON to preview its class, level and world; nothing is kept until the member confirms the link.
- Added the public `/privacy` page that the character data notice links to.
- The run sheet's Party tab names the character each member brings: the run's pick, else the weekly timing's pick, their main or their only character.
- The Party tab warns, without blocking, when a character is already on another run this boss week for a shared boss.
- With character data on, the Party tab shows each character's level and Combat Power with source and date, plus a level verdict per boss.
- Added scouter clear-time estimates for a run's party or one character, marked for planning only; set `KANADE_SCOUTER_URL` to a private scouter to turn them on ([runbook](deploy/README.md#scouter-estimates)).
- The Fixed editor gains a Characters tab to pick the character each member brings to a weekly timing, with the same readiness lines.
- Added Account › Characters on the public portal, where members add, edit, retire and link their own characters and run their own estimate lookups.
- A run's pane on the public portal shows Party · characters: each member's IGN, class and level, and cached estimates per boss.
- Inbox member requests and the run sheet show which other admins have them open and who is editing; it's a hint, never a lock.
- History has a Graph tab that draws your drafts as lanes beside the changes, with keyboard navigation and Go to #.
- Redesigned day-of and countdown cards end with an "Open in portal" button while the public portal is open.

### Changes
- A linked character's IGN now takes NEXON's exact letter case when the two differ only in case, recorded in History.
- The base Compose file now starts one bot with its admin portal at `http://localhost:8080`; a reverse proxy, Discord sign-in and other host-specific parts go in a git-ignored `deploy/compose.override.yaml`.
- `kanade.example.toml` now leaves uncommented only what a Compose setup needs, and the runbook opens with a Quick start that covers the Discord application's intents and permissions.

### Fixes
- Fixed the Week's Runs list placing When, Status, On and Channel at the top of taller rows.
- History answers now name the run's bosses ("→ on for HCarling + HStar") instead of a run id.
- Fixed the run sheet's notice line and Undo covering the status buttons on laptops.
- Fixed `kanade restore` refusing a good snapshot on Linux hosts ("the restored copy left a -wal file"); the store now always closes cleanly.
- The portal's My data now lists characters in a stable order: waiting requests, the main, other active characters, then retired ones.

### Removed
- Removed the v4 start guard (`discord.expect_v4_stopped`); `kanade import v4` is unchanged.
- Removed the optional `.env.v5` settings file; settings live in `kanade.toml`, Compose variables in `deploy/.env`.

## 1.0.0-beta.6

_12 Oct 2026_

Drafts and a character roster: admins can stage schedule changes and merge them as one, edit a member's request before approving it, and keep each member's characters.

> [!IMPORTANT]
> - This release adds a store migration: to go back to an older image, restore the pre-deploy tarball ([runbook](deploy/README.md#deploy-an-update)).
> - Persona, boss knowledge and boss catalog files must now be regular files (not symlinks) of at most 256 KiB; anything else fails the load.

### New features
- Kanade can run against other OpenAI-compatible model servers: declare each model's abilities in `[models.capabilities."<alias>"]` ([guide](docs/v5/runtime-bootstrap.md)).
- Added the optional roster module (`modules.roster`), where admins keep each member's MapleSEA characters with class, level, and main or alt; characters are never sent to models.
- Added a Drafts page: stage changes, see what a merge would write, settle conflicts with Keep mine or Take theirs, then merge as one change.
- The run sheet's "Stage in draft…" sends Move, Swap, party, Reset and Status into a draft instead of going live.
- Inbox has "Edit before approving" for member requests, to change the weekly day and time or drop some changes.
- Members are told when a request was "approved with changes by an admin" and what wasn't approved, in Discord and on the portal's Requests screen.

### Changes
- Rewrites, Limits and Reminders moved under a More item at the foot of the navigation rail.
- Persona and knowledge loading stops at 256 profiles or documents, and an unreadable knowledge entry now fails the load instead of being skipped.

### Improvements
- The run sheet's boss art spans from the boss names to the card's edge, with text on soft plates and contrast checked in every colourway.

## 1.0.0-beta.5

_11 Oct 2026_

Safer data and an operator toolkit: encrypted backups, an offline restore, and shell commands for the portal, sessions and the admin token.

> [!IMPORTANT]
> - This release adds a store migration: to go back to an older image, restore the pre-deploy tarball ([runbook](deploy/README.md#deploy-an-update)).
> - The first start deletes chat, extraction and rewrite logs older than 90 days, which had never been pruned; **this can't be undone**.
> - Outside Compose, `KANADE_PUBLIC_BIND` now refuses startup without `KANADE_CLOUDFLARED_PEER`; Compose already sets it.

### New features
- Added encrypted backups: point `KANADE_BACKUP_RECIPIENTS_HOST_FILE` at age public keys and snapshots are written encrypted ([runbook](deploy/README.md#encrypted-backups)).
- Added `kanade backup keygen`, `backup encrypt` and `backup decrypt`, and History › Checkpoints lists encrypted backups.
- Added `kanade restore`, an offline restore that checks the snapshot, keeps the previous store beside it and is a dry run without `--apply`.
- Added `kanade portal open|close|status` and `kanade sessions list|revoke|sign-out-all`, run against the live bot with the break-glass token.
- Added `kanade token rotate` to write a new break-glass admin token; a restart applies it.
- The public portal now updates live when the week, your runs, requests or allowance change.

### Changes
- Trusted Types are now enforced on both portals, so unsafe HTML or script injection is refused rather than only reported.
- History › Sign-ins also lists refused member writes, with the refusal reason.
- Member edits from the admin portal and from Discord `/pings`, `/style` and `/nick` are now recorded in History.

### Fixes
- Fixed members being able to skip the weekly-timing owner check, close others' ownership requests or probe owners by retrying.
- Hardened the public portal: no raw channel ids, 120 data reads a minute per member, and sign-in only from a real page visit.
- Model logs are now pruned after 90 days, daily.
- Reduced motion now shows the arrival colour fade it promised.
- When the models fail to switch after a Config save, Config now keeps saying so until the switch succeeds.
- History and the Inbox count no longer show mismatched numbers when a change lands mid-read.

## 1.0.0-beta.4

_10 Oct 2026_

The public portal beta and run ownership: members sign in on the internet to see the week, answer and move their own runs, send requests and hand over weekly timings.

> [!IMPORTANT]
> - This release adds store migrations: to go back to an older image, restore the pre-deploy tarball ([runbook](deploy/README.md#deploy-an-update)).
> - Existing weekly timings now belong to their party's first member unless an admin pins an owner; check Weekly timings after upgrading.
> - Member sign-in needs its own Discord application and the `public_discord_client_secret` file, which must exist (empty is fine) ([runbook](deploy/README.md#member-sign-in-public-discord-application)).
> - A model context reserve of 15360 tokens or more now refuses startup.
> - The v4 rollback is retired; roll back only to an earlier v5 image.

### New features
- Members sign in to the public portal with Discord (bossing role only), and Account lists their devices with sign-out.
- The public portal shows the boss week, My runs, a Weekly timings tab, your chat allowance and the Bosses guides.
- Members answer In, Maybe or Out and move their own runs on the portal, and ask admins for changes through Requests.
- Half an hour after a run ends, Kanade asks its channel whether it happened; unanswered runs are marked done at day's end or reset.
- Weekly timings can change owner: `/fixed owner` hands one over or asks for it, and admins decide asks in Inbox › Ownership.
- Change cards from Discord link into the portal to edit the run or request a change, filled in.
- Added History › Sign-ins, a stored audit log of sign-ins, refusals and ended sessions for both portals, kept 90 days.
- Added a "Discord message style" setting: classic (default) or redesigned cards, digest, `/schedule`, notices and proposal cards with Apply and Reject.
- Reminder headers are rewritten in a daily batch at a time set in Config, and `/debug rewrite` or a Config button rewrites this week's on demand.
- Added a Rewrites log beside Chat and Extractions, with each rewrite's prompt, verdict and tokens, and Copy transcript on Rewrites and Extractions.
- `kanade ctl emojis` uploads the difficulty pill emojis that cards use.
- `/debug ping` can post sample runs, a digest or a chosen style, and `[discord] test_channel` sets where test posts go.
- Boss entry art plays its animated clip, muted and looping, wherever it appears in both web apps.
- Chat replies to a reminder or proposal card now know which runs it's about, so "move it" moves that run.

### Changes
- Ended runs are frozen: moves, swaps, amends and late answers are refused with "That run has already ended."
- "Mine" now means runs and weekly timings you're on, in `/schedule`, `/fixed list` and the digest's My runs.
- Classic countdowns and digest use v4's text again; persona phrases appear only in the redesigned style.
- A change notice's "via portal" mark links into the portal while it's open.

### Improvements
- The public portal now moves like the admin app and previews as the bot when a link is pasted in Discord.
- Both web apps are served compressed, so the admin app loads much less data.
- Radiant Malefic Star's two high-risk routes now follow a published video guide.
- Kanade recovers RSVP reactions made while it was disconnected.

### Fixes
- Fixed chat posting cards for runs you didn't mean, and questions naming several members now list each person's runs separately.
- A slow model round no longer fails a chat question at 30 seconds.
- A stray "no" or "ok" in a channel message is no longer taken as an RSVP.
- Shutdown now fits inside Docker's 30-second stop grace without sending anything twice.
- Creating a weekly timing is now one change, and retried requests replay after a restart instead of applying again.

### Removed
- Removed the v4 (Python) rollback; v4 stays in git history and `kanade import v4` remains.

## 1.0.0-beta.3

_6 Oct 2026_

A redesigned admin app that updates live, a backup command, a profanity guardrail for chat, and rewritten boss guides.

> [!IMPORTANT]
> - This release adds store migrations: to go back to an older image, restore the pre-deploy tarball ([runbook](deploy/README.md#deploy-an-update)).
> - Compose now mounts the backups directory; create it before `up`, or the bot won't start ([runbook](deploy/README.md#deploy-an-update)).

### New features
- Admin pages update live when something changes elsewhere, and moved or new items glide in or are marked once.
- Added `kanade backup`, and History › Checkpoints verifies the history chain and lists backups with whether each still matches.
- Added an optional cloudflared sidecar (`--profile public`) for the public portal ([runbook](deploy/README.md#public-portal)).
- Added a chat profanity guardrail with a configurable in-character reply, set under Config › Profanity.
- Boss guides are rewritten as shorter tabbed pages with strategies, Destiny and Union Champion entries, and animated boss art.
- Inbox has a Past tab of closed items, and each item says what approving would do ("Adds a member", "2 reminders will move").
- Runs have a Changes tab listing every change, newest first, with fields before and after.
- Account shows how you signed in, your allowance and sessions, and has Reduce motion and "Open Discord links in the app" switches.
- Members and admins now show their Discord avatars.
- Reminders rows open a preview of their Discord card.
- Weekly timings have an owner you can pick in the Fixed editor.
- Added a Move picker, date picker and searchable dropdowns across the admin app.
- Chat answers schedule questions in the persona's own words with the run's card under them.
- History now records Config saves and Limits window clears.
- Added `/debug reminders` and `/debug materialise`.
- A "Quiet mode on" chip shows in the page line while quiet mode is on.

### Changes
- Config › Channels lists save on their own and then override their `KANADE_WATCH_*` and `KANADE_CHAT_CATEGORY_IDS` seeds.
- Re-read buttons are off, with the reason, while extraction is switched off.
- Colourways are grouped into sets, and Coral is retired (a saved choice falls back to Otonose).
- Champion and Destiny difficulties show only in boss guides, never in the scheduler.

### Improvements
- Week, Limits, Reminders, Chat, Extractions and Config are redesigned.
- Re-reads and other jobs show progress bars, and Reminders times follow the server clock.
- Boss guides got an accuracy pass, and the Easy difficulty pill no longer blends into grey cards.

### Fixes
- Open admin pages now pick up restored data after a restart instead of keeping the old week.
- Saving a Config switch no longer freezes the channel lists from the environment.
- "When is my next run?" now finds runs early next calendar week and answers with just the soonest one.
- Fixed the Week channel filter, which filtered nothing.
- Avatar initials now skip emoji and symbols.

### Removed
- Removed the unused `kanade export` command.

## 1.0.0-beta.2

_2 Oct 2026_

The admin app takes its new look, chat answers boss strategy questions, and run lengths drive the planner.

> [!IMPORTANT]
> This release adds store migrations: to go back to an older image, restore the pre-deploy tarball ([runbook](deploy/README.md#deploy-an-update)).

### New features
- Chat answers boss strategy questions from the boss knowledge, including event bosses by name.
- Boss guides are refreshed for MapleSEA with per-difficulty facts and named strategy options.
- Admin Bosses lists event bosses with their season.
- Run lengths are a saved setting per boss and difficulty, and planner drops now set the time as well as the day.
- Admins can swap two runs by dropping one card on another or with `S` on the keyboard.
- Limits and "Post digest" now work against the real server.
- Kanade posts v4's decline notices and weekly-timing added and removed notices again.
- After a ❌ on its own proposal card, chat posts one quiet follow-up.
- Chat shows a typing placeholder while it answers, then edits it into the answer.
- Model context windows are live settings in Config › Models.
- Model logs keep reasoning text and token counts, shown in Chat and Extractions.
- Inbox items show the channel messages around the evidence, with the used ones marked.
- ✅ and ❌ given on proposal cards while the bot was offline are applied after it reconnects.

### Changes
- Chat points to another channel's open proposal card for the same change instead of posting a duplicate.

### Improvements
- The admin app has a navigation rail and redesigned Bosses, Inbox, Config, History, Fixed and Members pages.
- Chat trims older history before refusing a question that's too long.

### Fixes
- Re-reading channels while watching is off now says so and points to Config › Watching.
- Long slash-command replies now post their follow-up parts.
- An edit made while a message was being read is now proposed instead of the old text.
- Fixed several chat queue problems, including questions not refunded when their channel left the chat category.

## 1.0.0-beta.1

_28 Sep 2026_

Kanade v5, a rewrite of the bot in Rust with a new admin app, replacing the Python v4.

> [!IMPORTANT]
> - v4 and v5 share the bot token, so never run both: v5 connects only with `discord.expect_v4_stopped = true` (or `KANADE_EXPECT_V4_STOPPED=1`); 1.0.0-beta.7 removed this guard.
> - The Discord token is read from a file only; a plain env value is refused.
> - `KANADE_BIND` is renamed `KANADE_ADMIN_BIND`, and unknown keys in `kanade.toml` stop startup.
> - The removed pseudonymization settings now refuse startup.
> - Store migrations make rollback need the pre-upgrade volume backup.

### New features
- Added a hardened container image and Compose stack ([runbook](deploy/README.md)).
- Non-secret settings live in `kanade.toml`; `KANADE_*` variables still work and override it, and Config saves settings to the database.
- `kanade import v4` brings v4's weekly timings and recent chat and extraction logs across ([guide](docs/v5/v4-import.md)).
- v4's slash commands are ported, and `/status` now also covers cancel, own time, done and restore.
- Reminders and the weekly digest post as v4-style cards with boss art, and the day-of heading is rewritten in the persona's voice.
- Every send goes through a delivery journal, so reminders and notices survive crashes without being sent twice.
- History records every schedule change, and admins can revert a change, restore a week or undo one member's changes.
- Checkpoints, cherry-pick and drafts let admins restore, re-apply or stage changes.
- Edits from an out-of-date screen are refused instead of overwriting someone else's change.
- Members can request a new weekly run, a change, or to join, leave or swap; admins approve or reject in the Inbox.
- An optional attendance mode adds standing "always in" answers and "assume everyone's coming" for weekly runs.
- Kanade reads watched channels and posts proposal cards approved with ✅, and `/rescan` re-reads history.
- The Kanade chatbot answers in the chat categories, with per-member allowances, a queue and refunds when a question fails.
- Personas, reply styles assigned by Discord role, and self-service nudges are configurable.
- Model calls are governed by permit groups and a circuit breaker, and `kanade models check` tests each role's model.
- Added the admin app: Week planner with drag and drop, run sheet, Fixed, Bosses, Members, Reminders, Inbox, Extractions, Chat, Limits, History and Config.
- Admins sign in with Discord, Tailscale or a break-glass token.
- The public portal is served separately and shows a closed page until it opens.
- The admin app shows the bot's live name, avatar and banner.
- `/debug ping` posts test copies of a run's cards.

### Changes
- Kanade v5 is licensed GPL-3.0-only.
- A bare hour after a weekday (`sat 10`) keeps the weekday instead of being read as a day of the month.
- Reminders and notices whose channel is unavailable now go to the post channel with an admin warning instead of being dropped.

### Fixes
- A day-of ping can no longer fire after its run starts on daylight-saving change days.

### Removed
- Removed model-request pseudonymization; configured external models receive raw member data with a warning.
- Removed v4's personal memory, including `/memory` commands.
- Removed `/bot`, `/pingtime`, `/debug status` and `/debug extract` in favour of the admin app.

## 4.9.0 and earlier (v4)

The Python bot, retired when v5 replaced it. Its code and changelog are at the [`v4.9.0`](https://github.com/hoshinoht/kanade-bot/tree/v4.9.0) tag and the `v4` branch; `kanade import v4` brings its data across ([guide](docs/v5/v4-import.md)).
