import type { CapacityGroup, ConfigView, ModelRole, RoleProfile, SelfServiceMode } from '@kanade/api-types';

type Rate = { count?: number; window_s?: number };

/**
 * `PATCH /api/admin/config`: one section per request, each partial; arrays
 * are replaced whole. Only these writable fields; anything else (read-only
 * derivations like `effective_mode` or unknown keys) is refused with 422.
 */
export interface ConfigPatch {
  pings?: { day_of_ping_time?: string; countdown_minutes?: number[] };
  watching?: { paused?: boolean; extract_enabled?: boolean };
  chatbot?: { enabled?: boolean; member_rate?: Rate; guild_rate?: Rate };
  persona?: { active?: string; role_profiles?: RoleProfile[]; visibility?: { key: string; public: boolean }[] };
  models?: { roles?: Partial<Record<ModelRole, { alias?: string; reasoning?: string }>>; groups?: CapacityGroup[] };
  self_service?: { mode?: SelfServiceMode; public_portal?: boolean };
  notifications?: { quiet_mode?: boolean };
}

export type Save = (patch: ConfigPatch, done: string) => Promise<string>;

export type { ConfigView };
