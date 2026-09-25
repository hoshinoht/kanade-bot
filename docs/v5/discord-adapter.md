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
  allowed-mentions), edit, delete, add/remove own unicode reaction, message
  presence, interaction reply, deferral and deferred completion, guild
  command registration.
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

Intents: `GUILDS` (availability, owner and role permissions for staff checks), `GUILD_MEMBERS`
(roster), `GUILD_MESSAGES` + `MESSAGE_CONTENT` (watched chat for extraction
and chat), `GUILD_MESSAGE_REACTIONS` (RSVPs). v4's other default intents (DMs,
typing, voice, presences, …) were unused and are dropped.

One shard (`ShardId::ONE`). Twilight reconnects with exponential backoff and
resumes on its own; the loop hands payload-free receive errors to a callback
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
- Guild access: `events::Router` is stateful. It keeps the owner and every
  role's permissions (`GuildRoles`) from `GUILD_CREATE`, `GUILD_UPDATE` and
  `GUILD_ROLE_CREATE/UPDATE/DELETE` (event-type filter only; the `GUILDS`
  intent already delivers them). A member is Administrator when `@everyone`
  (role id = guild id) or one of their roles has the `ADMINISTRATOR` bit; a
  role id the cache has not seen never grants it, and nothing does before
  `GUILD_CREATE`. `BotEvent::GuildAvailable { owner_id, admin_roles }` is
  emitted on `GUILD_CREATE` and whenever the owner or the set of
  Administrator roles changes.
- `api::auth::roster` applies both: `on_roster_update` stores `Seen`'s roles
  and Administrator, then re-checks staff; `on_guild_available` records the
  owner, clears the stored Administrator of every row whose roles no longer
  grant it and re-checks those members and a replaced owner, ending sessions
  that fail. It only revokes (a row may belong to someone who left while
  the bot was offline); a role newly granting Administrator takes effect for
  a member at their next member event. The staff gate's re-check reads the
  stored flag, so a lost Administrator is never kept.

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
reactions as a best effort, as in v4. A notice passes its operation's effect
ordinal; tick sends pass none.

Tick (v4 order, one clock reading, one `scheduler_tick` lease, ended even on
error): materialise the current and next two boss weeks when the week differs
from the one this process last materialised (v4 kept `last_materialised_week`
in config; re-materialising is idempotent), then `mark_done`, then the digest,
then dispatch. Dispatch retires hopeless rows, then claims at most
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

Quiet mode is never announced publicly: the rendered text has no mention tags
and the allow-list is empty.

Rendering is minimal plain text (bosses, Discord timestamps, mention tags).
Card parity (embeds, portraits, quiet lines) is a later slice.

## Deferred

Serve-mode wiring and logging; startup roster sync and member chunking;
reaction and roster reconciliation; roster persistence; wiring the tick into
serve mode with a live `ChannelDirectory` and the admin-alert destination;
notice delivery from mutation operations; card refresh/edits; proposal-card
reactions, opposite-reaction removal and decline notices; message events for chat and extraction;
attachments; the other command definitions; an authenticated gateway/TLS smoke test against Discord.
