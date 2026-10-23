# Changelog

What changed in each Kanade release, newest first. Upgrading across several releases? Read each one's upgrade notes on the way.

## 1.0.0-beta.14

_In development_

### Improvements

- The member portal now opens faster: screens other than Week load the first time they are opened.
- Kanade's database no longer grows by a few megabytes a day while nothing is happening, and the daily cleanup now also removes the old delivery bookkeeping that made up most of the file; the operator guide explains how to shrink the file once afterwards.
- Kanade now asks Kanata to reuse its cached prompts for each kind of model call, and the admin Chat, Extractions and Rewrites views show how many prompt tokens came from that cache.

### Fixes

- Kanade and its Cloudflare tunnel no longer trust certificates from the eMudhra certificate authorities whose private keys leaked in October 2026.

## 1.0.0-beta.13

_20 Oct 2026_

Secrets can now be entered, checked and applied from the admin app's Setup page instead of only as files on the server.

### New features

- Admins can now enter the bot token, the model gateway key, both Discord sign-in client secrets and the NEXON key in More › Setup, check a bot token or model key before saving it, and restart Kanade from there to use them; secrets in the server's secrets folder still win and can't be replaced from the web, and saving needs a sign-in from the last 15 minutes.

## 1.0.0-beta.12

_20 Oct 2026_

Configuration now has one home per value: secrets are files in one secrets directory, the image and Compose own every path and address, and `kanade.toml` keeps only deployment settings.

> [!IMPORTANT]
> Removed settings stop the bot at startup and name where each one moved. Before upgrading: save any Config value that Setup shows as coming from the environment; in `kanade.toml`, delete every `*_file`, path, `bind`, `healthcheck_url`, `discord_redirect_uri`, `trusted_proxy` and `cloudflared_peer` key and the `[settings]`, `[store]`, `[files]`, `[backup]`, `[models.extraction]`, `[models.chat]`, `[models.rewrite]` and `[models.context]` tables, moving `boss_week_reset_weekday`/`_time` to `[runtime]`; put each secret in the secrets directory under its fixed name; move the backup age recipients to `recipients` in the backups directory and the tunnel token to `cloudflared_token`; remove the secret and healthcheck lines from `compose.override.yaml` and the variables from `.env`. Take this deploy's pre-deploy snapshot with the rollback image as `deploy/README.md` describes.

### Changes

- Secrets are now files with fixed names in one secrets directory that Compose mounts read-only, so turning on an add-on needs its file and no Compose changes.
- `kanade.toml` now holds only deployment settings: the image sets every path, Compose sets every address, and channels, switches, pings, run lengths and model roles are set in Config.
- The member portal now turns on with `public.host` alone, and the Discord sign-in redirects and the healthcheck target are worked out from the hosts and the admin address.
- Startup errors now name the `kanade.toml` key, removed settings say where they moved, and the bot warns when an environment variable overrides a `kanade.toml` key.
- Config's read-only panel is now "Set in kanade.toml" and lists `kanade.toml` keys, and the chatbot no longer reports missing chat categories as a deployment gap.
- Encrypted backups now read their age keys from `recipients` in the backups directory.

## 1.0.0-beta.11

_20 Oct 2026_

Setup gets a checklist in the admin app, and Kanade now works with other OpenAI-compatible model servers, including ones that publish no model list.

### New features

- Added a Setup checklist to the admin app (More › Setup) that shows which required and optional setup steps are ready or missing, with a link to fix each one, an invite link with the right bot permissions, and a badge counting the required steps left.
- Model servers that publish no model list are now supported: models declared in `[models.capabilities]` can be picked in Config › Models, are sized by their declared context window and show as Declared, and Setup reports the gateway as ready.
- Added the `output_limit` model capability so operators can tell Kanade to send `max_completion_tokens` (as OpenAI's reasoning models require) instead of `max_tokens`.

### Changes

- The digest channel can now be picked in Config › Channels instead of only in the deployment's configuration.
- Declared models now appear in Config › Models and `kanade models check` even when the server's model list leaves them out.
- An exhausted provider quota now fails at once instead of being retried, and is reported as something the operator must fix.
- The admin app, Setup and the startup warning now say "model gateway" instead of naming Kanata, except where they describe limits Kanata publishes.

## 1.0.0-beta.10

_18 Oct 2026_

A polish release: tidier admin and member portal screens, clear error states when the portal's week fails to load, larger tap targets, and reminder cards that keep counting answers until a run ends.

### Changes

- Reminder cards now keep updating with ✅ and ❌ answers after a run starts, until the run's length in Config › Run lengths has passed; only then do they stay as the record.
- The Members roster now keeps each member's stats next to their name instead of across the screen, with names at the same size as the other admin tables.
- Week › Runs on a phone now shows each run as a card that fits the screen, and History › Sign-ins and Reminders keep their first column in view while scrolling sideways.
- Member portal windows now have the same rounder corners as the admin app's.
- The member portal's Requests now name bosses with their portraits and difficulty pills instead of boss codes such as "XKalos".
- The member portal now says "Not in this run · view only" the same way on the card, run pane, list, phone bar and Move screen.
- The member portal's Account › This browser now uses the admin's two-column layout on wide screens, and its tab reads "Browser" on phones.
- In the Fixed editor, a later boss's character picker now lists the character already picked for the first boss at the top.

### Improvements

- Party lines on the member portal's run cards and List tab now wrap instead of cutting names off.
- The member portal's small links and fields (the Week footer links, boss names on Bosses, the Move day-and-time field, the privacy links) are now easier to tap, and History rows in the admin app are a little taller.

### Fixes

- Fixed answers and other connected button choices in the member portal and the admin app changing shape twice on one press or tap (In / Maybe / Out on the run pane, the phone run screen and My runs); a choice now morphs once, straight to its selected shape.
- Fixed the member portal's My runs showing "Loading your runs…" forever when the week fails to load; it now shows the error with Try again, and the status chip no longer says "Loading…".
- Fixed History's change details squeezing the before and after values to nothing when a field name is long.
- Fixed Config › Models showing a blank Reasoning box when the saved level isn't one the model offers; it now shows the saved level and what actually runs.
- Fixed missing spaces around "·" in Extractions, Rewrites, Inbox, History and other admin lists.
- Fixed the member portal's phone week list not always showing runs in time order.

## 1.0.0-beta.9

_17 Oct 2026_

Reminder headers can now take their mood from the party's estimate, cheer on the top carriers by character name and lightly tease members who opted in, all behind two Config switches that start off. Without the switches, the scouter or the character-data module, headers are written exactly as before.

> [!IMPORTANT]
> This release adds the header placeholders and the teasing opt-in to the main store. Rolling back to an older image needs the pre-deploy backup restored with the bot stopped (deploy README, "Stop and roll back"); otherwise the older image refuses to start.

### New features

- Day-of and countdown reminder headers can now be worded from the party's estimate per boss and cheer on the top carriers by character name, when an admin turns on "Estimates in reminder headers"; names are added by Kanade after the line is written, so the chat model never sees them.
- Members can now turn "OK to be teased" on or off on the member portal's Account › Profile, even while teasing is off for everyone, and see the admins' running-joke note there.
- Admins can now set a member's "OK to be teased" and a short running-joke note in the Members sheet's new Reminder teasing section, which refuses a note that names anyone on the roster.
- Added "Estimates in reminder headers" and "Teasing in reminder headers" switches to Config › Notifications; teasing can only be on while estimates are, and turning estimates off turns teasing off too.

### Changes

- The Privacy page and the Characters notes in the admin app and the member portal now say that reminder headers may be worded from the party's overall estimate and the top two carriers' shares, never with a character name, and what teasing may send.

### Improvements

- The Fixed editor's character pick on phones now opens as its own sheet beside the boss list, as designed.

### Fixes

- Fixed redesigned reminder cards posting without the ✅ and ❌ reactions while the portal was open: the bot adds them again beside "Open in portal".
- Fixed the Guides screen's save bar showing without its styles.
- Fixed the "Large entry image" and plan checkboxes on phones being squeezed too small to tick in the Guides editor.
- Fixed trying a guide's Search again a second time after it failed starting a new search instead of retrying the first.

## 1.0.0-beta.8

_17 Oct 2026_

Estimates are now kept for up to 30 days and refreshed before upcoming runs, admins can pick a character per boss, the run sheet's Party tab shows a Party total, and an optional Guides screen publishes boss guides to Discord. Estimates and guides stay optional: without the scouter or the guides module everything else works as before.

> [!IMPORTANT]
> This release adds stored estimates to the character-data file and per-boss picks to the main store. Rolling back to an older image needs the pre-deploy backup restored and one command on the character-data file, both with the bot stopped (deploy README, "Stop and roll back"); otherwise the older image refuses to start.

### New features
- A linked character's roster class and level now take NEXON's values on every refresh, recorded in History as NEXON, and stay on the roster after an unlink or once the NEXON data expires, so other members see the current level.
- Added "Who carries", a bar per boss showing each member's share of the party's estimate, to the run sheet's Clear-time estimate panel and the member portal's run Party · characters.
- Redesigned day-of and countdown reminder cards, and the admin Reminders card preview, now carry a "Who carries" image of each boss's estimate split between the party members by IGN (or "no estimate yet"), drawn from the stored estimates and left off only while a party member with no estimate yet is still being looked up.
- Estimates are now kept for up to 30 days, across restarts, and refreshed automatically for party members of runs in the next 7 days, so reminder images and run sheets usually show one without anyone pressing Look up.
- Added "Refresh estimates every n days" to Config › Character data (7 unless changed); an older estimate still shows, marked "older than n days", on the run sheet, the roster, the member portal and the reminder image.
- Admins can now pick a character per boss for each party member on multi-boss weekly timings (Per boss in the Fixed editor's Characters tab) and runs (each boss's cell on the run sheet), with "Same for every boss" to undo it, and readiness, estimates, the "Who carries" image and the member portal's Party · characters follow each boss's own character.
- Added `kanade --version`, which prints the release number; the bot's startup log line now carries it too.
- Added the admin Guides screen (under More), where admins write boss guides as text, boss and spacer blocks, preview them as Discord shows them and publish them silently to a Discord channel after reviewing the plan, with later publishes editing the posted messages in place; it is off unless an operator sets `KANADE_MODULE_GUIDES=1` (`modules.guides`).

### Changes
- Estimates are now labelled “Estimate” on the portals and in Discord.
- An admin's Link… (and Add character for a MapleSEA character) now links it at once and fetches its NEXON data, without waiting for the member to confirm; link requests still waiting from before become active on the first start.
- The admin app, the member portal and Discord now say that unlinking keeps the class and level on the roster, and "Waiting for you" now only shows links waiting for the member to confirm the notice.
- The NEXON link notice and the Privacy page now say that a linked character's class and level are copied to the roster, stay after an unlink and are seen by guild members; members who linked a character themselves are asked to confirm the new notice before it refreshes again, while characters an admin linked stay linked.
- The Privacy page now says how long estimates are kept, how they are refreshed and when they are deleted.
- The run sheet's Party tab is now a member × boss grid: each boss is drawn once, a Party total row on top shows each boss's % and minutes, technically possible, when it was looked up and any assumed critical rate, each cell shows that member's character, level verdict and estimate for that boss, and the Clear-time estimate panel keeps only the lookup and Who carries.
- The member portal's Party · characters now opens with the same Party total as the run sheet and the reminder image, and neither portal nor run sheet shows a level or Combat Power that isn't known.

### Improvements
- Corrected boss guide details for eight bosses (gauge rules, forced merges, entry levels and MapleSEA mechanic names) after a source check.

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
- Fixed estimates asking you to set up critical rate on the scouter when it had already tried assuming 100% critical rate and couldn't finish; they now say the estimate couldn't be completed for that character and to try again later.

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

The Python bot, retired when v5 replaced it. Its code and changelog are at the [`v4.9.0`](https://github.com/hoshinoht/kanade-bot/tree/v4.9.0) tag and the `v4` branch; releases up to 1.0.0-beta.7 brought its data across with `kanade import v4`.
