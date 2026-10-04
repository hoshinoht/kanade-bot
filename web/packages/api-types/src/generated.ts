// Generated from the Rust API DTOs by src/api/ts_bindings.rs; do not edit.
// Regenerate: KANADE_WRITE_TS=1 cargo test --all-features --lib ts_bindings

import type { ActorKind, Answer, CardKind, CardState, ContextSource, Difficulty, InboxTab, KnowledgeDoc, PastOutcome, PingLevel, ProposalFlag, ProposalKind, ProposalSource, RunStatus, SelfServiceMode, SignInMethod, WeekKey } from './manual';

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

export type ReminderRow = { id: string, run_id: string, run_short_id: string, kind: CardKind, state: 'queued' | 'due' | 'sent' | 'stale', at: string, bosses: Array<Boss>, party: Array<string>, url: string | null, };

export type Reminders = { upcoming: Array<ReminderRow>, sent: Array<ReminderRow>, };

export type DifficultyOption = { letter: Difficulty, name: string, token: string, in_use: boolean, };

export type BossRow = { key: string, name: string, level: number, hue: number, portrait: string | null, difficulties: Array<DifficultyOption>, };

export type Knowledge = { key: string, name: string, level: number | null, portrait: string | null, 
/**
 * The looping MP4 (`/art/animated/{key}`); null where the deployment has none.
 */
animated: string | null, hue: number, researched_as_of: string | null, path: string, in_use: Difficulty[], doc: KnowledgeDoc, };

export type EventBoss = { key: string, event: { name: string; availability: string }, summary: string, portrait: string | null, portrait_sm: string | null, art: string | null, animated: string | null, };

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

export type Proposal = { id: string, short_id: string, kind: ProposalKind, kind_label: string, source: ProposalSource, tab: InboxTab, version: number, flags: ProposalFlag[], preview: ProposalPreview, 
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

export type ConfigView = { pings: Pings, watching: Watching, chatbot: Chatbot, notifications: Notifications, self_service: SelfServiceSettings, persona: PersonaSettings, models: ModelSettings, run_lengths: RunLengths, manage_messages: ManageMessages, notices: Array<string>, env: Array<EnvRow>, 
/**
 * `null` when no digest is active or the journal could not be read.
 */
last_digest: LastDigest | null, };

export type Pings = { day_of_ping_time: string, countdown_minutes: Array<number>, };

export type Watching = { paused: boolean, extract_enabled: boolean, };

export type Rate = { count: number, window_s: number, };

export type Chatbot = { enabled: boolean, configured: boolean, missing_env: Array<string>, member_rate: Rate, guild_rate: Rate, };

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

export type CapacityGroup = { model: string, group: string, permits: number | null, };

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
