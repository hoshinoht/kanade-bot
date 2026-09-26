# v5 Discord adapter (foundation)

Status: storage-independent groundwork under `src/bot/`. Not wired into
`serve`; nothing connects to Discord. Tests (`tests/discord/`, target
`discord`, feature `test-support`) are offline: fakes, Twilight models built
from JSON, and a loopback HTTP stub.

## Dependencies

Exact pins at Twilight 0.17.1 (the stack evaluation's selection), all with
`default-features = false`:

| Crate | Features | Why |
| --- | --- | --- |
| `twilight-model` | none | Gateway/HTTP payload types. |
| `twilight-http` | `rustls-platform-verifier` | REST client. No `decompression` (brotli), no `native-tls`/`hickory`. |
| `twilight-gateway` | `rustls-platform-verifier` | Shard and reconnects. No `zstd` (avoids the C `zstd-sys` build; one guild's traffic is small), no `twilight-http` (`create_recommended` is for multi-shard bots). |
| `hyper-util` (existing pin) | adds `client-legacy` | Names `hyper_util::client::legacy::Error` to recognise connect failures; Twilight already enabled the feature, so the build graph is unchanged. |

`cargo tree -e features -i rustls` (host and `aarch64-unknown-linux-gnu`)
shows rustls features `ring`, `std`, `tls12` only; the graph contains no
`aws-lc-*`, `native-tls`, `hyper-tls` or `openssl` crate (`openssl-probe`, a
pure-Rust CA path locator used by `rustls-native-certs`, is the only match).
Both clients take the process provider, so `runtime::tls` must install ring
before either is constructed. The platform verifier needs OS CA roots:
the final image keeps `ca-certificates`.

## Seams

- `transport::DiscordTransport`: create (content, embeds, required
  allowed-mentions, optional `reply_to`, `attachments` as `Upload`
  `{filename, bytes}` sent multipart and referenced from an embed as
  `attachment://<filename>`), edit (never sends attachments, so a message
  keeps the files it was posted with), delete, add/remove own
  unicode reaction, message presence, interaction reply, deferral and
  deferred completion, guild command registration, and the reads
  `list_members(guild, after, limit ≤ 1000)`, `channel_messages(channel,
  HistoryPage::{Latest, Before, After}, limit ≤ 100)` and
  `guild_channels(guild)`. A reply sends a message reference with
  `fail_if_not_exists = false` (a deleted target still posts); pinging the
  replied author stays governed by the explicit allow-list
  (`replied_user` false). Reads are classified like every call: 4xx
  definite, 5xx/unreadable body ambiguous, persistent 429 `RateLimited`,
  out-of-range limits `Invalid` and never sent. History pages are newest
  first, as Discord returns them (`After` holds the oldest messages after
  the cursor).
- Commands: only `register_guild_commands` (PUT
  `/applications/{app}/guilds/{guild}/commands`) exists. There is no
  global-command operation, and a test fails if any source under `src/`
  names one: v5 runs with the production token, which may sit in other
  guilds.
  `TwilightTransport` implements it; `FakeDiscord` (test support) records calls,
  scripts outcomes per operation (including ambiguous-but-applied) and mints
  sequential message ids.
- `events::CardIndex`: message id → run ids; both stores implement it (bind
  writes the card→run mapping).
- `events::ReactionSink`: implemented for `SchedulerService` by delegating to
  its existing `apply_reaction`.
- `events::EventHandler` and `gateway::EventSource`: the loop runs over a
  `Shard` in production and over a scripted source in tests.
- `commands::SlashCommand` and `Dispatcher`: one top-level command per
  handler, with one access gate for its whole tree.

## Outcomes and ambiguity

Every call returns `Delivered`, `DefinitelyRejected(kind)` or
`Ambiguous(kind)`. Only definite rejections prove nothing happened remotely.

| Result | Classification |
| --- | --- |
| 2xx with a readable created id | `Delivered` |
| 4xx except 429: codes 50013, 50001, 10003, 10008; 401; others by status | `DefinitelyRejected` |
| Connect failure (DNS, refused, TLS handshake): hyper-util `is_connect()` | `DefinitelyRejected(NotSent)` |
| Deadline reached before any send (e.g. waiting for a rate-limit permit), pre-flight cancel | `DefinitelyRejected(NotSent)` |
| Every send answered 429 until the send cap or deadline | `DefinitelyRejected(RateLimited)` |
| Client-side validation, known-invalid token | `DefinitelyRejected` (never sent) |
| 5xx, non-JSON error body, failure after the connection was obtained (including error-body read failures), attempt timeout, deadline after a send | `Ambiguous` |
| 2xx whose id cannot be read | `Ambiguous` (posted but unbindable) |

Connect failures are recognised by downcasting Twilight's `RequestError`
source to `hyper_util::client::legacy::Error`: in hyper-util 0.1.20 the
`Connect` kind is raised only while obtaining a connection, before
`try_send_request` hands the request over. Error-body read failures carry a
body error as source and stay ambiguous.

Caller rules: a delete answered `UnknownMessage` means the message is gone.
`message_presence` maps Unknown Message to `Delivered(Absent)`; whether an
Unknown Channel may count as absent (e.g. for digest replacement) is for the
journal slice to decide, so it stays a rejection here.

Retries (verified in twilight-http 0.17.1 and hyper-util 0.1.20 sources):
Twilight re-sends only after HTTP 429, which Discord returns for requests it
did not process, so that is not a replay. Each send waits for a rate-limiter
permit (the limiter stays on in production); a `set_pre_flight` check then
allows at most `MAX_SENDS` = 4 sends (one plus three re-sends) within the
deadline and cancels otherwise, so a scope-less 429 (Cloudflare or global)
cannot spin toward Discord's invalid-request ban. hyper-util re-sends only
requests never written to a reused connection. Nothing else retries.
Deadlines: 30 s per call, 10 s per attempt, 2.5 s for initial interaction
responses. Hitting a deadline after a send was allowed is
`Ambiguous(Timeout)`, even if that send was itself answered 429, because the
transport cannot see responses between sends. Loopback tests pin the 429
behaviour with the default rate limiter. The journal must record ambiguous
sends as indeterminate and never replay them (see the maintenance contract).

Debug output and outcomes never carry the token, interaction tokens,
request payloads or response bodies.

## Mentions

`mentions::allow_users` is the one builder: `parse` empty (so `@everyone`,
`@here`, role and unlisted user mentions in content notify nobody), `roles`
empty, `replied_user` false, users = the canonical (nonzero, unpadded decimal)
ids from the list, sorted and deduplicated; anything else is dropped. An empty
list serialises as `{"parse": []}`, v4's `AllowedMentions.none()` behaviour.
`for_intent` uses a `NotificationIntent`'s quiet-gated list. The HTTP client's
default allow-list is also none, and interaction replies always mention nobody.

## Gateway

Intents: `GUILDS` (availability, owner and role permissions for staff
checks, channel and thread events for the guild cache), `GUILD_MEMBERS`
(roster), `GUILD_MESSAGES` + `MESSAGE_CONTENT` (watched chat for extraction
and chat), `GUILD_MESSAGE_REACTIONS` (RSVPs). v4's other default intents (DMs,
typing, voice, presences, …) were unused and are dropped.

Developer Portal requirement (Bot → Privileged Gateway Intents): **Server
Members Intent** and **Message Content Intent** must be enabled, or the
gateway closes with 4014. Without Message Content, message events arrive
with empty content and extraction/chat see nothing.

Deserialized events (`WANTED_EVENTS`): ready, guild create/update, role
create/update/delete, member add/update/remove, reaction add/remove,
interaction create, message create/update/delete/delete-bulk, channel
create/update/delete, thread create/update/delete/list-sync.

One shard (`ShardId::ONE`). Twilight reconnects with exponential backoff and
resumes on its own (`RESUMED` is deserialized only so `ConnectionStatus`
reads `ready` again; a close frame or a failed reconnect reads
`disconnected`; serve marks a fatal close `closed`); the loop hands payload-free receive errors to a callback
and continues. When the stream ends after a fatal close it returns
`Closed { reason }`, taken from the close frame just before the end: 4004
(token), 4010/4011 (sharding), 4012 (API version), 4013 (invalid intents) and
4014 (privileged intents not enabled; the usual first-deploy error) have
named reasons with operator guidance, and other codes are kept. Events from other guilds and DMs are ignored. `run` takes any
shutdown future (e.g. the runtime's signal wait): it stops dispatching, sends
a normal close, drains until the close frame within a timeout, and reports how
many in-guild events arrived after shutdown and were not dispatched; startup
reconciliation must cover them.

Handler contract: `EventHandler::handle` runs inline in the loop, so it must
be short and must not await Discord transport calls. Interaction and
transport work is spawned (`commands::spawn_interaction`), so the shard keeps
being polled for heartbeats. A test shows a one-minute command does not hold
up the next event. `BotEvent`'s `Debug` redacts the interaction token.

## Event mapping

- Reactions: ✅/❌ unicode only; the bot itself, bot accounts (payload member
  on adds, roster flag on removals) and other emoji are ignored. Each card run
  is applied in its own transaction; runs deleted since posting are skipped.
- Members: add/update produce `RosterUpdate::Seen` (discord.py display name,
  nickname, bossing-role flag; ping level untouched; v5 adds the role ids and
  computed Administrator); bots produce nothing. Remove produces `Left`,
  clearing the role flag immediately (v4 waited for the next full sync) while
  keeping the row.
- Scope: the production token may be in other guilds. Every event whose
  guild is not the configured one returns `None` and increments
  `DroppedEvents` (`other_guild`); guild-less messages, reactions,
  interactions and channel events (DMs) return `None` and count as
  `no_guild`. Gateway control events are not counted. `Router::dropped()`
  hands out the shared counters for health.
- Messages: `MESSAGE_CREATE` → `BotEvent::MessageCreated`, `MESSAGE_UPDATE`
  (full message) → `MessageUpdated`, `MESSAGE_DELETE` and
  `MESSAGE_DELETE_BULK` → `MessagesDeleted { channel_id, origin_channel_id,
  thread_id, message_ids }`. `origin_channel_id` is v4 `origin_ids`: a
  thread's parent channel from the guild cache, else the message's own
  channel (also for an unknown channel). The router passes the bot's own and
  other bots' messages through; the consumer applies the loop guard.
  `Debug` omits message content.
- Extraction feed (`extract_feed/`): the serve handler forwards `Message*`
  to a `MessageFeed` (unbounded, never awaited inline) with the bot's id
  from `READY`; `Feed::run` converts each to an `IncomingMessage` (filed
  under `origin_channel_id`; `AuthorKind` `Myself` / `Webhook` / `Bot` /
  `Member`) and forwards it to the pipeline in gateway order. A message or
  edit whose latest timestamp is more than `STALE_AFTER` (60 s) old is
  `Replay` and only cached (`StaleCache`), never offered. `DiscordHistory`
  is the rescan `History`: the channel, then each cached thread under it,
  `channel_messages` `After` pages of 100 from the window's snowflake (at
  most 200 pages each; an unreadable thread is skipped).
- Channels and threads (`CHANNEL_*`, `THREAD_*`, `THREAD_LIST_SYNC`) only
  update the guild cache and return `None`.
- Guild access: `events::Router` is stateful. It keeps the owner and every
  role's permissions (`GuildRoles`) from `GUILD_CREATE`, `GUILD_UPDATE` and
  `GUILD_ROLE_CREATE/UPDATE/DELETE` (event-type filter only; the `GUILDS`
  intent already delivers them). A member is Administrator when `@everyone`
  (role id = guild id) or one of their roles has the `ADMINISTRATOR` bit; a
  role id the cache has not seen never grants it, and nothing does before
  `GUILD_CREATE`. `BotEvent::GuildAvailable { owner_id, admin_roles }` is
  emitted on `GUILD_CREATE`, whenever the owner or the set of
  Administrator roles changes, and on every role deletion (Discord sends no
  member updates then; serve prunes stored role lists against the guild's
  roles, `GuildCache::role_ids`). `BotEvent::Ready` carries the bot's user
  id and the application id.
- `api::auth::roster` applies both: `on_roster_update` stores `Seen`'s roles
  and Administrator, then re-checks staff; `on_guild_available` records the
  owner, clears the stored Administrator of every row whose roles no longer
  grant it and re-checks those members and a replaced owner, ending sessions
  that fail. It only revokes (a row may belong to someone who left while
  the bot was offline); a role newly granting Administrator takes effect for
  a member at their next member event. The staff gate's re-check reads the
  stored flag, so a lost Administrator is never kept while the bot is online; changes made while it was offline wait for startup roster reconciliation.

## Guild cache

`bot::guild_cache::GuildCache` (shared `Arc`, fed by `Router::with_cache`)
holds the configured guild's channels and threads (name, kind, parent,
position, permission overwrites), the role permissions, the owner and the
bot's own roles, from `GUILD_CREATE` (channels, active threads, the bot's
member), `GUILD_UPDATE`, role events, the bot's member add/update, and
channel/thread events. A deleted channel takes its threads;
`THREAD_LIST_SYNC` replaces the synced parents' threads (all parents when
unscoped). An unavailable guild keeps its last view. It implements:

- `api::state::ChannelList`: text and announcement channels by position,
  named `#name`, `watched` from the `WatchList` (`set_watch`: channel ids
  and category ids; v4 `is_watched`, threads count as their parent).
- `chat::gate::ChannelDirectory`: a channel's category, or a thread's
  parent.
- `domain::notify::ChannelDirectory` (`is_reachable`, v4 `can_send_in`): a
  known text-capable channel where the bot's computed permissions include
  View Channel + Send Messages (Send Messages in Threads for threads, from
  the parent's overwrites). Permissions follow Discord's hierarchy (owner,
  Administrator, `@everyone`/role/member overwrites); timeouts and
  private-thread membership are not modelled. Before the bot's own member is
  seen, permissions are unknown and a known channel counts as reachable (v4
  "go ahead and try"); unknown channels are unreachable.
- `channel_name(id)` for prompts and logs (raw name, no `#`).

## Commands

Gates follow v4: bossing role (no staff bypass), staff (Administrator,
guild owner, admin role), debug (staff or `DEBUG_USER_IDS`). Refusals,
user errors and failures reply ephemerally with v4's text; internal
failures use the generic text and return the detail for logging. The proof
command is `/debug status`, reduced to uptime and storage state. It needs no
storage, it is admin-hidden (`default_member_permissions = ADMINISTRATOR`,
guild context), and it exercises the debug gate.

Initial responses have 2.5 s. A command whose `defer()` returns
`Some(ephemeral)` is first acknowledged with a deferred response (type 5, with
visibility fixed then), then runs, then fills in the original response. It
runs only if the acknowledgement was delivered, so an ambiguous or rejected
deferral never executes the command (`Disposition::NotAcknowledged`).
Refusals are always answered immediately.

## Delivery (tick and executor)

`src/bot/delivery/` turns planned notifications into posts, each claimed in the
delivery journal before transport. Each journal write is its own transaction,
and none is open during a Discord call. Tests: `tests/delivery/` (target
`delivery`), on the memory store and a temp SQLite store.

| Transport outcome | Journal |
| --- | --- |
| `Delivered` | `bind` with the channel actually used (including fallback); digests pass `record_week` |
| `Ambiguous`, or `bind` failed | `mark_indeterminate`: held, never resent |
| `NotSent`, `RateLimited` | `release_unsent`: claimable again next tick |
| other definite rejection | `retire_rejected` (never retried) + `AdminAlert::SendRejected` |

A planned `Suppressed` send, or a claim returning `Held`, posts nothing. A
target that vanished before the claim is skipped. Bound reminder cards get ✅/❌
reactions as a best effort, as in v4. A notice sent inside its own operation
passes that operation's effect ordinal; tick sends pass none. Outbox notices
are claimed by `(source, ordinal)` (`Executor::execute_source`).

Tick (v4 order, one clock reading, one `scheduler_tick` lease, ended even on
error): materialise the current and next two boss weeks when the week differs
from the one this process last materialised (v4 kept `last_materialised_week`
in config; re-materialising is idempotent), then `mark_done`, then the
notice outbox drain (`delivery/notices.rs`, `maintenance-contract.md`
*Notice outbox*), then the digest, then dispatch. Dispatch retires hopeless rows, then claims at most
`max_sends_per_tick` (default 20) sends. Suppressed, held and vanished sends
don't count toward the cap; sends past it wait for the next tick
(`deferred`). Call
`Delivery::start` (`recover_on_start`) once after taking ownership of the
store: in-flight attempts become indeterminate and are never resent.

Digest:
- Earlier weeks' active cards are retired every time the digest step runs.
- The first tick, or a missing post channel, only records the week.
- **v5 deviation (user decision):** a post happens only when the current boss
  week is later than the recorded one. A clock behind the recorded week raises
  `DigestClockRollback` and posts nothing; v4 compared for equality and would
  re-post. No frozen vector moves the clock backwards, so no vector changes.
- Replacing a week's active card needs confirmed deletion: delete `Delivered`,
  Unknown Message, or an ambiguous delete followed by a fetch showing
  `Absent`. Only then does `retire_for_replacement` run. Anything else
  suppresses the new post and raises `DigestReplacementSuppressed`. A
  `Suppressed` send never deletes.

Error isolation (v4 `send_card` parity):
- A journal error on one send raises `JournalFailure`, is reported as
  `SendOutcome::Failed`, and the tick moves on to the next send.
- Only lease loss (`LeaseNotLive`) or a backend failure aborts the tick.
- A failure after a fresh claim names the attempt. That intent stays held until
  restart recovery makes it indeterminate; it is never resent.

Admin alerts (`AlertSink`; `AlertRecorder` keeps them in memory) are
throttled per process: an alert with the same kind and target
(`AdminAlert::key`) passes at most once per hour (`ALERT_WINDOW`) of tick time.
A clock that moved backwards never suppresses one. The alerts are:
- `SendRejected`, carrying the channel and reason.
- `HomeChannelUnavailable`, raised for a fallback send when it is claimed.
- `DigestReplacementSuppressed`.
- `DigestClockRollback`.
- `JournalFailure`.
- `BacklogDropped` (extraction backlog overflow).
- `CardAnswerFailed` (a ✅/❌ failure members must not see).

Quiet mode is never announced publicly: cards carry no mention tags (a
member without a known name reads `(unnamed)`) and the allow-list is empty.

## Reminder and digest cards

`src/bot/delivery/cards/` ports v4 `bot.agent.formatting` (`day_of_card`,
`countdown_card`, `digest_card` and their helpers) byte for byte where v4
and v5 agree; `render.rs` turns a card into the post. Tests:
`tests/delivery/cards.rs` (both stores, FakeDiscord) and
`tests/delivery/attendance.rs`.

- **Day-of** (`day_of.rs`): content `📅 **<heading>**` plus everyone on the
  runs; one field per run, `🕘 HH:MM  ·  <bosses>` (`🕒 own time` for own-time
  runs), valued boss detail (`**XKalos** · Gatekeeper Kalos (Extreme,
  Lv265)`), status line (`⚠️ unconfirmed · 1/5 ✅`) and the party; footer
  `React ✅ if you're on, ❌ if not.`; the lead boss's colour (else blurple),
  portrait thumbnail and entry artwork as the image.
- **Countdown** (`countdown.rs`): `⏰ **<bosses>** in <1h|15m|1h30m> (HH:MM) —
  <waiting>`, waiting being `everyone's confirmed ✅` or the party still on
  plus `<declined> out` (decliners are named, never pinged); description
  boss detail, status line and `Still to answer: …` while answers are
  pending; the react hint only while pending; yellow while pending or
  someone is out, green when all set (a catalog colour wins, as v4); the
  portrait thumbnail, no image.
- **Digest** (`digest.rs`): `🗓️ Boss week of <Ddd DD Mon>`, the summary
  (`**c/n Cleared** · n run(s) across d day(s)` + unconfirmed/at-risk
  counts), one field per local day of `digest_line`s, v4's footer, no art,
  no mentions.
- Bosses are the stored tokens joined with ` + ` (v4 `format_bosses`); the
  catalog supplies the detail line, colour and art. People follow v4's
  `Audience`: allow-listed members are `<@id>`, others their name.
- v5 attendance mode keeps the v5 tally in status and digest lines
  (`4/4 (2 assumed), expected`); v4-compat renders v4's `1/5 ✅ · 1 ❌`.
- **Art** (`art.rs`, `infrastructure::files::BossArt` over
  `KANADE_BOSS_DIR`): `portraits/<portrait or key>.<png|webp|jpg|jpeg>` and
  `artwork/entry/<key>.<ext>`, matched exactly (case included) against the
  directory listing, at most 1.5 MiB each (the shipped art peaks at
  ~1.2 MB entry / ~0.2 MB portrait, so a post stays well inside the 10 s
  upload attempt: a timed-out upload is ambiguous and never resent).
  Uploaded as `<file>` and `image-<file>` (v4 `IMAGE_PREFIX`). Missing,
  oversized (logged `card_art_skipped`) or unreadable art drops the
  picture, never the send. Each picture is resolved and read in one lookup
  on the blocking pool (`fetch_art`); suppressed sends are not rendered.
- **Records** (migration 0014 `reminder_cards`, `ReminderCardStore`, both
  stores + journal conformance): before a reminder card is claimed, its kind
  (`day_of` / `countdown_<M>`) and day-of heading are stored under the
  send's native dedupe key, first write wins. A retry after `NotSent`, a
  restart and every edit reuse them. Dedupe and request fingerprints are
  unchanged: both hash the intent, never the rendered payload, so embeds and
  attachments cannot affect them.
- **Edits** (`refresh.rs`, v4 `card_needs_refresh` / `refresh_run_cards` /
  `refresh_weekly_digest`): the store's run-write observer
  (`observe_run_writes`, called after every committed `commit` /
  `commit_merge` with the runs whose row or RSVPs it wrote) queues run ids
  in a `RefreshQueue` (coalesced per run, at most 1024 pending, excess
  dropped and logged). One worker (`CardRefresh::run`) drains it in batches,
  off the reaction worker and the tick, and stops at shutdown mid-batch.
  So reactions, `/rsvp` and other commands, the portal/admin API, chat,
  applied proposals, extraction and the tick's own status changes all
  refresh. Per batch: every bound reminder card with a record naming a run
  that has not started is re-rendered from current answers and edited (same
  heading, art referenced by the posted names, nothing uploaded, allow-list
  empty; mentions in the text planned as dispatch plans them; quiet mode
  read live from the tick), then the active digest of each touched week
  (also for runs already done). Cards posted before records existed are
  left as they are. Not covered: weekly-timing-only writes (standing
  answers, attendance default) that change v5 derived states without a run
  write.

**Named difference from v4 — day-of heading (user decision 2026-09-26).**
The heading is rewritten in the guild default persona's voice (bundle, no
member profile) by the small `rewrite` model role through the nudge
rewriter (`heading.rs`): seed `Today — {day}` (v4's text), mood playful,
`{day}` left to the model and filled afterwards; code keeps the `📅 `
prefix, the bold and the mentions line. The prompt holds the code-owned
instruction, the persona and the seed only: no member, boss or schedule
data. It is tried once per card before the claim, bounded by the 2 s
`REWRITE_DEADLINE` (`try_acquire`, no queueing, no retries) and checked by
`accept_rewrite`; failure, timeout, rejection, no `rewrite` role or no
persona → exactly v4's `Today — <Ddd DD Mon>`. The chosen line is stored in
the card record; `day_of_heading` logs `source` (`rewrite`/`seed`) and a
reason (`no_rewriter`, `no_persona`, `timeout`, `unavailable`, `refused`,
`misconfigured`, `rejected`, `accepted`), never the text. Serve wires it in `runtime::serve::tick::card_kit`
(identity codec: `Passthrough`; the prompt carries no member data).

## Proposal cards

`src/bot/cards/` (slice E5; v4 `formatting.proposal_card`,
`Pipeline.apply_plan`, `_handle_proposal_reaction`). Tests:
`tests/extract/cards.rs` (vectors) and `tests/discord/cards.rs`.

- **Text** (`format.rs`): `when_text`, `proposal_line`, `card_kind`,
  `proposal_card`, `unanswered` and the notices, byte-exact to
  `docs/v5/vectors/extract/cards.json` (4 cases, 35 steps, no deviations).
  Portraits and artwork are not rendered. v5 adds one line: a card entry's
  self-service link (`<lead-in> → edit the run: <url>`) under its field.
- **Stored details.** `CardDesk::post_card` saves each proposal's
  `CardDetails` (the v4 row the card reads) with its channel through
  `ProposalCardStore` (migration 0011 `proposal_cards`), then posts. A card is
  always rendered from stored details plus the runs as they stand, so it can
  be refreshed after a restart (`CardDesk::refresh`).
- **Posting** goes through the delivery `Executor` under its own lease:
  intent `EffectKind::Card`, one `DeliveryTarget::Card(proposal id)` per
  proposal (binding type `card`). The claim requires a stored, unposted card
  of a live proposal with no unproven retirement; `bind` writes
  `message_id`/`posted_at` in the same transaction. Ambiguous sends stay held
  and are never replayed; a refused or unsent card stays unposted and is
  posted, as v4's stranded rows, before the next card in its channel: each
  stranded card as its own message, skipping cards still held by an
  unresolved attempt and proposals past their TTL, so one held or
  unavailable card never blocks another. Bound cards get ✅/❌. Allowed
  mentions are empty: people are named, not pinged. `PostResult` tells the
  pipeline what happened: `Posted` (bound, held, or a journal write failed
  after Discord took or may have taken it), `Pending` (details saved, a later
  pass reposts it with its link; tips stay spent), `NotPosted` (details not
  saved; tips are given back).
- **Refresh.** Re-render content and embed and append one line per decision
  on the card, from the proposals' states: merged → `✅ applied by <name>`,
  rejected → `❌ rejected by <name>`, superseded → `↪ superseded by a newer
  card`. The edit mentions nobody. A card's retired siblings are refreshed
  after every pass and every approval. A ✅ refused because the run changed
  after the card went up adds `⚠️ out of date` at that refresh (not
  persisted: a later refresh shows only the decisions).
- **Reactions** (`CardDesk::on_reaction`): only added ✅/❌ on a message
  with stored cards (else `NotACard`, routed as an RSVP). `Authority` gives
  the member's `Approver` (role, Administrator or guild owner). ❌ rejects
  and ✅ approves every proposal on the card through
  `SchedulerService::{reject_proposal, approve_proposal}`, so the approval
  rules, the 24 h TTL, move revival, repeat-✅ follow-ups and v4's ✅-time
  refusal wording are the scheduler's. The scheduler checks the member's
  authority before anything else, so a stranger's reaction never expires,
  closes or posts anything. Unauthorised or already answered proposals are
  silent. Only member-readable refusals are posted, once, as
  `⚠️ <reason>; …` (journalled notice, no mentions): v4's refusal texts,
  "that is already the case", "that proposal has expired", and (user
  decision 2026-09-25) for a draft conflict "That run was changed after this
  card went up, so I didn't apply it. Check the run and ask again if it
  still needs changing." Any other failure (store, retries, id reuse) raises
  `AdminAlert::CardAnswerFailed` and posts nothing; on ❌, member-readable
  refusals (e.g. an expired card) stay silent. An approved move's notice
  (v4 `amend_notice`, the only kind a ✅ announces) is written to the
  notice outbox with the merge; `CardReaction::notices` only reports it.
- **Outbox** (`CardOutbox`): cards as above; link-first links as a
  journalled notice `<@author> <lead-in> → edit the run: <url>` (named, not
  pinged); chat answers through the reaction path (`apply_reaction` as the
  member, then the answer's source set to `chat`, v4 `_apply_rsvp`: two
  history records, both attributed to the member on the Discord surface);
  backlog drops as `AdminAlert::BacklogDropped`.

## Serve wiring

`runtime::serve::discord` composes the adapter (details and shutdown order:
`runtime-bootstrap.md`, "Live serve"): `gateway::run_live` with a
`Router::with_cache` over the one `GuildCache` shared with the API and the
tick, and a `ConnectionStatus` for health; `handler::Fanout` (the
`EventHandler`) sends roster jobs to `roster::RosterTask`, RSVP reactions to
`handler::Reactions` (card ✅/❌ → `CardDesk::on_reaction`, else the
`ReactionRouter`), spawns interaction and command-registration tasks, offers created messages
to the chat pilot (`chat_feed::ChatFeed`: resolved roles, mentions and the
replied-to message, nothing fetched; deletions cancel) and counts message
events. `roster::reconcile` pages `list_members` and diffs it
against the stored rows (`Seen` for changed members, `Left` for rows still
holding a role, roles or Administrator); `roster::LiveRoster` is the
in-memory member snapshot (`Directory`) the tick and cards read.
`delivery::LogAlerts` is the beta alert destination (structured log).
Commands: `runtime::serve::commands` builds a `CommandContext` from the
API's own store, writer, policy, catalog, personas, access and clock (the
gateway cache as `GuildChannels`, the bot's name from `READY`; rescans and
test cards `None`, `/limits` reads the chat pilot's allowance) and `register_retained`; the dispatcher is
built on the first `READY` and every guild-registered command and
autocomplete goes through `commands::spawn_interaction`.

## Deferred

Opposite-reaction removal and decline notices (chat answers apply without
them); proposal-card portraits/artwork; withdrawing a card whose message was deleted;
converting `BotEvent::Message*` into anything beyond chat and extraction;
archived threads in rescans; attachments; an admin-alert
destination beyond the log; an
authenticated gateway/TLS smoke test against Discord (L1, run by hand).
