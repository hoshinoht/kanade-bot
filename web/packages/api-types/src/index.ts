// Hand-written for the mock server's JSON (tools/pwa-mock). Production types will be
// generated from the Rust API models (ts-rs or specta) and replace this file;
// keep field names snake_case to match serde defaults.

export type RunStatus = 'planned' | 'confirmed' | 'at_risk' | 'otot' | 'done' | 'cancelled';
export type Answer = 'yes' | 'no' | 'maybe' | 'waiting';
export type Difficulty = 'e' | 'n' | 'h' | 'c' | 'x';

export interface Boss {
  /** Canonical token shown on compact cards, e.g. `HCarling`. */
  token: string;
  /** Catalog key (`Carling`, `MaleficStar`), used for art and knowledge links. */
  key: string;
  name: string;
  difficulty: Difficulty;
  level: number | null;
  /**
   * Same-origin art URLs, or null when the deployment has no file: absent
   * means absent (no element, no broken image). Art is deployment-private and
   * never committed; tests use synthetic fixtures.
   */
  portrait: string | null;
  /** The small (64 px) portrait render, falling back to `portrait`. */
  portrait_sm: string | null;
  /** Entry-screen artwork shown under the run-card veil. */
  art: string | null;
  /** Monogram hue (0-359) for the portrait stand-in. */
  hue: number;
}

export interface Tally {
  /** Participants who answered yes. */
  on: number;
  total: number;
}

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

export interface WeekDay {
  index: number;
  /** ISO local date `YYYY-MM-DD`. */
  date: string;
  dow: string;
  is_reset: boolean;
  is_today: boolean;
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

export interface Participant {
  /** Stable member id (a Discord user id in production); display names can repeat. */
  id: string;
  name: string;
  answer: Answer;
}

export type CardKind = 'morning' | 'T-1h' | 'T-15m';
export type CardState = 'posted' | 'queued' | 'skipped';

/** One reminder card the bot posts about a run, in the order it posts them. */
export interface ReminderCard {
  label: CardKind;
  state: CardState;
  /** Local `HH:MM`: when it posted, or when it is due. */
  at: string;
  /** Discord message link once posted. */
  url: string | null;
}

/** Admin run (`GET /api/admin/week`): the public projection plus who and where. */
export interface Run extends PublicRun {
  /** Short id shown as `#630b3544`; secondary identity for audit/troubleshooting. */
  short_id: string;
  participants: Participant[];
  party: string;
  /** The channel id a re-read targets (the explicit key; `party` is the legacy handle). */
  channel_id: string;
  /** Channel display name, e.g. `#hstar-party`. */
  channel: string;
  cards: ReminderCard[];
  /** The weekly timing this run was materialised from, if any. */
  fixed_id: string | null;
  /** Day, time or roster differs from that timing. */
  amended: boolean;
  /** v4 "this week: −A +B" against the timing's roster. */
  roster_change: { out: Member[]; in: Member[] } | null;
}

export interface Week extends Omit<PublicWeek, 'runs'> {
  runs: Run[];
  /** Optimistic-concurrency token for moves. */
  version: number;
}

/** Either projection; enough for shared presentation components. */
export type WeekShape = Pick<PublicWeek, 'days' | 'timezone'>;

export interface MoveRequest {
  day: number;
  time: string | null;
  /** Optimistic concurrency: the week version the client moved from. */
  version: number;
}

export type WeekKey = 'this' | 'next';

export interface Member {
  id: string;
  name: string;
}

export type PingLevel = 'essential' | 'all' | 'off';

/** `GET /api/admin/members`: the bossing roster plus members with chatbot access. */
export interface MemberRow extends Member {
  nickname: string | null;
  /** What the extractor matches names against in chat. */
  aliases: string[];
  runs_this_week: number;
  ping_level: PingLevel;
  /** Chosen reply style (persona profile key); null = default. */
  persona: string | null;
  /** False when the chosen style is no longer offered. */
  persona_available: boolean;
  /** Holds the bossing role (on the roster). */
  bossing: boolean;
  /** Chatbot access from Discord roles. */
  access: 'staff' | 'pilot' | 'none';
}

export interface Persona {
  key: string;
  name: string;
}

export interface MemberPatch {
  ping_level?: PingLevel;
  /** Empty string clears back to the default style. */
  persona?: string;
}

/** `GET /api/admin/fixed`: weekly timings (v4 fixed_rows). */
export interface FixedRow {
  id: string;
  short_id: string;
  /** 0 = Monday. */
  weekday: number;
  weekday_name: string;
  time: string;
  bosses: Boss[];
  participants: Member[];
  channel_id: string;
  channel_name: string;
  channel_watched: boolean;
  owner: string;
  note: string | null;
  /** Live runs materialised from it, this week and next. */
  runs: { run_id: string; short_id: string; week: WeekKey; day: number; time: string | null; status: RunStatus; amended: boolean }[];
}

export interface FixedRequest {
  weekday: number;
  time: string;
  /** Boss text as v4 parsed it: `hstar, hfa` or tokens `HStar HFA`. */
  bosses: string;
  participants: string[];
  channel_id: string;
  note: string | null;
  /** For each amended run of the timing: follow the new timing, or keep this week's change. */
  decisions?: Record<string, 'update' | 'keep'>;
  /** Week version the form was loaded at: required by PATCH (else 422 `version_required`), ignored by POST. */
  version?: number;
  /** admin-api "Edit preconditions": replace the version-derived expectations. */
  expect?: { field: string; seen: number | null }[];
  override?: { seq: number; hash: string }[];
}

export interface ValidateResult {
  bosses: Boss[];
}

export interface DifficultyOption {
  letter: Difficulty;
  name: string;
  token: string;
  /** A live weekly timing runs it. */
  in_use: boolean;
}

/** `GET /api/admin/bosses`: the in-game list in level order. */
export interface BossRow {
  key: string;
  name: string;
  level: number;
  hue: number;
  portrait: string | null;
  difficulties: DifficultyOption[];
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
  sources: KnowledgeSource[];
}

/** `GET /api/admin/bosses/{key}/knowledge`. */
export interface Knowledge {
  key: string;
  name: string;
  /** null for event bosses outside the catalog. */
  level: number | null;
  portrait: string | null;
  hue: number;
  researched_as_of: string | null;
  path: string;
  /** Difficulty letters a live weekly timing runs; the page opens on the first. */
  in_use: Difficulty[];
  doc: KnowledgeDoc;
}

/** `GET /api/admin/bosses/events`: knowledge documents that declare an `event`. */
export interface EventBoss {
  key: string;
  event: { name: string; availability: string };
  summary: string;
}

/** A card the bot will post or posted: `queued` later, `due` now, `sent`, `stale` (retired unposted). */
export interface ReminderRow {
  id: string;
  run_id: string;
  run_short_id: string;
  kind: CardKind;
  state: 'queued' | 'due' | 'sent' | 'stale';
  at: string;
  bosses: Boss[];
  party: string[];
  url: string | null;
}

export interface Reminders {
  upcoming: ReminderRow[];
  sent: ReminderRow[];
}

export interface Channel {
  id: string;
  name: string;
}

/** `GET /api/admin/summary`: the four "right now" tiles. */
export interface Summary {
  next: { run_id: string; bosses: string; when: string; countdown: string; on: number; total: number } | null;
  unanswered: number;
  inbox: number;
  model: { busy: boolean; holder: string | null };
}

/** `GET /api/identity`: the bot's cached Discord identity, or synthetic art. */
export interface Identity {
  name: string;
  avatar: string;
  banner: string;
  /** False when the server fell back to generated art (nothing cached). */
  cached: boolean;
}

/** `GET /api/admin/session`: who is signed in; the `X-Kanade-CSRF` response header carries the write token. */
export interface Session {
  display: string;
  /** How it signed in; only `discord` sessions may approve or reject Kanade's proposals. Always sent by the server. */
  method?: SignInMethod;
}

export type SignInMethod = 'discord' | 'tailscale' | 'token';

/** `GET /api/admin/auth/methods`: the sign-in methods this server offers this browser. */
export interface SignInMethods {
  discord: boolean;
  /** This request carries an allow-listed identity from the tailnet edge. */
  tailscale: boolean;
  token: boolean;
}

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

export interface RunResult {
  run: Run;
  version: number;
}

export interface PingResult {
  message: string;
}

export interface MoveResult {
  run: Run;
  previous: { day: number; time: string | null };
  version: number;
}

export interface DayStat {
  day: number;
  answered: number;
  waiting: number;
}

export interface Stats {
  per_day: DayStat[];
}

export interface ApiError {
  error: string;
  message: string;
}

/** `GET /api/public/status`: the closed page needs nothing else to render. */
export interface PublicStatus {
  portal: 'open' | 'closed';
}

// ── Inbox ─────────────────────────────────────────────────────────────────

export interface Evidence {
  id: string;
  author: string;
  at: string;
  content: string | null;
  url: string | null;
  /** The message is no longer stored. */
  missing: boolean;
}

export type InboxTab = 'extractor' | 'self_service';
/** Badges an inbox item can carry; each also blocks or qualifies an action. */
export type ProposalFlag = 'conflict' | 'expired' | 'requester_frozen' | 'requester_unauthorised' | 'no_effect';

export interface ProposalPreview {
  /** Applying it would change nothing (already in effect): approve is refused with 409 no_effect. */
  no_effect: boolean;
  changes: { field: string; from: string; to: string }[];
  /** Three-way conflicts (what the change was based on vs now): approving is refused (409 conflicts); reject instead. */
  conflicts: { field: string; expected: string; found: string }[];
}

/** A `change_fixed` request's amended run the edit would move: it needs `update` or `keep`; other runs follow the timing. */
export interface ProposalChoice {
  run_id: string;
  label: string;
  when: string;
  amended: boolean;
}

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

/** `GET /api/admin/inbox`: changes waiting for an admin (v4 amendments, plus v5 self-service requests). */
export interface Proposal {
  id: string;
  short_id: string;
  /** A proposal's change kind, or a member request's type (`new_fixed` … `swap`). */
  kind: 'move' | 'add' | 'cancel' | 'split' | 'otot' | 'sub' | 'rsvp' | 'fix' | 'new_fixed' | 'change_fixed' | 'join' | 'leave' | 'swap';
  kind_label: string;
  /** Kanade read it from party chat (`extraction`) or was asked in chat (`chat`); `self_service` is a member request. */
  source: 'extraction' | 'chat' | 'self_service';
  tab: InboxTab;
  /** Name it on approve/reject; a different current version answers 409 stale. */
  version: number;
  flags: ProposalFlag[];
  preview: ProposalPreview;
  expires_at: string | null;
  choices: ProposalChoice[] | null;
  /** The generated one-line summary the member sees in the public app. */
  public_summary: string | null;
  bosses: Boss[];
  run_id: string | null;
  from_when: string | null;
  when: string;
  participants: Member[];
  confidence: number | null;
  is_question: boolean;
  channel: string | null;
  read_at: string;
  summary: string;
  evidence: Evidence[];
  card_url: string | null;
  /** Member requests: who asked, `via` (`request`) and their own title (admin-only). */
  self_service: { member: Member; via: string; note: string | null } | null;
}

// ── Extractions and rescans ───────────────────────────────────────────────

export type ExtractionOutcome = 'proposed' | 'no_change' | 'failed' | 'turned_away' | 'content_blocked' | 'self_service_link';

export interface ExtractionRow {
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
  rows: ExtractionRow[];
  /** Rows before filtering. */
  total: number;
  facets: LogFacets;
}

export interface Extraction extends Omit<ExtractionRow, 'messages' | 'changes'> {
  prompt: string;
  raw_response: string;
  amendments: { kind: string; bosses: string; when: string; confidence: number; status: string }[];
  messages: { id: string; author: string; at: string; content: string }[];
}

export interface RescanJob {
  id: string;
  state: 'running' | 'done' | 'cancelled';
  window: 'week' | 'since_reset' | 'two_weeks';
  channels: { id: string; name: string; state: 'queued' | 'reading' | 'done'; messages: number }[];
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
  id: string;
  at: string;
  member: Member;
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
}

export interface ChatSummary {
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

export interface ChatTurn extends ChatRow {
  said: string;
  tools: { name: string; arguments: string; result: string; took_ms: number; outcome: string }[];
  rounds: { round: number; requested_tools: string[]; finish: string }[];
  cards: { kind: string; url: string }[];
  raw: string;
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

export interface Checkpoints {
  verified: { ok: boolean; checked: number; head: { seq: number; hash: string } };
  backups: {
    file: string;
    format: 'kanade.backup.v1';
    created_at: string;
    history_head: { seq: number; hash: string };
    revision: number;
    schema_version: number;
    anchored: boolean;
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

/** One alias from Kanata's live model list, with its published capabilities. */
export interface ModelInfo {
  id: string;
  trust_zone: 'homelab' | 'external' | 'unknown';
  /**
   * Requests to it leave the homelab. Derived failing closed: external trust,
   * an unknown zone, or the `-cloud` alias suffix (the Ollama cloud proxy
   * reports `local`, so the suffix is what marks it).
   */
  leaves_homelab: boolean;
  function_tools: boolean;
  structured_output: boolean;
  sampling_controls: boolean;
  reasoning_control: boolean;
  /**
   * The efforts the model publishes; the reasoning picker offers only these
   * (plus `off`). Null = the model decides (offered as low/medium/high, as v4
   * did); empty = no reasoning control.
   */
  reasoning_efforts: string[] | null;
  /** Admission Kanata publishes for the alias, or null when the operator declares it. */
  admission: { max_in_flight: number; adapter_max_in_flight?: number } | null;
}

export interface RoleModel {
  alias: string;
  /** `off`, a published effort, or `""` = same as extraction (chat and rewrite only). */
  reasoning: string;
}

export interface CapacityGroup {
  model: string;
  group: string;
  /** A cleared number input arrives as null and is refused naming its row. */
  permits: number | null;
}

/** Per-alias admission: Kanata's published route/adapter caps, or the operator's declaration. */
export interface AliasLimit {
  alias: string;
  max_in_flight: number;
  adapter_max_in_flight?: number;
  source: 'published' | 'declared';
}

/** Key-level admission. The key is shared with the owner's other clients. */
export interface KeyLimits {
  max_in_flight: number;
  shared: boolean;
}

/** The startup capacity check, run on every save: an `error` would stop the bot, so it is refused. */
export interface CapacityCheck {
  level: 'ok' | 'warning' | 'error';
  message: string;
}

export interface RoleProfile {
  role_id: string;
  role_name: string;
  profile: string;
}

/** A reply profile file: shown read-only (text is edited as files only). */
export interface ReplyProfile {
  key: string;
  name: string;
  public: boolean;
  voice: string;
  prompt_summary: string;
}

/**
 * `GET /api/admin/config`: runtime settings. `PATCH` takes one section per
 * request with only its writable fields (the admin app's `ConfigPatch`);
 * read-only or unknown keys are refused with 422.
 */
export interface ConfigView {
  pings: { day_of_ping_time: string; countdown_minutes: number[] };
  watching: { paused: boolean; extract_enabled: boolean };
  chatbot: {
    enabled: boolean;
    /** False until CHAT_PILOT_ROLE_ID and CHAT_PILOT_CHANNEL_IDS are both set; the toggle is then disabled. */
    configured: boolean;
    missing_env: string[];
    member_rate: { count: number; window_s: number };
    guild_rate: { count: number; window_s: number };
  };
  notifications: { quiet_mode: boolean };
  /** With the public portal off, members can only answer on cards: `effective_mode` is `cards_only`. */
  self_service: { mode: SelfServiceMode; effective_mode: SelfServiceMode; public_portal: boolean };
  persona: {
    active: string;
    personas: { key: string; name: string; bundle: string }[];
    profiles: ReplyProfile[];
    /** Order is significant: the first matching role wins. Saved and applied whole. */
    role_profiles: RoleProfile[];
  };
  models: {
    reachable: boolean;
    catalog: ModelInfo[];
    roles: Record<ModelRole, RoleModel>;
    groups: CapacityGroup[];
    alias_limits: AliasLimit[];
    key_limits: KeyLimits;
    capacity_check: CapacityCheck[];
    /** Env-only (read-only here): whether names are pseudonymised before leaving the homelab. */
    pii_pseudonymise: boolean;
  };
  /** Channels where the bot lacks Manage Messages; shown above every Config section. */
  manage_messages: { missing: string[] };
  /** Present on PATCH responses that reset a stranded reasoning level. */
  notices?: string[];
  /** Settings only the deployment can change, each with the reason. */
  env: { key: string; label: string; value: string; reason: string }[];
}

/** `GET /api/admin/access`: the bot's role permissions per channel (v4 access.html). */
export interface AccessReport {
  connected: boolean;
  checked_at: string;
  rows: {
    id: string;
    name: string;
    watched: boolean;
    digest: boolean;
    view: boolean;
    send: boolean;
    history: boolean;
    embed: boolean;
    react: boolean;
    manage_messages: boolean;
  }[];
}
