// Hand-written API types with no typed Rust counterpart yet: string vocabularies
// the generated types name, request bodies, the mock-only public week, the
// YAML knowledge document, and the canonical history record (its JSON is the
// hashed encoding). Everything else is in `generated.ts`.

import type { Boss, Member, RoleProfileView, RowChange, Tally, WeekDay } from './generated';

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

/** Either projection; enough for shared presentation components. */
export type WeekShape = Pick<PublicWeek, 'days' | 'timezone'>;

export interface MoveRequest {
  day: number;
  time: string | null;
  /** Optimistic concurrency: the week version the client moved from. */
  version: number;
}

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

/** A proposal's change kind, or a member request's type (`new_fixed` … `swap`). */
export type ProposalKind = 'move' | 'add' | 'cancel' | 'split' | 'otot' | 'sub' | 'rsvp' | 'fix' | 'new_fixed' | 'change_fixed' | 'join' | 'leave' | 'swap';

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

/** Where a round's member data went; null for rows recorded before routes were. */
export type ChatRoute = 'homelab' | 'external_masked' | 'external_unmasked';

// ── Limits (v5: model backends behind the Kanata gateway) ─────────────────

export interface Refusal {
  /** `rate`, `concurrency`, `quota`, `key_rate`, `key_quota`, … */
  kind: string;
  scope: 'group' | 'key';
  target: string;
  count: number;
  last_at: string;
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

/** Common to the three rollback requests. */
export interface RollbackMode {
  force?: boolean;
  preview?: boolean;
  request_id?: string;
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
