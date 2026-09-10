# Chatbot memory policy

Kanade's durable memory is an optional, administrator-managed feature for typed
reply preferences. `CHAT_MEMORY_ENABLED` defaults to `false`, and changing it
requires a restart. It is not a transcript archive, model-training system, or a
way to learn boss facts from members.

## Individual enrollment

`CHAT_MEMORY_ENABLED=true` makes the capability available, but enrolls nobody.
Holding the bossing role does not opt a member in, and there is no bulk or
role-wide enrollment.

An administrator enrolls one member at a time through the authenticated portal
or `bossctl`. The enrollment stays inactive until Kanade successfully sends that
member a Discord direct-message notice. The notice explains:

- which typed preferences may be stored;
- how long they are retained;
- that portal administrators can view and modify them;
- how to opt out or delete memory; and
- the limits of logical deletion.

If the notice cannot be delivered, memory remains inactive for that member and
the administrator sees an actionable error. Retrying the notice is explicit and
audited.

Delivery is the activation condition; the notice is not a second policy-acceptance
flow. The member may use opt-out or deletion controls without accepting anything.

## What is remembered

The MVP stores only allowlisted presentation preferences:

- answer detail: concise, standard, or detailed;
- answer format: prose, bullets, or steps;
- strategy disclosure: none, hints, or full; and
- strategy emphasis: mechanics, survival, or party roles.

Strategy preferences may be scoped to a catalog-resolved boss. Free text,
conversation summaries, shared channel memory, and automatic extraction from
ordinary messages are not stored as durable memory.

A member can propose a preference with the explicit
`remember preference: <slot>=<value>` syntax. Kanade posts a review card and
only that member can approve it. Portal administrators may directly create or
correct typed preferences for an actively enrolled member; every change records
a content-free lifecycle event identifying the administrator and action.

The complete single-line forms are:

```text
remember preference: <slot>=<value>
remember preference: <slot>=<value>; boss=<boss-reference>
```

Keywords, slots, and values are ASCII case-insensitive; surrounding whitespace
around the delimiters is ignored. Exactly one slot/value is accepted. The
allowlisted slots and values are `answer_detail` (`concise`, `standard`,
`detailed`), `answer_format` (`prose`, `bullets`, `steps`),
`strategy_disclosure` (`none`, `hints`, `full`), and `strategy_emphasis`
(`mechanics`, `survival`, `party_roles`). A `boss=` clause is optional and valid
only for the two strategy slots; its 1–64 character reference must resolve to a
catalog boss with an explicit supported difficulty. Extra clauses, duplicate
keys, unknown values, newlines, negation, quotations, code blocks, questions,
reply-derived text, and `remember when` text are rejected deterministically and
are never sent to the model as memory.

## Opt-out and deletion

Members may view their memory, opt out, or delete one or all memories at any
time. These actions remain available when memory is globally disabled or the
member is not actively enrolled. They do not require accepting the memory
notice or any additional policy prompt.

The guild-only `/memory` command group is member-scoped and ephemeral:
`/memory status`, `/memory list`, `/memory opt-out`, `/memory forget id:<id>`,
and `/memory forget-all`. Members cannot self-enroll. To correct a value, submit
a new typed proposal; administrators use the portal or `bossctl` for direct
typed edits.

Opting out immediately revokes active preferences and invalidates pending
proposals. Re-enrollment requires a new administrator action and a successfully
delivered notice; it never restores old preferences.

Deletion immediately physically deletes the live `chat_memories` row and removes
its preference content from current retrieval. A confirmation may identify the
affected item and deletion consequences, but it is not a policy-acceptance gate.
Content-free lifecycle events may remain for audit. The policy calls this logical
live-memory deletion because it guarantees removal from the live memory surface,
not forensic erasure: related text can still exist in Discord, watched-message or
chatbot logs, SQLite free pages or WAL files, manually created database copies,
and older managed backups until those backups age out.
New managed backups omit purged live rows; existing snapshots age out under the
deployment's current backup rotation, without changing the free-page/WAL caveat.

## Retention

- Pending proposals expire after 7 days.
- Active preferences expire after 180 days.
- Rejected, revoked, superseded, and expired content is purged after 30 days.
- Retrieval diagnostics retain at most 30 days and the newest 500 rows.
- Content-free lifecycle events are purged after 365 days.

The reminder worker runs this cleanup on its first tick and no more than once an
hour thereafter, even when `CHAT_MEMORY_ENABLED=false`. A failed sweep is logged
and retried on the next eligible hourly tick; it does not stop reminders, chat,
or the portal.

## Authority and isolation

Memory is filtered by trusted guild and member identity before retrieval. It is
rendered to the model as untrusted presentation data and cannot grant tool
access, change identity or policy, mutate schedules, or override checked-in boss
knowledge. Schedule state remains authoritative for runs, and YAML remains
authoritative for boss mechanics and sources.

The authenticated portal exposes enrollment, typed values, provenance,
lifecycle state, and deletion controls. Administrators can enroll or disable one
member, edit typed values for active enrollments, and revoke or delete memory.
Members cannot self-enroll, but their opt-out and deletion controls always take
priority.

The portal's **Memory** page supports filters by member, enrollment state,
lifecycle, slot, boss, and expiry, plus one-member enrollment and detail pages.
The JSON API exposes the same administrator operations at `GET /api/memory`,
`GET /api/memory/{user_id}`, `POST /api/memory/{user_id}/enroll`,
`POST /api/memory/{user_id}/disable`, `PUT /api/memory/{user_id}/memories`,
`POST /api/memory/{user_id}/memories/{memory_id}/revoke`,
`POST /api/memory/{user_id}/memories/{memory_id}/expire`, and
`DELETE /api/memory/{user_id}/memories/{memory_id}`. All require portal
authentication. The HTTP-only CLI mirrors them with `bossctl memory list`,
`show`, `enroll`, `disable`, `set`, `revoke`, `expire`, and `delete`.

Boss mechanics remain checked-in YAML authority. Authenticated source details are
available at the portal page `/bosses/{boss}/knowledge` and JSON route
`GET /api/bosses/{boss}/knowledge`, which show the catalog-resolved boss,
researched date, source path, hashes, and full source URLs. Discord strategy
answers show only bounded hostname/date/count attribution. Catalog and knowledge
files load at startup, so changes to `BOSSES_PATH` or `BOSS_KNOWLEDGE_PATH`
require a restart; image assets only need a page reload.

## Rollout and evaluation

Keep the feature disabled until the deployment notice, policy interpretation, and
logical-deletion limitation are accepted. A canary must be explicitly enabled;
the setting alone never enrolls a member. Review retrieval usefulness, false or
stale-memory use, unsupported claims, cross-scope isolation, deletion
verification, latency, and prompt context size. This MVP does not use memory for
model training.
