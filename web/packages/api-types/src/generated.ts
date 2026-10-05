// Generated from the Rust API DTOs by src/api/ts_bindings.rs; do not edit.
// Regenerate: KANADE_WRITE_TS=1 cargo test --all-features --lib ts_bindings

import type { ActorKind, Answer, ChangeRecord, ChatOutcome, ChatRoute, ContextSource, Difficulty, DifficultyName, ExtractionOutcome, IdListSource, KnowledgeDoc, MissionSeries, PingLevel, ProposalKind, Refusal, RowKey, RunStatus, SelfServiceMode, SignInMethod, Surface } from './manual';

/**
 * `common.json#/$defs/Boss`.
 */
export type Boss = { token: string, key: string, name: string, difficulty: Difficulty, level: number | null, portrait: string | null, portrait_sm: string | null, art: string | null, hue: number, };

/**
 * `{id, name}` for members and channels.
 */
export type Member = { id: string, name: string, };

/**
 * `GET /api/admin/roles` row; `color` is `#rrggbb`, absent when uncoloured.
 */
export type Role = { id: string, name: string, color?: string, };

export type PublicStatus = { portal: 'open' | 'closed', };

export type ApiError = { error: string, message: string, };

export type Identity = { name: string, 
/**
 * Carries `?v=<version>` so a refreshed image is a new URL.
 */
avatar: string, banner: string, cached: boolean, 
/**
 * Changes whenever the name or the cached art does.
 */
version: string, 
/**
 * Admin origin only, once the gateway is `READY`.
 */
bot_user_id: string | null, };

export type Session = { display: string, 
/**
 * `discord`, `tailscale` or `token`: only Discord sessions may decide proposals.
 */
method: SignInMethod, };

export type SignInMethods = { discord: boolean, 
/**
 * This request carries an allow-listed identity from the authenticated edge.
 */
tailscale: boolean, token: boolean, };

/**
 * The reminder cards the PWA labels: the day-of card and two countdowns.
 */
export type CardKind = "morning" | "T-1h" | "T-15m";

export type CardState = "posted" | "queued" | "skipped";

/**
 * This or next boss week.
 */
export type WeekKey = "this" | "next";

export type WeekDay = { index: number, date: string, dow: string, is_reset: boolean, is_today: boolean, };

export type Tally = { on: number, total: number, };

export type Participant = { id: string, name: string, answer: Answer, };

export type ReminderCard = { label: CardKind, state: CardState, at: string, url: string | null, };

export type RosterChange = { out: Array<Member>, in: Array<Member>, };

export type Run = { id: string, day: number, time: string | null, 
/**
 * Derived from every boss even when an own-time run has no start clock.
 */
minutes: number, status: RunStatus, bosses: Array<Boss>, tally: Tally, short_id: string, participants: Array<Participant>, party: string, channel_id: string, channel: string, cards: Array<ReminderCard>, fixed_id: string | null, amended: boolean, roster_change: RosterChange | null, };

export type Week = { starts: string, timezone: string, reset: string, days: Array<WeekDay>, runs: Array<Run>, generated_at: string, version: number, };

export type DayStat = { day: number, answered: number, waiting: number, };

export type Stats = { per_day: Array<DayStat>, };

export type NextRun = { run_id: string, bosses: string, when: string, countdown: string, on: number, total: number, };

export type ModelBusy = { busy: boolean, holder: string | null, };

export type Summary = { next: NextRun | null, unanswered: number, inbox: number, 
/**
 * Listed members with the bossing role, as `/api/admin/members` counts them.
 */
members: number, 
/**
 * `/api/admin/reminders` `upcoming` rows over the same two weeks.
 */
reminders: number, model: ModelBusy, };

/**
 * The signed-out sign-in strip: no names, ids, answers, party, channel or version.
 */
export type TonightRun = { 
/**
 * Guild-local `HH:MM`.
 */
time: string, 
/**
 * Catalog display names.
 */
bosses: Array<string>, 
/**
 * The public week's aggregate (`on`/`total`).
 */
tally: Tally, };

export type Tonight = { 
/**
 * The next live run when it starts later today in the guild zone, else `null`.
 */
run: TonightRun | null, };

export type RunResult = { run: Run, version: number, };

export type MovePrevious = { day: number, time: string | null, };

export type MoveResult = { run: Run, previous: MovePrevious, version: number, };

export type SwapResult = { runs: [Run, Run], version: number, };

export type PingResult = { message: string, };

export type MemberRow = { id: string, name: string, nickname: string | null, aliases: Array<string>, runs_this_week: number, ping_level: PingLevel, persona: string | null, persona_available: boolean, bossing: boolean, access: 'staff' | 'pilot' | 'none', };

export type Persona = { key: string, name: string, };

export type FixedRunLink = { run_id: string, short_id: string, week: WeekKey, day: number, time: string | null, status: RunStatus, amended: boolean, };

export type FixedRow = { id: string, short_id: string, weekday: number, weekday_name: string, time: string, bosses: Array<Boss>, participants: Array<Member>, channel_id: string, channel_name: string, channel_watched: boolean, owner: string, owner_id: string, note: string | null, runs: Array<FixedRunLink>, };

export type ValidateResult = { bosses: Array<Boss>, };

/**
 * A reminder row's state: `due` is past its fire time but not yet posted.
 */
export type ReminderState = "queued" | "due" | "sent" | "stale";

export type ReminderRow = { id: string, run_id: string, run_short_id: string, kind: CardKind, state: ReminderState, at: string, bosses: Array<Boss>, party: Array<string>, url: string | null, };

export type Reminders = { upcoming: Array<ReminderRow>, sent: Array<ReminderRow>, };

export type DifficultyOption = { letter: Difficulty, name: string, token: string, in_use: boolean, };

export type BossRow = { key: string, name: string, level: number, hue: number, portrait: string | null, difficulties: Array<DifficultyOption>, };

/**
 * One boss's place in a mission series (Destiny Weapon, Union Champion).
 */
export type MissionStop = { series: MissionSeries, order: number, key: string, name: string, difficulty: DifficultyName, };

export type Knowledge = { key: string, name: string, level: number | null, portrait: string | null, 
/**
 * The looping MP4 (`/art/animated/{key}`); null where the deployment has none.
 */
animated: string | null, hue: number, researched_as_of: string | null, path: string, in_use: Difficulty[], doc: KnowledgeDoc, 
/**
 * Every boss in the series of a mission this doc defines, sorted by
 * series, `order` and key; empty when the doc has no mission.
 */
missions: Array<MissionStop>, };

export type EventBoss = { key: string, event: { name: string; availability: string }, summary: string, portrait: string | null, portrait_sm: string | null, art: string | null, animated: string | null, };

export type InboxTab = "extractor" | "self_service";

/**
 * Kanade read it from party chat (`extraction`) or was asked in chat
 * (`chat`); `self_service` is a member request.
 */
export type ProposalSource = "extraction" | "chat" | "self_service";

/**
 * Badges an inbox item can carry; each also blocks or qualifies an action.
 */
export type ProposalFlag = "conflict" | "expired" | "requester_frozen" | "requester_unauthorised" | "no_effect";

export type Evidence = { id: string, author: string, 
/**
 * `None` when the message is gone.
 */
author_id: string | null, at: string, content: string | null, url: string | null, missing: boolean, };

/**
 * A message of the thread around a card's evidence; `used` when the card
 * cites it.
 */
export type ThreadMessage = { used: boolean, id: string, author: string, 
/**
 * `None` when the message is gone.
 */
author_id: string | null, at: string, content: string | null, url: string | null, missing: boolean, };

export type FieldChange = { field: string, from: string, to: string, };

export type FieldConflict = { field: string, expected: string, found: string, };

export type ProposalPreview = { no_effect: boolean, changes: Array<FieldChange>, conflicts: Array<FieldConflict>, };

export type ProposalChoice = { run_id: string, label: string, when: string, amended: boolean, };

export type ProposalSelfService = { member: Member, via: string, note: string | null, };

export type Proposal = { id: string, short_id: string, kind: ProposalKind, kind_label: string, source: ProposalSource, tab: InboxTab, version: number, flags: Array<ProposalFlag>, preview: ProposalPreview, 
/**
 * One line on what approving does (party, upcoming reminders); `None`
 * with conflicts, no effect, or nothing to say.
 */
consequence: string | null, expires_at: string | null, choices: Array<ProposalChoice> | null, public_summary: string | null, bosses: Array<Boss>, run_id: string | null, from_when: string | null, when: string, participants: Array<Member>, confidence: number | null, is_question: boolean, channel: string | null, read_at: string, summary: string, evidence: Array<Evidence>, 
/**
 * The channel thread around `evidence`; `None` when there is none.
 */
thread: Array<ThreadMessage> | null, card_url: string | null, self_service: ProposalSelfService | null, };

/**
 * How a closed Inbox item ended: `approved` = merged, `superseded` =
 * replaced by a newer proposal.
 */
export type PastOutcome = "approved" | "rejected" | "superseded" | "discarded" | "withdrawn" | "expired";

/**
 * Who closed it; `name` is ready to show (system actors read as Kanade).
 */
export type PastDecider = { kind: ActorKind, id: string, name: string, };

export type PastItem = { id: string, short_id: string, kind: ProposalKind | 'change', kind_label: string, 
/**
 * `extractor` for proposals, `self_service` for member requests.
 */
tab: InboxTab, source: ProposalSource, 
/**
 * The extraction log or chat interaction that staged a proposal.
 */
source_id: string | null, summary: string, channel: string | null, 
/**
 * The member who asked (requests only).
 */
requester: Member | null, outcome: PastOutcome, decided_by: PastDecider | null, decided_at: string, reason: string | null, created_at: string, 
/**
 * The History record an approval wrote.
 */
history_seq: number | null, evidence: Array<Evidence>, card_url: string | null, };

export type PastPage = { items: Array<PastItem>, 
/**
 * The `before` cursor of the next page; `None` on the last.
 */
next_before: string | null, };

export type ConfigView = { pings: Pings, watching: Watching, chatbot: Chatbot, notifications: Notifications, self_service: SelfServiceSettings, persona: PersonaSettings, models: ModelSettings, run_lengths: RunLengths, profanity: ProfanitySettings, manage_messages: ManageMessages, notices: Array<string>, env: Array<EnvRow>, 
/**
 * `null` when no digest is active or the journal could not be read.
 */
last_digest: LastDigest | null, };

export type Pings = { day_of_ping_time: string, countdown_minutes: Array<number>, };

export type Watching = { paused: boolean, extract_enabled: boolean, 
/**
 * Effective watched channel ids (saved row, else the env seed).
 */
channel_ids: Array<string>, 
/**
 * `saved` once an explicit list save stored the list, else `env`.
 */
channel_ids_source: IdListSource, category_ids: Array<string>, category_ids_source: IdListSource, };

export type Rate = { count: number, window_s: number, };

export type Chatbot = { enabled: boolean, configured: boolean, missing_env: Array<string>, member_rate: Rate, guild_rate: Rate, 
/**
 * Effective chat category ids (saved row, else the env seed).
 */
category_ids: Array<string>, category_ids_source: IdListSource, };

export type Notifications = { quiet_mode: boolean, };

export type SelfServiceSettings = { mode: SelfServiceMode, effective_mode: SelfServiceMode, public_portal: boolean, };

export type PersonaEntry = { key: string, name: string, bundle: string, };

export type ReplyProfile = { key: string, name: string, public: boolean, voice: string, prompt_summary: string, };

export type RoleProfileView = { role_id: string, role_name: string | null, profile: string, };

export type PersonaSettings = { active: string, personas: Array<PersonaEntry>, profiles: Array<ReplyProfile>, role_profiles: Array<RoleProfileView>, role_profiles_digest: string, };

export type ModelSettings = { reachable: boolean, catalog: Array<ModelInfo>, roles: ModelRoles, groups: Array<CapacityGroup>, 
/**
 * `default` (the one `gateway` group) or `config` (`[[models.groups]]`).
 */
groups_source: 'default' | 'config', alias_limits: Array<AliasLimit>, key_limits: KeyLimits, capacity_check: Array<CapacityCheck>, pii_pseudonymise: boolean, context: ContextSettings, };

export type ModelInfo = { id: string, trust_zone: 'homelab' | 'external' | 'unknown', leaves_homelab: boolean, function_tools: boolean, structured_output: boolean, sampling_controls: boolean, reasoning_control: boolean, reasoning_efforts: Array<string> | null, context_tokens?: number, max_output_tokens?: number, 
/**
 * False when the alias requires reasoning (a published list without `none`).
 */
off_allowed: boolean, admission: Admission | null, 
/**
 * Set on a listed `<base>:<level>` alias: the picker lists the base only.
 */
variant_of?: string, 
/**
 * The variant's baked-in level in the `reasoning` vocabulary (`:none` is `off`).
 */
fixed_effort?: string, };

export type Admission = { max_in_flight: number, adapter_max_in_flight?: number, };

export type ModelRoles = { extraction: RoleModel, chat: RoleModel, rewrite: RoleModel, };

export type RoleModel = { alias: string, reasoning: string, 
/**
 * A stored variant alias: shown as "`variant_of` (fixed: `fixed_effort`)".
 */
variant_of?: string, fixed_effort?: string, 
/**
 * What the role's next session opens with; absent while unrouted.
 */
running?: RunningRole, context?: EffectiveContext, };

/**
 * The running alias and the level requests send (inherit and floors resolved).
 */
export type RunningRole = { alias: string, reasoning: string | null, };

export type EffectiveContext = { window: number, reserve: number, prompt_budget: number, source: ContextSource, clamped_by_published: boolean, clamped_by_hard_cap: boolean, clamped_by_role_cap: boolean, local_warning: boolean, };

export type ContextRole = { reserve: number, cap: number | null, };

export type ContextSettings = { cloud_default: number, local_default: number, chat: ContextRole, extraction: ContextRole, rewrite: ContextRole, overrides: { [key in string]: number }, };

export type CapacityGroup = { model: string, group: string, permits: number | null, 
/**
 * The group's permits held by calls in flight, from the governor's
 * snapshot; `null` while model serving is not composed.
 */
in_use: number | null, };

export type AliasLimit = { alias: string, max_in_flight: number, adapter_max_in_flight?: number, source: 'published' | 'declared', };

export type KeyLimits = { 
/**
 * `None`: Kanata publishes no per-key limit.
 */
max_in_flight: number | null, shared: boolean, };

export type CapacityCheck = { level: 'ok' | 'warning' | 'error', message: string, 
/**
 * The capacity group the check is about (`models.groups[].group`);
 * `null` for checks that span groups.
 */
group: string | null, };

export type RunLengths = { default_minutes: number, overrides: Array<RunLengthOverride>, };

export type RunLengthOverride = { boss: string, difficulty: Difficulty, minutes: number, };

/**
 * The chat profanity guardrail. `builtin_words` is read-only: the code-owned
 * list, each entry of which may be allowed again.
 */
export type ProfanitySettings = { extra_words: Array<string>, allowed_words: Array<string>, check_questions: boolean, check_replies: boolean, deflection_line: string, builtin_words: Array<string>, };

export type ManageMessages = { missing: Array<string>, };

export type EnvRow = { key: string, label: string, value: string, reason: string, 
/**
 * The raw value to paste into the deployment env for `key`; `null` when
 * unset or when the row has no single env value. Never a secret.
 */
copy: string | null, };

/**
 * The most recent active weekly digest card.
 */
export type LastDigest = { 
/**
 * RFC 3339 in the guild's offset, whole seconds.
 */
posted_at: string, 
/**
 * Guild-local boss-week start date (`YYYY-MM-DD`).
 */
week_start: string, this_week: boolean, channel_id: string, channel_name: string | null, url: string | null, };

export type AccessReport = { connected: boolean, 
/**
 * Guild-local, e.g. `Tue 29 Sep 12:00`.
 */
checked_at: string, rows: Array<AccessRow>, };

export type AccessRow = { id: string, name: string, watched: boolean, digest: boolean, view: boolean, send: boolean, history: boolean, embed: boolean, react: boolean, manage_messages: boolean, };

/**
 * What the log filters can offer (all values seen, not only the filtered rows').
 */
export type LogFacets = { models: Array<string>, tools: Array<string>, outcomes: Array<string>, channels: Array<Member>, };

/**
 * Reported token usage over a set of logged requests: sums over those that
 * reported a pair (null when none did), how many did, and the median of
 * reported prompt tokens / local estimate (two decimals; null when none).
 */
export type UsageSummary = { prompt_tokens: number | null, completion_tokens: number | null, reported: number, est_ratio: number | null, };

/**
 * Per model over the listed (filtered) extraction calls.
 */
export type ExtractionSummary = { model: string, count: number, prompt_tokens: number | null, completion_tokens: number | null, reported: number, est_ratio: number | null, };

export type ExtractionRow = { messages: number, changes: number, id: string, short_id: string, at: string, model: string, latency_ms: number | null, channel: string | null, channel_id: string, error: string | null, outcome: ExtractionOutcome, 
/**
 * Provider-reported tokens summed over the call's reporting attempts; null = not reported (never 0).
 */
prompt_tokens?: number | null, completion_tokens?: number | null, 
/**
 * Provider-reported reasoning tokens over reporting attempts; null = unknown.
 */
reasoning_tokens?: number | null, };

/**
 * Query params of `GET /api/admin/extractions`, all optional and
 * combinable: `model`, `from`/`to` (guild-local YYYY-MM-DD), `outcome`
 * (comma-separated, any of), `channel`, `member`, `q`. Unknown outcomes or
 * malformed dates: 422 invalid_filter.
 */
export type Extractions = { model: string, 
/**
 * Per model over the filtered calls.
 */
summary: Array<ExtractionSummary>, rows: Array<ExtractionRow>, 
/**
 * Rows before filtering.
 */
total: number, facets: LogFacets, };

export type Amendment = { kind: string, bosses: string, when: string, confidence: number, status: string, };

export type ReadMessage = { id: string, author: string, author_id?: string, at: string, content: string, };

/**
 * The context the call was budgeted for.
 */
export type CallContext = { window: number, reserve: number, source: string, };

/**
 * A change the scheduler refused to stage for the call.
 */
export type ExtractionRefusal = { change: string, code: string, message: string, };

export type Extraction = { prompt: string, raw_response: string, 
/**
 * Response-only text, capped at 64 KiB including a visible marker.
 */
reasoning_content?: string | null, 
/**
 * Local prompt estimate over the attempts that reported usage, else every sent attempt.
 */
prompt_estimate?: number | null, 
/**
 * Null when not logged whole.
 */
context?: CallContext | null, amendments: Array<Amendment>, messages: Array<ReadMessage>, refusals?: Array<ExtractionRefusal>, 
/**
 * The call session's gateway correlation stem; null when not recorded.
 */
session_id?: string | null, 
/**
 * Every `x-request-id` the call sent, in order (retries included);
 * empty when none was recorded.
 */
request_ids?: Array<string>, id: string, short_id: string, at: string, model: string, latency_ms: number | null, channel: string | null, channel_id: string, error: string | null, outcome: ExtractionOutcome, 
/**
 * Provider-reported tokens summed over the call's reporting attempts; null = not reported (never 0).
 */
prompt_tokens?: number | null, completion_tokens?: number | null, 
/**
 * Provider-reported reasoning tokens over reporting attempts; null = unknown.
 */
reasoning_tokens?: number | null, };

export type JobState = "running" | "done" | "cancelled";

export type ChannelState = "queued" | "reading" | "done";

export type RescanChannel = { id: string, name: string, state: ChannelState, 
/**
 * The gated messages its read found (0 until `done`).
 */
messages: number, unread?: number, 
/**
 * Fixed sentences naming what went wrong, never the recorded text.
 */
errors?: Array<string>, };

export type RescanJob = { id: string, state: JobState, window: 'week' | 'since_reset' | 'two_weeks', 
/**
 * When the runner took the job (UTC `Z`); null while queued.
 */
started_at?: string | null, channels: Array<RescanChannel>, 
/**
 * The channels' `messages`: gated messages read so far.
 */
messages?: number, 
/**
 * `messages` plus each unread channel's gated messages as cached when the job
 * started; equals `messages` once the job ends. Null while a channel still to
 * be read has no count. Progress = messages / messages_total.
 */
messages_total?: number | null, proposals: number, unread?: number, };

export type ChatRow = { id: string, at: string, member: Member, 
/**
 * The full Discord id, even when `member.name` is a placeholder.
 */
member_id?: string | null, channel: string | null, channel_id: string, 
/**
 * The first round's alias ("—" when no model was called).
 */
model: string, 
/**
 * Model alias per request round.
 */
models: Array<string>, latency_ms: number, outcome: ChatOutcome, asked: string, tools_used: Array<string>, 
/**
 * Turn totals as logged (v4 imports may carry one); null = not reported.
 */
prompt_tokens?: number | null, completion_tokens?: number | null, 
/**
 * Sum of the rounds' reported reasoning counts; null = unknown.
 */
reasoning_tokens?: number | null, };

/**
 * Per model; usage fields come from this model's round rows, never the
 * turn totals.
 */
export type ChatSummary = { model: string, count: number, answered: number, refused: number, errors: number, p50_ms: number, tool_calls: number, prompt_tokens: number | null, completion_tokens: number | null, reported: number, est_ratio: number | null, };

/**
 * Query params of `GET /api/admin/chat`, all optional and combinable:
 * `model`, `from`/`to` (guild-local YYYY-MM-DD), `outcome` (comma-separated,
 * any of), `channel`, `member`, `q`, `tool` and `min_ms`. Unknown outcomes or
 * malformed dates: 422 invalid_filter.
 */
export type Chat = { 
/**
 * Per model, over the filtered rows.
 */
summary: Array<ChatSummary>, rows: Array<ChatRow>, total: number, facets: LogFacets, };

export type ChatToolCall = { 
/**
 * The request round (1-based index into `rounds`) whose reply asked for it.
 */
round: number, name: string, arguments: string, result: string, 
/**
 * Wall time; null when unknown (0 is a real 0 ms).
 */
took_ms: number | null, outcome: string, };

export type RoundGuardrail = { clean: boolean, content_filter: boolean, };

export type ChatRoundFacts = { round: number, requested_tools: Array<string>, finish: string, 
/**
 * Alias the request named, as sent.
 */
model: string, 
/**
 * Reasoning effort as sent (after capability shaping); null when none went out.
 */
effort: string | null, route: ChatRoute | null, 
/**
 * null when unknown.
 */
latency_ms: number | null, 
/**
 * Provider-reported usage for this request (both or neither); null = not reported.
 */
prompt_tokens?: number | null, completion_tokens?: number | null, 
/**
 * The context budget's estimate, completion reserve excluded.
 */
prompt_estimate?: number | null, reasoning_content?: string | null, reasoning_tokens?: number | null, 
/**
 * Every `x-request-id` this round sent, in order (retries included);
 * empty when none was recorded.
 */
request_ids?: Array<string>, guardrail: RoundGuardrail, };

export type ChatCard = { kind: string, url: string, };

export type MaskedRoundView = { round: number, clean: boolean, 
/**
 * The request messages exactly as sent (masked).
 */
request: { role: 'system' | 'user' | 'assistant' | 'tool'; [key: string]: unknown }[], 
/**
 * The model's reply before names were restored.
 */
reply: string | null, 
/**
 * Tool-call arguments before names were restored.
 */
tool_calls: { name: string; arguments: string }[], };

export type TokenName = { token: string, name: string, };

/**
 * A pseudonymized turn as the model saw it (admin only).
 */
export type ModelView = { rounds: Array<MaskedRoundView>, 
/**
 * The decoded, finished reply members saw.
 */
reply: string, 
/**
 * Fake name → member display name; never user ids.
 */
mapping: Array<TokenName>, };

/**
 * A profanity guardrail hit (`guardrail.profanity`, outcome `profanity`).
 */
export type ProfanityDetail = { 
/**
 * The member's question (deflected, no model call) or the finished reply.
 */
side: 'question' | 'reply', 
/**
 * The deny-listed word that matched (the first hit).
 */
word: string, 
/**
 * The line sent instead; null when the reply's clean retry came back
 * clean and was delivered.
 */
sent: string | null, };

export type ChatTurn = { said: string, tools: Array<ChatToolCall>, rounds: Array<ChatRoundFacts>, cards: Array<ChatCard>, raw: string, 
/**
 * Persona bundle id; null when none answered (rate limited, imported).
 */
persona: string | null, 
/**
 * Reply profile id; null for the bundle default voice.
 */
profile: string | null, profile_source: 'saved' | 'role' | 'default' | null, 
/**
 * The turn's route (its last round's); null when no model ran.
 */
route: ChatRoute | null, error: string | null, 
/**
 * Stable code: timeout, malformed, content_blocked, identity_leak_blocked, rate_limited, …
 */
error_code: string | null, 
/**
 * The question session's gateway correlation stem; each request went
 * out as `{session_id}-{n}`. Null when not recorded.
 */
session_id?: string | null, 
/**
 * content_filter, external_unmasked, pseudonymized, identity_leak_blocked {role, kinds, count}, …
 */
guardrail: Record<string, unknown>, 
/**
 * Pseudonymized, with a stored Model view.
 */
masked: boolean, 
/**
 * Null for passthrough and withheld turns.
 */
model_view: ModelView | null, 
/**
 * Set on `profanity` turns: which side hit, the word and the line sent.
 */
profanity?: ProfanityDetail | null, id: string, at: string, member: Member, 
/**
 * The full Discord id, even when `member.name` is a placeholder.
 */
member_id?: string | null, channel: string | null, channel_id: string, 
/**
 * The first round's alias ("—" when no model was called).
 */
model: string, 
/**
 * Model alias per request round.
 */
models: Array<string>, latency_ms: number, outcome: ChatOutcome, asked: string, tools_used: Array<string>, 
/**
 * Turn totals as logged (v4 imports may carry one); null = not reported.
 */
prompt_tokens?: number | null, completion_tokens?: number | null, 
/**
 * Sum of the rounds' reported reasoning counts; null = unknown.
 */
reasoning_tokens?: number | null, };

export type Permits = { in_use: number, total: number, };

export type QueuedCall = { position: number, kind: string, who: string, waiting_s: number, };

export type RateLevel = { available: number, capacity: number, refill_per_min: number, };

export type RetryLevel = { remaining: number, capacity: number, };

export type Breaker = { state: 'closed' | 'half_open' | 'open', failures: number, since: string, 
/**
 * Set only while a probe is scheduled (open).
 */
retry_at?: string | null, };

export type BackendGroup = { name: string, backend: string, models: Array<string>, permits: Permits, queue: Array<QueuedCall>, rate: RateLevel, retry: RetryLevel, breaker: Breaker, };

export type AdmissionWindow = { window: string, 
/**
 * No refusal is recorded yet, so the list is always empty.
 */
refusals: Refusal[], };

export type Quota = { count: number, per_s: number, };

export type Allowance = { member: Member, staff: boolean, 
/**
 * Null for staff (no limit).
 */
allowance: Quota | null, used: number, override: boolean, };

export type Limits = { groups: Array<BackendGroup>, admission: AdmissionWindow, allowances: Array<Allowance>, 
/**
 * The server's clock when the snapshot was taken (ISO-8601 UTC), as `Week.generated_at`.
 */
generated_at: string, };

export type MeMember = { id: string, name: string, access: 'staff' | 'pilot' | 'none', 
/**
 * Holds the bossing role (on the roster).
 */
bossing: boolean, 
/**
 * Current guild roles, highest first; ids the directory cannot name are
 * left out. Null while the role directory is unavailable.
 */
roles: Array<Role> | null, 
/**
 * The member's row exactly as Limits shows it; null without chatbot access.
 */
allowance: Allowance | null, };

export type Me = { display: string, method: SignInMethod, 
/**
 * The guild member behind a Discord sign-in; null for the admin token,
 * Tailscale, and a Discord account with no member row.
 */
member: MeMember | null, };

/**
 * `{seq, hash}`: a record in the chain.
 */
export type ChainHead = { seq: number, hash: string, };

/**
 * `GET /api/admin/history?week&actor&run&before&limit`, newest first. With
 * `run=<id>` (alone; not with `week` or `actor`): the run's change log, each
 * record changing its row or RSVPs; the run's before → after is in those
 * `rows` (`runs` keyed by `id`, `rsvps` by `run_id`).
 */
export type HistoryPage = { records: ChangeRecord[], head: ChainHead, 
/**
 * Pass as `before` for the next (older) page; null on the last page.
 */
next_before: number | null, 
/**
 * Matching records (journal only; `settings_total` counts Config saves).
 */
total: number, 
/**
 * Config section saves in this page's time window, newest first: at or
 * after the page's oldest record (no lower bound on the last page) and
 * before the oldest record of the page `before` came from (no upper
 * bound on the first page), so each save appears on exactly one page.
 * Always empty with `run`.
 */
settings: Array<SettingsChangeRow>, 
/**
 * Config saves matching `week`/`actor` across all pages.
 */
settings_total: number, };

/**
 * `{kind, id}`, as a record names its actor.
 */
export type SettingsActor = { kind: ActorKind, id: string, };

/**
 * One stored settings row's text before and after the save.
 */
export type SettingRowDiff = { key: string, from: string, to: string, };

/**
 * A saved Config section: view-only, outside the hash chain, never
 * revertible. Settings rows never hold a secret.
 */
export type SettingsChangeRow = { id: number, at: string, actor: SettingsActor, surface: Surface, 
/**
 * The saved section; the PWA links `/config?section=<section>`.
 */
section: string, 
/**
 * The settings revision the save published (counted per process run).
 */
revision: number, 
/**
 * The boss week containing `at`, named as records name weeks.
 */
week: string, 
/**
 * Changed rows, by key.
 */
values: Array<SettingRowDiff>, };

export type RowChange = { key: RowKey, 
/**
 * Full domain row; null = absent.
 */
before: Record<string, unknown> | null, after: Record<string, unknown> | null, };

export type RowConflict = { seq: number, key: RowKey, expected: unknown, found: unknown, };

export type SkippedKey = { key: RowKey, reason: string, };

export type PlanOutcome = "preview" | "applied" | "unchanged" | "conflicts";

export type RevertPlan = { outcome: PlanOutcome, 
/**
 * The requested records (a refused week or actor rollback: the conflicting ones).
 */
reverts: Array<number>, 
/**
 * Empty when `outcome` is `conflicts`: a strict refusal plans nothing.
 */
rows: Array<RowChange>, conflicts: Array<RowConflict>, skipped: Array<SkippedKey>, record: ChangeRecord | null, };

/**
 * `matches`: the chain holds the backup's head; `older_schema`: it does,
 * but the backup predates the store's schema; `mismatch`: the head is not
 * in the chain (truncated or forked history).
 */
export type BackupAnchor = "matches" | "older_schema" | "mismatch";

export type BackupRow = { file: string, format: 'kanade.backup.v1', created_at: string, history_head: ChainHead, revision: number, schema_version: number, 
/**
 * The chain still contains `history_head`.
 */
anchored: boolean, anchor: BackupAnchor, };

export type Verified = { ok: boolean, checked: number, head: ChainHead, };

export type Checkpoints = { verified: Verified, 
/**
 * `KANADE_BACKUP_DIR` is set: false means no directory, not no backups.
 */
backup_dir_configured: boolean, 
/**
 * Newest first (at most 100); re-read and re-checked on every request.
 */
backups: Array<BackupRow>, };
