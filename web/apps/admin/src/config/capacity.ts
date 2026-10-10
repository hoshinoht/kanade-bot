import type { CapacityCheck, ConfigView, ModelInfo, ModelRole, RoleModel } from '@kanade/api-types';
import { wavingGroups } from '../limits/permits';

export const ROLES: { id: ModelRole; name: string; job: string }[] = [
  { id: 'extraction', name: 'Extraction', job: 'Reads the party channels and proposes schedule changes.' },
  { id: 'chat', name: 'Chat', job: 'Answers members; it calls tools, so it needs a model that can.' },
  { id: 'rewrite', name: 'Rewrite', job: 'A small local model that nudges replies into the persona’s voice.' },
];

/** The effort an inheriting role resolves to: the extraction role's level. */
export function effectiveReasoning(reasoning: string, extractionEffort: string): string {
  return reasoning === '' ? extractionEffort : reasoning;
}

/** Every stored reasoning level after `off`, in the server's order. */
export const ALL_EFFORTS = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'];

/**
 * Whether an effort is legal for an alias: `off` always is; `null` efforts
 * mean Kanata restricts nothing, so every level is (admin-api "Config semantics").
 */
export function isReasoningValid(model: ModelInfo | undefined, effort: string): boolean {
  if (effort === 'off') return true;
  if (!model) return false;
  if (model.reasoning_efforts === null) return ALL_EFFORTS.includes(effort);
  return model.reasoning_efforts.includes(effort);
}

/**
 * The reasoning levels a role may pick. Inherit is offered only when the
 * extraction role's current effort is `off` or in the role model's published
 * list (every level when it publishes none); otherwise the resolved
 * effort would be illegal, so the role must choose explicitly.
 */
export function reasoningChoices(
  role: ModelRole,
  model: ModelInfo | undefined,
  extractionEffort: string,
  current?: string,
): { value: string; label: string }[] {
  const published = model?.reasoning_efforts;
  const levels = published === undefined ? [] : (published ?? ALL_EFFORTS);
  // A model that requires reasoning never offers `off` (the server would refuse it).
  const off = model?.off_allowed === false ? [] : [{ value: 'off', label: 'Off' }];
  const choices = [...off, ...levels.map((e) => ({ value: e, label: e[0]!.toUpperCase() + e.slice(1) }))];
  if (role === 'extraction') return choices;
  const values = new Map(choices.map((c) => [c.value, c.label] as const));
  // A saved value the catalog no longer lists stays selectable, marked, so a
  // fail-closed admin sees what is stored rather than a blank box.
  if (current && !values.has(current)) choices.push({ value: current, label: `${current} (not listed)` });
  const inherit =
    extractionEffort === 'off' || (model !== undefined && (published === null ? ALL_EFFORTS : (published ?? [])).includes(extractionEffort));
  if (inherit) return [{ value: '', label: 'Same as extraction' }, ...choices];
  // Never a blank box: a stored inherit that no longer fits stays visible, marked.
  if (current === '') return [{ value: '', label: `Same as extraction (${extractionEffort}, not published)` }, ...choices];
  return choices;
}

/**
 * Inheriting roles whose resolved effort is illegal for their alias once
 * extraction's effort (or an alias) changed: they switch to `off`, as the
 * server would otherwise refuse the save. Returns the reset roles, named.
 */
export function resetStrandedInheritors(roles: Record<ModelRole, RoleModel>, catalog: ModelInfo[]): ModelRole[] {
  const reset: ModelRole[] = [];
  for (const role of ['chat', 'rewrite'] as const) {
    if (roles[role].reasoning !== '') continue;
    const model = catalog.find((m) => m.id === roles[role].alias);
    if (!isReasoningValid(model, roles.extraction.reasoning)) {
      roles[role].reasoning = 'off';
      reset.push(role);
    }
  }
  return reset;
}

// ── Model picker and capacity summary (read-only groups, base models only) ──

/** A listed `<base>:<level>` variant's base, or the alias itself. */
export function baseOf(catalog: ModelInfo[], alias: string): string {
  return catalog.find((m) => m.id === alias)?.variant_of ?? alias;
}

export interface PickerOption {
  value: string;
  label: string;
  disabled?: boolean;
}

/**
 * Base models only: a `model:level` variant is offered only when it is what
 * the role already stores, shown as "<base> (fixed: <level>)". An unset role
 * reads "Not configured"; chat needs a model that calls tools.
 */
export function modelOptions(role: ModelRole, stored: RoleModel, catalog: ModelInfo[]): PickerOption[] {
  const out: PickerOption[] = [];
  if (!stored.alias) out.push({ value: '', label: 'Not configured' });
  else if (stored.variant_of) out.push({ value: stored.alias, label: `${stored.variant_of} (fixed: ${stored.fixed_effort ?? '?'})` });
  else if (!catalog.some((m) => m.id === stored.alias)) out.push({ value: stored.alias, label: `${stored.alias} (not listed)` });
  for (const m of catalog) {
    if (m.variant_of) continue;
    const noTools = role === 'chat' && !m.function_tools;
    out.push({ value: m.id, label: noTools ? `${m.id} (no tools)` : m.id, disabled: noTools });
  }
  return out;
}

export interface GroupRow {
  group: string;
  permits: number | null;
  /** Permits held by calls in flight; null when the server has no governor. */
  inUse: number | null;
  /** Base aliases, each once. */
  models: string[];
}

/** One row per group; variants fold into their base. */
export function groupRows(models: ConfigView['models']): GroupRow[] {
  const rows: GroupRow[] = [];
  for (const g of models.groups) {
    let row = rows.find((r) => r.group === g.group);
    if (!row) rows.push((row = { group: g.group, permits: g.permits, inUse: g.in_use, models: [] }));
    const base = baseOf(models.catalog, g.model);
    if (!row.models.includes(base)) row.models.push(base);
  }
  return rows;
}

/** Rows whose bar waves: calls in flight, the fullest first, at most two (as on Limits). */
export function wavingRows(rows: readonly GroupRow[]): Set<string> {
  return wavingGroups(
    rows.flatMap((r) => (r.permits === null || r.inUse === null ? [] : [{ name: r.group, permits: { in_use: r.inUse, total: r.permits } }])),
  );
}

/**
 * The least Kanata admits for any of a group's models (each alias's
 * `max_in_flight`, held to its adapter's), as the startup check counts it;
 * null when none of them publishes or declares a limit.
 */
export function groupCap(models: ConfigView['models'], group: string): number | null {
  let cap: number | null = null;
  for (const g of models.groups) {
    if (g.group !== group) continue;
    const limit = models.alias_limits.find((l) => l.alias === g.model);
    if (!limit) continue;
    const each = Math.min(limit.max_in_flight, limit.adapter_max_in_flight ?? Infinity);
    if (cap === null || each < cap) cap = each;
  }
  return cap;
}

export interface SplitChecks {
  /** Each group row's own checks, keyed by group name. */
  byGroup: Map<string, CapacityCheck[]>;
  /** Checks spanning groups (no `group`), or naming a group no row shows. */
  rest: CapacityCheck[];
}

/** The server's startup checks placed on their group rows; each verdict once. */
export function splitChecks(checks: CapacityCheck[], groups: string[]): SplitChecks {
  const byGroup = new Map<string, CapacityCheck[]>();
  const rest: CapacityCheck[] = [];
  const seen = new Set<string>();
  for (const check of checks) {
    const key = `${check.group ?? ''}\n${check.level}\n${check.message}`;
    if (seen.has(key)) continue;
    seen.add(key);
    if (check.group && groups.includes(check.group)) {
      const own = byGroup.get(check.group) ?? [];
      own.push(check);
      byGroup.set(check.group, own);
    } else rest.push(check);
  }
  return { byGroup, rest };
}

/** A check in its group's row: the row already names the group, so "Group local: …" reads "…". */
export function rowCheckText(message: string, group: string): string {
  for (const lead of [`Group ${group}: `, `Group ${group} `]) {
    if (message.startsWith(lead)) {
      const rest = message.slice(lead.length);
      return rest.charAt(0).toUpperCase() + rest.slice(1);
    }
  }
  return message;
}

export interface KanataLimits {
  /** Every base model admits the same number of calls. */
  uniform: number | null;
  /** Base models the roles or groups use, with what Kanata admits. */
  inUse: { alias: string; max: number; declared: boolean }[];
  /** Every base model Kanata lists with a limit. */
  all: { alias: string; max: number; declared: boolean }[];
}

/** What Kanata admits per base model; `model:level` variants are never listed. */
export function kanataLimits(models: ConfigView['models']): KanataLimits {
  const variant = (alias: string) => alias.includes(':') || models.catalog.some((m) => m.id === alias && m.variant_of);
  const all = models.alias_limits
    .filter((l) => !variant(l.alias))
    .map((l) => ({ alias: l.alias, max: l.max_in_flight, declared: l.source === 'declared' }));
  const used = new Set([
    ...ROLES.map((r) => baseOf(models.catalog, models.roles[r.id].alias)).filter(Boolean),
    ...models.groups.map((g) => baseOf(models.catalog, g.model)),
  ]);
  const maxes = new Set(all.map((l) => l.max));
  return { uniform: all.length && maxes.size === 1 ? all[0]!.max : null, inUse: all.filter((l) => used.has(l.alias)), all };
}

/** The key sentence, once: Kanata publishes no per-key limit today. */
export function keyLine(key: ConfigView['models']['key_limits']): string {
  const shared = key.shared ? "; it is shared with the owner's other clients" : '';
  return key.max_in_flight === null
    ? `Kanata publishes no limit for this key${shared}.`
    : `Kanata admits ${key.max_in_flight} calls at a time for this key${shared}.`;
}

