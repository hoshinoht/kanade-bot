// Hand-written API types with no typed Rust counterpart yet: string vocabularies
// the generated types name, request bodies, the mock-only public week, the
// YAML knowledge document, and responses the server still builds with `json!`
// (logs, rescans, history, limits). Everything else is in `generated.ts`.

import type { Boss, Member, RoleProfileView, Tally, WeekDay } from './generated';

export type RunStatus = 'planned' | 'confirmed' | 'at_risk' | 'otot' | 'done' | 'cancelled';
export type Answer = 'yes' | 'no' | 'maybe' | 'waiting';
export type Difficulty = 'e' | 'n' | 'h' | 'c' | 'x';

/**
 * Public projection of a run (`GET /api/public/week`): schedule facts only.
 * - `id`: opaque render key, stable across polls; grants nothing (the public
 *   origin has no mutation routes).
 * - `day`, `time`, `status`, `bosses`: the schedule itself.
 * - `tally`: aggregate count, as v4's public board shows ("3/4").
 * Omitted: participant names/ids and answers (personal data), `party`
 * (internal channel handle), `version` (a concurrency token for writers).
 */
export interface PublicRun {
  id: string;
  /** Day offset from the boss-week reset day, 0..6. */
  day: number;
  /** Local `HH:MM`, or null for own-time runs. */
  time: string | null;
  status: RunStatus;
  bosses: Boss[];
  tally: Tally;
}

export interface PublicWeek {
  starts: string;
  timezone: string;
  reset: string;
  days: WeekDay[];
  runs: PublicRun[];
  /** Server clock (ISO) so clients can show freshness. */
  generated_at: string;
}

export type CardKind = 'morning' | 'T-1h' | 'T-15m';
export type CardState = 'posted' | 'queued' | 'skipped';

/** Either projection; enough for shared presentation components. */
export type WeekShape = Pick<PublicWeek, 'days' | 'timezone'>;

export interface MoveRequest {
  day: number;
  time: string | null;
  /** Optimistic concurrency: the week version the client moved from. */
  version: number;
}

export type WeekKey = 'this' | 'next';

export type PingLevel = 'essential' | 'all' | 'off';

export interface MemberPatch {
  ping_level?: PingLevel;
  /** Empty string clears back to the default style. */
  persona?: string;
}

export interface FixedRequest {
  weekday: number;
  time: string;
  /** Boss text as v4 parsed it: `hstar, hfa` or tokens `HStar HFA`. */
  bosses: string;
  participants: string[];
  channel_id: string;
  note: string | null;
  /** A rostered member (bossing role, not a bot), not necessarily in the party (else 422 `invalid`). Omitted: POST keeps its default, PATCH leaves the owner unchanged. */
  owner_id?: string;
  /** For each amended run of the timing: follow the new timing, or keep this week's change. */
  decisions?: Record<string, 'update' | 'keep'>;
  /** Week version the form was loaded at: required by PATCH (else 422 `version_required`), ignored by POST. */
  version?: number;
  /** admin-api "Edit preconditions": replace the version-derived expectations. */
  expect?: { field: string; seen: number | null }[];
  override?: { seq: number; hash: string }[];
}

export type DifficultyName = 'Easy' | 'Normal' | 'Hard' | 'Chaos' | 'Extreme';

/** One difficulty's facts (boss/knowledge schema v2). Unit-bearing values stay strings (`241.5t`). */
export interface DifficultyFacts {
  name: DifficultyName;
  entry_level?: number;
  boss_level?: number;
  pdr_percent?: number;
  party_max?: number;
  force?: { kind: 'arcane' | 'sacred'; value: number };
  hp?: { phase: string; value: string }[];
  recommended_spec?: { kind: string; text: string };
  notes?: string[];
}

export interface KnowledgeSource {
  url: string;
  title: string;
  author: string;
  kind: 'guide' | 'wiki' | 'tool' | 'official';
  fetched: string;
  updated?: string;
}

export type StrategyLevel = 'low' | 'medium' | 'high';

/** One way to run the fight; `damage` is the damage it asks of the party. */
export interface Strategy {
  name: string;
  when: string;
  risk: StrategyLevel;
  damage: StrategyLevel;
  payoff: string;
  /** 1–6 ordered steps. */
  steps: string[];
}

/** A tracked `boss/knowledge/<key>.yaml` document, as validated against its schema. */
export interface KnowledgeDoc {
  boss: string;
  summary: string;
  core: string[];
  danger: string[];
  tips: string[];
  difficulty_notes?: Partial<Record<Difficulty, string>>;
  notes?: string[];
  /** Seasonal/event bosses outside the catalog (e.g. Kai). */
  event?: { name: string; availability: string };
  difficulties?: DifficultyFacts[];
  /** 1–4 strategies; absent on most documents. */
  strategies?: Strategy[];
  sources: KnowledgeSource[];
}

/** `GET /api/admin/channels` rows share the member shape. */
export type Channel = Member;

export type SignInMethod = 'discord' | 'tailscale' | 'token';

export interface StatusRequest {
  status: RunStatus;
  version: number;
}

export interface RsvpRequest {
  member_id: string;
  answer: 'yes' | 'no' | 'clear';
  version: number;
}

export interface ParticipantsRequest {
  add?: string;
  remove?: string;
  version: number;
}

/** `POST /api/admin/runs/{id}/swap`: exchange this run's slot with `with`. */
export interface SwapRequest {
  with: string;
  version: number;
}

// ── Inbox ─────────────────────────────────────────────────────────────────

export type InboxTab = 'extractor' | 'self_service';
/** Badges an inbox item can carry; each also blocks or qualifies an action. */
export type ProposalFlag = 'conflict' | 'expired' | 'requester_frozen' | 'requester_unauthorised' | 'no_effect';
/** A proposal's change kind, or a member request's type (`new_fixed` … `swap`). */
export type ProposalKind = 'move' | 'add' | 'cancel' | 'split' | 'otot' | 'sub' | 'rsvp' | 'fix' | 'new_fixed' | 'change_fixed' | 'join' | 'leave' | 'swap';
/** Kanade read it from party chat (`extraction`) or was asked in chat (`chat`); `self_service` is a member request. */
export type ProposalSource = 'extraction' | 'chat' | 'self_service';

/**
 * `POST /api/admin/inbox/{id}/approve`. Requests need `version` (and, for
 * `change_fixed`, `choices`, `{}` when none are listed) and take no edit;
 * proposals take an optional `version` and, for a move, new run or split,
 * an edit: `day` (0–6) in the boss week of the proposed time and `time`
 * `HH:MM`. Conflicts always block; there is no `force`.
 */
export interface ApproveRequest {
  version?: number;
  choices?: Record<string, 'update' | 'keep'>;
  day?: number;
  time?: string;
}

/** `POST /api/admin/inbox/{id}/reject`: requests need `version` and a reason of 1–500 characters; proposals take no reason (422 `reason_not_applicable`). */
export interface RejectRequest {
  version?: number;
  reason?: string;
}

/** How a closed Inbox item ended: `approved` = merged, `superseded` = replaced by a newer proposal. */
export type PastOutcome = 'approved' | 'rejected' | 'superseded' | 'discarded' | 'withdrawn' | 'expired';

// ── Extractions and rescans ───────────────────────────────────────────────

/** `identity_leak`: pseudonymization's boundary scanner refused the request; nothing was sent. */
export type ExtractionOutcome =
  | 'proposed'
  | 'no_change'
  | 'failed'
  | 'turned_away'
  | 'content_blocked'
  | 'self_service_link'
  | 'identity_leak';

export interface ExtractionRow {
  /** Provider-reported reasoning tokens over reporting attempts; null = unknown. */
  reasoning_tokens?: number | null;
  id: string;
  short_id: string;
  at: string;
  model: string;
  latency_ms: number | null;
  messages: number;
  changes: number;
  channel: string | null;
  channel_id: string;
  error: string | null;
  outcome: ExtractionOutcome;
  /** Provider-reported tokens summed over the call's reporting attempts; null = not reported (never 0). */
  prompt_tokens?: number | null;
  completion_tokens?: number | null;
}

/**
 * Reported token usage over a set of logged requests: sums over those that
 * reported a pair (null when none did), how many did, and the median of
 * reported prompt tokens / local estimate (two decimals; null when none).
 */
export interface UsageSummary {
  prompt_tokens: number | null;
  completion_tokens: number | null;
  reported: number;
  est_ratio: number | null;
}

/** Per model over the listed (filtered) extraction calls. */
export interface ExtractionSummary extends UsageSummary {
  model: string;
  count: number;
}

/** What the log filters can offer (all values seen, not only the filtered rows'). */
export interface LogFacets {
  models: string[];
  tools: string[];
  outcomes: string[];
  channels: { id: string; name: string }[];
}

/**
 * Query params of `GET /api/admin/chat` and `/api/admin/extractions`, all
 * optional and combinable: `model`, `from`/`to` (guild-local YYYY-MM-DD),
 * `outcome` (comma-separated, any of), `channel`, `member`, `q`; Chat also
 * `tool` and `min_ms`. Unknown outcomes or malformed dates: 422 invalid_filter.
 */
export interface Extractions {
  model: string;
  /** Per model over the filtered calls (always sent by the server). */
  summary?: ExtractionSummary[];
  rows: ExtractionRow[];
  /** Rows before filtering. */
  total: number;
  facets: LogFacets;
}

export interface Extraction extends Omit<ExtractionRow, 'messages' | 'changes'> {
  /** Response-only text, capped at 64 KiB including a visible marker. */
  reasoning_content?: string | null;
  prompt: string;
  raw_response: string;
  amendments: { kind: string; bosses: string; when: string; confidence: number; status: string }[];
  messages: { id: string; author: string; author_id?: string; at: string; content: string }[];
  /** Local prompt estimate over the attempts that reported usage, else every sent attempt. */
  prompt_estimate?: number | null;
  /** The context the call was budgeted for; null when not logged. */
  context?: { window: number; reserve: number; source: string } | null;
}

export interface RescanJob {
  id: string;
  state: 'running' | 'done' | 'cancelled';
  window: 'week' | 'since_reset' | 'two_weeks';
  /** When the runner took the job (UTC `Z`); null while queued. */
  started_at?: string | null;
  /** Per channel, the gated messages its read found (0 until `done`). */
  channels: { id: string; name: string; state: 'queued' | 'reading' | 'done'; messages: number }[];
  /** The channels' `messages`: gated messages read so far. */
  messages?: number;
  /**
   * `messages` plus each unread channel's gated messages as cached when the job
   * started; equals `messages` once the job ends. Null while a channel still to
   * be read has no count. Progress = messages / messages_total.
   */
  messages_total?: number | null;
  proposals: number;
}

// ── Chat ──────────────────────────────────────────────────────────────────

export type ChatOutcome =
  | 'answered'
  | 'refused'
  | 'clarified'
  | 'error'
  | 'timeout'
  | 'rate_limited'
  | 'turned_away'
  | 'content_blocked'
  | 'withheld'
  | 'clean_retry';

export interface ChatRow {
  /** Sum of the rounds' reported reasoning counts; null = unknown. */
  reasoning_tokens?: number | null;
  id: string;
  at: string;
  member: Member;
  /** The full Discord id, even when `member.name` is a placeholder. */
  member_id?: string | null;
  channel: string | null;
  channel_id: string;
  /** The first round's alias ("—" when no model was called). */
  model: string;
  /** Model alias per request round. */
  models: string[];
  latency_ms: number;
  outcome: ChatOutcome;
  asked: string;
  tools_used: string[];
  /** Turn totals as logged (v4 imports may carry one); null = not reported. */
  prompt_tokens?: number | null;
  completion_tokens?: number | null;
}

/** Usage fields come from this model's round rows, never the turn totals. */
export interface ChatSummary extends Partial<UsageSummary> {
  model: string;
  count: number;
  answered: number;
  refused: number;
  errors: number;
  p50_ms: number;
  tool_calls: number;
}

export interface Chat {
  /** Per model, over the filtered rows. */
  summary: ChatSummary[];
  rows: ChatRow[];
  total: number;
  facets: LogFacets;
}

/** Where a round's member data went; null for rows recorded before routes were. */
export type ChatRoute = 'homelab' | 'external_masked' | 'external_unmasked';

export interface ChatToolCall {
  /** The request round (1-based index into `rounds`) whose reply asked for it. */
  round: number;
  name: string;
  arguments: string;
  result: string;
  /** Wall time; null when unknown (0 is a real 0 ms). */
  took_ms: number | null;
  outcome: string;
}

export interface ChatRoundFacts {
  reasoning_content?: string | null;
  reasoning_tokens?: number | null;
  round: number;
  requested_tools: string[];
  finish: string;
  /** Alias the request named, as sent. */
  model: string;
  /** Reasoning effort as sent (after capability shaping); null when none went out. */
  effort: string | null;
  route: ChatRoute | null;
  /** null when unknown. */
  latency_ms: number | null;
  /** Provider-reported usage for this request (both or neither); null = not reported. */
  prompt_tokens?: number | null;
  completion_tokens?: number | null;
  /** The context budget's estimate, completion reserve excluded. */
  prompt_estimate?: number | null;
  guardrail: { clean: boolean; content_filter: boolean };
}

/** A pseudonymized turn as the model saw it (admin only). */
export interface ModelView {
  rounds: {
    round: number;
    clean: boolean;
    /** The request messages exactly as sent (masked). */
    request: { role: 'system' | 'user' | 'assistant' | 'tool'; [key: string]: unknown }[];
    /** The model's reply before names were restored. */
    reply: string | null;
    /** Tool-call arguments before names were restored. */
    tool_calls: { name: string; arguments: string }[];
  }[];
  /** The decoded, finished reply members saw. */
  reply: string;
  /** Fake name → member display name; never user ids. */
  mapping: { token: string; name: string }[];
}

export interface ChatTurn extends ChatRow {
  said: string;
  tools: ChatToolCall[];
  rounds: ChatRoundFacts[];
  cards: { kind: string; url: string }[];
  raw: string;
  /** Persona bundle id; null when none answered (rate limited, imported). */
  persona: string | null;
  /** Reply profile id; null for the bundle default voice. */
  profile: string | null;
  profile_source: 'saved' | 'role' | 'default' | null;
  /** The turn's route (its last round's); null when no model ran. */
  route: ChatRoute | null;
  error: string | null;
  /** Stable code: timeout, malformed, content_blocked, identity_leak_blocked, rate_limited, … */
  error_code: string | null;
  /** content_filter, external_unmasked, pseudonymized, identity_leak_blocked {role, kinds, count}, … */
  guardrail: Record<string, unknown>;
  /** Pseudonymized, with a stored Model view. */
  masked: boolean;
  /** Null for passthrough and withheld turns. */
  model_view: ModelView | null;
}

// ── Limits (v5: model backends behind the Kanata gateway) ─────────────────

export interface BackendGroup {
  name: string;
  backend: string;
  models: string[];
  permits: { in_use: number; total: number };
  queue: { position: number; kind: string; who: string; waiting_s: number }[];
  rate: { available: number; capacity: number; refill_per_min: number };
  retry: { remaining: number; capacity: number };
  breaker: { state: 'closed' | 'half_open' | 'open'; failures: number; since: string; retry_at?: string };
}

export interface Refusal {
  /** `rate`, `concurrency`, `quota`, `key_rate`, `key_quota`, … */
  kind: string;
  scope: 'group' | 'key';
  target: string;
  count: number;
  last_at: string;
}

export interface Allowance {
  member: Member;
  staff: boolean;
  allowance: { count: number; per_s: number } | null;
  used: number;
  override: boolean;
}

export interface Limits {
  groups: BackendGroup[];
  admission: { window: string; refusals: Refusal[] };
  allowances: Allowance[];
}

// ── History (docs/v5/history.md, format kanade.change.v1) ─────────────────

export type ActorKind = 'member' | 'admin' | 'system';
export type Surface =
  | 'discord'
  | 'admin_portal'
  | 'public_portal'
  | 'cli'
  | 'chat_approval'
  | 'extraction_approval'
  | 'delivery_tick'
  | 'rollback'
  | 'import'
  | 'draft_merge'
  | 'request_merge'
  | 'cherry_pick';

export type RowKey = { table: 'runs' | 'fixed_runs' | 'reminders'; id: string } | { table: 'rsvps'; run_id: string; user_id: string };

export interface RowChange {
  key: RowKey;
  /** Full domain row; null = absent. */
  before: Record<string, unknown> | null;
  after: Record<string, unknown> | null;
}

export interface ChangeRecord {
  format: 'kanade.change.v1';
  seq: number;
  id: string;
  revision: number;
  at: string;
  actor: { kind: ActorKind; id: string };
  surface: Surface;
  request_id: string | null;
  /** Boss weeks the change touched, each the RFC 3339 instant it starts (UTC, e.g. `2026-09-23T16:00:00+00:00`). */
  weeks: string[];
  rows: RowChange[];
  notices: string[];
  /** Records this change refers to: the records a rollback undid. */
  refs: { seq: number; hash: string }[];
  prev_hash: string;
  hash: string;
}

/**
 * `GET /api/admin/history?week&actor&run&before&limit`, newest first. With
 * `run=<id>` (alone; not with `week` or `actor`): the run's change log, each
 * record changing its row or RSVPs; the run's before → after is in those
 * `rows` (`runs` keyed by `id`, `rsvps` by `run_id`).
 */
export interface HistoryPage {
  records: ChangeRecord[];
  head: { seq: number; hash: string };
  /** Pass as `before` for the next (older) page; null on the last page. */
  next_before: number | null;
  total: number;
}

export interface RevertPlan {
  outcome: 'preview' | 'applied' | 'unchanged' | 'conflicts';
  /** The requested records (a refused week or actor rollback: the conflicting ones). */
  reverts: number[];
  /** Empty when `outcome` is `conflicts`: a strict refusal plans nothing. */
  rows: RowChange[];
  conflicts: { seq: number; key: RowKey; expected: unknown; found: unknown }[];
  skipped: { key: RowKey; reason: string }[];
  record: ChangeRecord | null;
}

/** Common to the three rollback requests. */
export interface RollbackMode {
  force?: boolean;
  preview?: boolean;
  request_id?: string;
}

/** `matches`: the chain holds the backup's head; `older_schema`: it does, but the backup predates the store's schema; `mismatch`: the head is not in the chain (truncated or forked history). */
export type BackupAnchor = 'matches' | 'older_schema' | 'mismatch';

export interface Checkpoints {
  verified: { ok: boolean; checked: number; head: { seq: number; hash: string } };
  /** `KANADE_BACKUP_DIR` is set: false means no directory, not no backups. */
  backup_dir_configured: boolean;
  /** Newest first (at most 100); re-read and re-checked on every request. */
  backups: {
    file: string;
    format: 'kanade.backup.v1';
    created_at: string;
    history_head: { seq: number; hash: string };
    revision: number;
    schema_version: number;
    /** The chain still contains `history_head`. */
    anchored: boolean;
    anchor: BackupAnchor;
  }[];
}

/** `GET /api/admin/runs/{id}/blame`: who last changed each field some record set. */
export interface BlameEntry {
  /** `slot`, `bosses`, `participants`, `channel`, `status`, `status_pin`, `rsvp:<member id>`, `attended:<member id>`. */
  field: string;
  /** Current value: `slot` is `{datetime, week_start, source, fixed_run_id}`, `channel` the channel id, `rsvp:`/`attended:` the row or entry (null once cleared). */
  value: unknown;
  seq: number;
  at: string;
  actor: { kind: ActorKind; id: string };
  surface: Surface;
}

// ── Config ────────────────────────────────────────────────────────────────

export type ModelRole = 'extraction' | 'chat' | 'rewrite';
export type SelfServiceMode = 'cards_and_link' | 'link_first' | 'cards_only';

/** Where a role's context window came from, before clamps. */
export type ContextSource = 'override' | 'catalog' | 'cloud_default' | 'local_default';

export interface RoleProfile {
  role_id: string;
  role_name: string;
  profile: string;
}

/** Writable fields for an ordered role-profile assignment; names are display-only. */
export type RoleProfileWrite = Pick<RoleProfileView, 'role_id' | 'profile'>;
