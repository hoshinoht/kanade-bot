import type { CapacityGroup, ConfigView, ContextSettings, ModelRole, RoleProfileWrite, SelfServiceMode } from '@kanade/api-types';

type Rate = { count?: number; window_s?: number };

/**
 * `PATCH /api/admin/config`: one section per request, each partial. Arrays are
 * replaced whole except persona visibility, which merges only the listed keys.
 * Read-only derivations and unknown keys are refused with 422.
 */
export interface ConfigPatch {
  pings?: { day_of_ping_time?: string; countdown_minutes?: number[] };
  watching?: { paused?: boolean; extract_enabled?: boolean };
  chatbot?: { enabled?: boolean; member_rate?: Rate; guild_rate?: Rate };
  persona?: {
    active?: string;
    role_profiles?: RoleProfileWrite[];
    role_profiles_digest?: string;
    visibility?: { key: string; public: boolean }[];
  };
  /** `context` is always the complete object; a partial one is refused. */
  models?: { roles?: Partial<Record<ModelRole, { alias?: string; reasoning?: string }>>; groups?: CapacityGroup[]; context?: ContextSettings };
  self_service?: { mode?: SelfServiceMode; public_portal?: boolean };
  notifications?: { quiet_mode?: boolean };
  /** Both keys optional; `overrides` replaces the list whole. */
  run_lengths?: Partial<ConfigView['run_lengths']>;
}

export type Save = (patch: ConfigPatch, done: string) => Promise<string>;

export type RoleProfileSave =
  | { ok: true; value: ConfigView }
  | { ok: false; message: string; status?: number | null; code?: string | null };

export type SaveRoleProfiles = (assignments: RoleProfileWrite[], digest: string) => Promise<RoleProfileSave>;

export type { ConfigView };
