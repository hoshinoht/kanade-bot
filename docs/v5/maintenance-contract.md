# Durable maintenance and delivery contract

Status: parent-selected design; state-and-leases foundation accepted after
correction and independent review. Delivery journaling, ingress, reconciliation
and transfer gates remain incomplete; no operational readiness or production
upgrade is claimed. Sources are the relocated v4
reference; line numbers below describe that reference, not future edited files.

## Release and bootstrap boundary

No production v14 upgrade before maintenance, bidirectional transfer, the updated
rollback runtime and both prewrite/postwrite zero-loss rehearsals are accepted.
An old-image snapshot restore is only a pre-accepted-write escape hatch. After
accepted writes, preserve them through the updated runtime and bundle path.

Probe schema version read-only. For persistent v9-v13, take a unique private
non-pruning online backup before any upgrade DDL, then upgrade transactionally.
Existing v14 reads maintenance state before schema work: no unconditional schema
script or write-affecting PRAGMA startup. Install guards and normalize orphans or
abandoned intents to BLOCKED before Repo returns. Config seeding/workers are
OPEN-only; closed read/control
startup remains possible. No development check may open private/live stores.

One writable Repo owns each persistent SQLite inode for its entire lifetime.
Retain an unlocked database identity FD and a nonblocking exclusive `flock` on a
separate device/inode-keyed lockfile in one operator-configured private local
`DB_OWNER_LOCK_DIR`, plus a process-local device/inode registry. Never lock the
database inode: Darwin whole-file flock conflicts with SQLite, while traditional
POSIX byte locks disappear when any same-process descriptor for that inode closes
(parent reproduced both on SQLite3.53.1/Darwin; oracle review confirms semantics).
The user approved a shared service account and common lock-directory contract;
actual UID/mount consistency remains a deployment verification, not admin-token
authentication. Require owner0700 directory and no-follow owner0600 regular
single-link lockfiles, never truncate/replace/unlink them on release. No global-temp,
per-process or DB-path-derived fallback lock root. All writer entrypoints use the
same explicit setting; missing persistent-store configuration fails before mutation.

Acquire ownership before probe/recovery. Aliases/hardlinks must contend; verify
the SQLite pathname still names the retained identity inode before writes. Exact
`:memory:` stores remain independent. Cover all constructor stages with cleanup;
close SQLite before releasing ownership. Inherited fork objects are unusable;
child cleanup closes inherited ownership FDs without LOCK_UN and cannot prolong
or release the parent's lock. Read-only health/snapshot inspection does not
construct Repo and uses the designated DB pathname (WAL hardlink aliases are not
safe reader paths). This is a same-UID/host/filesystem-namespace cooperating-app
guarantee, not exclusion of arbitrary SQLite clients or hostile same-UID actors.

The user explicitly requires live history export/recording with database updates
to remain available (currently one admin). Standalone live recorders must route
upserts through the running owner's authenticated, bounded, lease-gated API;
they must not open another writable Repo or require stopping the bot. Standalone
offline recording may own the store. The later gate-and-reconcile package owns
this ingress integration and its retry/idempotency tests; it is a release gate,
not an optional future expansion. Single-admin usage does not eliminate bot/worker
concurrency. No fallback to direct SQLite when the live owner/API is unavailable.

Install the deny-by-default bootstrap authorizer immediately after connecting,
before migration/recovery writes. Bootstrap authority is private, fixed-purpose,
constructing PID/thread-bound and permanently revoked before Repo returns.
Runtime admin authority is fixed-purpose/action/table and actual-task-bound;
no caller-selectable table set or public generic bypass exists.

Upgrade snapshots use an owner-checked mode0700 no-follow directory, retained
exclusive mode0600 temporary file descriptor, inode verification before/after
SQLite's necessary pathname open/backup, online backup and no-replace anchored
publication. Preserve older snapshots and valid pre-upgrade images on DDL failure;
never unlink an unknown replacement. Reject unsafe parent/directory substitution.
Document the same-UID/root threat boundary rather than claiming an FD-only SQLite API.

## Admission and state

| Mode | New ordinary leases | Already-live scoped leases |
| --- | --- | --- |
| OPEN | admit atomically | valid |
| PREPARING | deny | valid through completion/finalization |
| BLOCKED | deny | valid through completion/finalization |
| FROZEN | deny | must be zero |
| RESUMING | deny until checked OPEN transition | administrative work only |

Prepare timeout/cancellation durably enters BLOCKED without cancelling accepted work or
revoking its finalization rights. Restarted rows are unauthorized orphans, not
capabilities; retirement requires actor/reason. No time expiry proves completion.
No await occurs inside a SQLite transaction. The authorizer has statement caching
disabled and gates application DML/DDL, ATTACH and write PRAGMAs. Internal SQLite
WAL/locking activity is not represented as application writes.
Maintenance/journal metadata is protected by narrow internal authority in every
mode, including OPEN; ordinary direct SQL cannot change admission behind its cache.
Draining leases is not an authoritative freeze: the foundation may report
quiescence while remaining PREPARING, but must not enter FROZEN or fabricate
checkpoint hashes. The later reconciliation gate alone completes the validated
freeze transition after adoption, uncertainty, revision/week and roster/reaction
checks. Pending adoption or orphan blockers remain BLOCKED, not a successful drain.

Task-local context alone is insufficient: capability, process instance and actual
task owner must match. Nested calls reuse the live lease; child work requires a
synchronous reservation before spawning and explicit ownership transfer. A copied
context or closed parent token confers no authority. Reservation is a distinct,
non-authoritative state that counts toward draining and transfers exactly once
to one child, including after PREPARING/BLOCKED. Active parent leases cannot be
claimed. Live authority requires persisted admission plus a current in-memory
operation/token/task registration, not token possession alone. Administrative authorities
are internal, task-bound, purpose/table scoped, never a public bypass switch.

## Table contract before v14 DDL

`maintenance_state` is a singleton containing mode, generation, state_revision,
pinned instant/week/reset fingerprint, reconciliation hashes/time and bounded
blocker code. Adoption fields: state `pending|complete`, adoption_id, started_at,
completed_at, attested_by, attested_at and attestation_reason.

Fresh empty creation starts OPEN/adoption-complete. Persistent v9-v13 upgrades start
BLOCKED/adoption-pending atomically. Pending survives restart and prohibits
OPEN, export and import. Seeding is idempotent by adoption_id. Completion requires
resolved typed uncertainty and the global operator attestation described below.

`maintenance_leases`: operation_id primary key; instance_id, owner_token_hash,
generation, operation_kind, started_at. Unique (instance_id, owner_token_hash),
index (generation, started_at). The token itself never enters portable data.
Retain lifecycle `live|orphaned|retired` with orphan/retirement time, retirement
actor and bounded nonempty reason. Normal completed leases may be removed;
orphan retirement must atomically preserve operation identity and actor/reason
instead of deleting the evidence. Startup classifies rows only after ownership
is exclusively acquired. A live child reservation has distinct claim state.

`delivery_attempts` is one actual transport send per row:

- attempt_id primary key, operation_id, effect_ordinal, owner_instance_id;
- origin `runtime|adoption`, effect_kind, dedupe_scope `native|source|operation`,
  SHA-256 dedupe_key and dedupe_active;
- state `intent|indeterminate|bound|retired`;
- destination_kind `channel|dm`, guild_id, channel_id, recipient_id, message_id;
- fingerprint_version, request_fingerprint, observable_fingerprint, intended_at;
- resolved_at, resolved_by, resolution_reason.

Unique (operation_id, effect_ordinal); partial unique dedupe_key while active;
indexes on (state, intended_at), message_id and operation_id. Every non-retired
state, including bound, remains dedupe-active. Only retired rows have
dedupe_active=0; DDL enforces both directions. Required SHA-256 fields are exactly
64 lowercase hexadecimal characters. Authorized retirement
disables its key and releases target claims atomically. Retirement requires time,
actor and a bounded nonempty
reason. Bound requires actual channel/message identity; excluded local memory DM
records may use recipient/message identity and native enrollment binding.
Do not foreign-key attempt history to an ephemeral lease row that is deleted on
completion. Runtime attempt IDs are real persisted internal IDs, not invented
portable binding identities.

`delivery_attempt_targets`: attempt_id FK, target_ordinal, binding_type,
key_primary, optional key_secondary, released_at, release_actor, release_reason.
Primary key (attempt_id, target_ordinal). Partial unique
(binding_type, key_primary, COALESCE(key_secondary, '')) while unreleased.
Replacement/release is explicit and reasoned, never implicit retry clearance.

| Target | Primary key | Secondary key |
| --- | --- | --- |
| reminder | reminders.id | none |
| digest | canonical weekly_digests.week_start | none |
| decline | decline_notices.run_id | user_id |
| card | amendments.id | none |
| debug_card | debug_messages.message_id | none |
| memory_notice | enrollment guild_id | user_id |
| memory_proposal | chat_memories.id | none |

Zero/one/many targets are valid. Debug intent has zero targets until its message
ID exists, then finalization inserts its native target. Generic sends have none.
Native dedupe keys hash sorted typed target identities; per-target uniqueness
prevents regrouping one claimed target into a second send. Chat source keys use
guild/channel/source message and semantic slot. Mutation announcements use entity,
action and result fingerprint where available. Deliberate debug, guide and /say
requests use operation ID/effect ordinal: a new request may repeat intentionally.
An internal retry never receives a fresh operation/key; notifying retries are
removed rather than falsely promising exactly-once transport.

## Fingerprints and finalization

Canonicalize in memory; persist only versioned SHA-256 values, no plaintext
content/embed/file names/bytes. Hashes are not encryption or proof of authorship.
Request fingerprint includes exact content, ordered embeds/fields, ordered files
(hashed filename, size, bytes hash), sorted allowed user/role IDs and everyone/
replied_user flags, reference and destination. Observable fingerprint uses only
Discord-recoverable content/embeds, actual mentions, reference and attachments.
Bounded attachment retrieval may be needed for history reconciliation; unavailable
data remains unresolved. Zero or multiple matching messages are ambiguous.

Transitions: intent -> bound/indeterminate/retired; indeterminate -> bound/retired;
bound -> retired for authoritative deletion/replacement/out-of-week retirement.
No return to intent; retired rows are immutable. Explicit definitive rejection or
operator retirement may end an intent, not arbitrary timeout classification.

Success transaction verifies live authority (including timeout-induced BLOCKED),
inserts response-created targets, writes every native binding and marks the attempt
bound with actual identity. Grouped reminders and amendments finalize together.
Any failure rolls back then records indeterminate separately; crash leaves intent,
normalized to indeterminate at restart. No lease is silently considered completed
while an unfinalized transport result still needs durable recording.

## Unpublished portable v1 correction

Parent selects explicit grouped targets rather than projecting one send into
unrelated pseudo-attempts. This requires a separately tested correction to the
accepted-but-unpublished schema before maintenance DDL. Format remains v1; there
are no released consumers to silently break.

- Bound attempt: state=bound, delivery_kind, channel_id, message_id and nonempty
  targets[] of typed native reminder/digest/decline/card/debug references.
- Retired attempt: state=retired, delivery_kind, optional channel/message,
  targets[] (possibly empty), retired_by and retired_reason.
- Delivery kinds are closed: reminder, digest, decline, card, debug_card; targets
  must match that producer family. Reminder/card posts may group several targets;
  digest/decline/debug bound posts each have one native target. Reminder posts and
  proposal-card posts are separate actual messages, never a mixed-family group.
  pre_journal_attestation is a retired-only kind with no targets.
- One bound attempt has one authoritative (channel_id,message_id); duplicate
  message groups and duplicate native targets within/across bound groups fail.
- Discord IDs use canonical nonzero decimal strings: padded, signed or zero
  aliases cannot evade identity uniqueness. Every bound proposal card must equal
  its amendment's non-null authoritative proposal message reference.
- Every target resolves to its current binding and matches the group's message;
  channels match wherever bindings carry them. Every delivered binding belongs
  to exactly one bound group; genuinely unsent reminder rows need no group.
- Retired historical references may outlive native rows; they do not claim live
  bindings. A target may appear in historical retirement and a later bound group.
  Validate their closed identity shape, not a false live FK or live uniqueness.
- Omit terminal generic/memory attempts from portable delivery; unresolved ones
  still block export. Retained history is not omitted. A targetless global
  pre-journal retirement attestation may be carried. No private memory content
  appears in portable targets, fingerprints or retirement descriptions.

## Exact first-adoption classifications

Candidate bindings require authoritative fetch before final binding. Store an
idempotent adoption record, never infer remote absence from missing local data.

| Family/source | Candidate / no replay | Uncertainty |
| --- | --- | --- |
| reminders, db185-195; client1015-1039 | sent_at+message_id -> fetch bounded candidate channels; both null and future fire_at -> genuinely future unsent | due both-null; sent without message (also stale-suppression shape); message without sent |
| amendments, db128-157; pipeline1202-1220,1348-1394 | message -> group by channel/message and fetch; terminal null -> retire terminal-unbound-no-replay | proposed null binding |
| weekly_digests, db198-207; client1532-1659 | active row -> fetch exact channel/message | last_digest_week without matching row, absent/mismatched marker, retired/mismatched row, delete-old/send-new gap |
| declines, db240-248; client1749-1800 | channel+message -> fetch | message without channel; null message with notified_at, including deliberate retraction shape; age/cooldown never proves safety |
| memory enrollment, db328-341,1682-1789; client1052-1112 | pending_notice with null attempt time -> not started; active with message+sent time -> locally bound/excluded; disabled/opted-out -> no replay | attempted pending with no delivery; active missing sent/message; inconsistent pending sent fields |
| memory proposal, db346-381,1919-2014; chat1082-1123 | proposed message -> local fetch/reconcile; terminal -> no replay, excluded | proposed null message |

Terminal bindings are fetched/carried only inside the agreed active horizon;
retained schedule/history is never pruned by that rule. Out-of-week binding
retirement is explicit. Uncertain old sends do not expire automatically.

Unrowed replies, /say, guides, status/fixed announcements and some DMs cannot be
proven from audit/chat rows. Adoption remains pending until an operator attests,
after drain and authoritative reconciliation, that unknowable pre-journal effects
are retired and will not be retried. This is not a claim that no messages were sent.
No live retirement is authorized by this design document.

## Required writer/sender seam checklist

Each row needs concrete integrated race/denial/drain evidence before release; this
is a bounded source checklist, not a generalized analyzer project.

| Family | Source seam | Required check |
| --- | --- | --- |
| SQLite incl memory | infrastructure/db.py714-2926 | direct and Repo closed writes denied; accepted multiwrite finishes |
| API/portal/CLI | api/service.py; routes_web.py1154-1156 | shared operation; remove direct portal write bypass |
| slash/debug | agent/commands.py; debug.py233-477 | deny before effects; ephemeral mention-free control response works |
| gateway/roster/reactions/delete | client.py461-550,1246-1469,1689-1800 | prepare race; post-drain events mark dirty generation only |
| reminders/digest/decline | client.py605-665,1015-1039,1532-1660,1749-1800 | intent before send, grouped bind, ambiguity suppresses retry |
| extractor/cards | pipeline.py746-768,1040-1142,1157-1394 | debounce reservation/drain; no stranded-card retry |
| chat/model/tools | chat/agent.py934-1027,1255-1579,2198-2276 | full-operation lease; source keys; placeholders/reactions gated |
| rescan | agent/rescan.py112-126,156-311 | queued cancellation; live burst drains cooperatively |
| card-refresh task | client.py840-869 | pre-spawn reservation, stale copied context denied |
| process/API tasks | __main__.py191-192; api/server.py73 | task existence alone grants no write authority |
| standalone recorders | export.py231-236; extract/__main__.py205-216 | live upserts via authenticated sole owner, bounded/idempotent admission; offline exclusive owner; no second-writer fallback |
| plugin files | behaviour_plugins.py149-182 | write/delete under lease, temporary directory tests |
| identity cache | infrastructure/identity.py75-99; client.py369-384 | closed startup skips persisted refresh |
| backup/pruning | infrastructure/backup.py81-104; client.py1662-1687 | accepted backup drains, no closed prune |
| guide bypass | api/service.py2536-2694 | use journal/effect adapter |
| DM bypass | client.py1052-1112 | memory target, ambiguous DM regression |
| edits/deletes/reactions | client.py979-1013,1488-1619,1729-1800; chat/agent.py2261-2276 | ordinary denial; explicit mention-free cleanup only |
| future bundle transfer | bundle/output.py, transfer package | dedicated export/import authority, not ordinary bypass |

Authoritative reconciliation uses complete member/reaction pagination, fresh role
authorization, source-aware RSVP preservation, conflict blocking and NotFound vs
Forbidden distinctions. Gateway dirty generations and reset rollover invalidate
checkpoints; bounded retry never opens the gate on failure.
