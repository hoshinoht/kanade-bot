import type {
  AliasLimit,
  CapacityCheck,
  CapacityGroup,
  KeyLimits,
  ModelInfo,
  ModelRole,
  RoleModel,
} from '@kanade/api-types';

export const ROLES: { id: ModelRole; name: string; job: string }[] = [
  { id: 'extraction', name: 'Extraction', job: 'Reads the party channels and proposes schedule changes.' },
  { id: 'chat', name: 'Chat', job: 'Answers members; it calls tools, so it needs a model that can.' },
  { id: 'rewrite', name: 'Rewrite', job: 'A small local model that nudges replies into the persona’s voice.' },
];

/** One row's permit ceiling; the server refuses more (422), never clamps. */
export const MAX_PERMITS = 64;

export interface CapacityInputs {
  groups: CapacityGroup[];
  roles: Record<ModelRole, RoleModel>;
  catalog: ModelInfo[];
  aliasLimits: AliasLimit[];
  keyLimits?: KeyLimits;
}

const known = (catalog: ModelInfo[], alias: string) => catalog.some((m) => m.id === alias);
const capOf = (aliasLimits: AliasLimit[], alias: string): { max: number; adapter?: number } | undefined => {
  const found = aliasLimits.find((l) => l.alias === alias);
  // A server may send an absent adapter cap as null; either way it caps nothing.
  return found ? { max: found.max_in_flight, adapter: found.adapter_max_in_flight ?? undefined } : undefined;
};

/**
 * A live preview of the startup capacity check while groups are edited,
 * mirroring the server's rule: per-alias admission caps each group, the key
 * cap bounds the sum. The server runs the same check on save and is
 * authoritative. Messages name their row so identical rows stay distinct
 * (Svelte's keyed each would otherwise throw on duplicates).
 */
export function capacityCheck({ groups, roles, catalog, aliasLimits, keyLimits }: CapacityInputs): CapacityCheck[] {
  const out: CapacityCheck[] = [];
  groups.forEach((g, i) => {
    const row = i + 1;
    if (!g.group) out.push({ level: 'error', message: `Row ${row}: every row needs a group name.` });
    if (!known(catalog, g.model)) out.push({ level: 'error', message: `Row ${row}: Kanata does not list ${g.model}.` });
    if (g.permits === null || !Number.isInteger(g.permits))
      out.push({ level: 'error', message: `Row ${row}: permits must be a whole number.` });
    else if (g.permits <= 0)
      out.push({ level: 'error', message: `Row ${row}: ${g.model} in group ${g.group} declares ${g.permits} permits.` });
    else if (g.permits > MAX_PERMITS)
      out.push({ level: 'error', message: `Row ${row}: permits are at most ${MAX_PERMITS}.` });
  });
  // An alias belongs to exactly one group, listed once.
  const seen = new Map<string, { group: string; row: number }>();
  groups.forEach((g, i) => {
    const row = i + 1;
    const prior = seen.get(g.model);
    if (prior) {
      if (prior.group === g.group)
        out.push({ level: 'error', message: `Rows ${prior.row} and ${row}: ${g.model} is in group ${g.group} twice; list it once.` });
      else out.push({ level: 'error', message: `${g.model} is in groups ${prior.group} and ${g.group}; an alias belongs to exactly one group.` });
    } else seen.set(g.model, { group: g.group, row });
  });
  // A role's model with no group only warns: it runs with no permits.
  for (const role of ROLES) {
    const alias = roles[role.id].alias;
    if (!groups.some((g) => g.model === alias))
      out.push({ level: 'warning', message: `The ${role.id} model ${alias} is in no capacity group, so it runs with no permits.` });
  }
  for (const name of [...new Set(groups.map((g) => g.group))].sort()) {
    const rows = groups.filter((g) => g.group === name);
    const total = rows.reduce((n, g) => n + (Number.isInteger(g.permits) ? (g.permits ?? 0) : 0), 0);
    let cap: number | undefined;
    let cappedBy = '';
    let unlimited: string | null = null;
    for (const g of rows) {
      const limit = capOf(aliasLimits, g.model);
      if (!limit) {
        unlimited = g.model;
        break;
      }
      const each = limit.adapter === undefined ? limit.max : Math.min(limit.max, limit.adapter);
      if (cap === undefined || each < cap) {
        cap = each;
        cappedBy = g.model;
      }
    }
    if (unlimited) out.push({ level: 'error', message: `Kanata publishes no limit for ${unlimited} and none is declared; declare one for it first.` });
    else if (cap !== undefined) {
      if (total > cap)
        out.push({
          level: 'error',
          message: `Group ${name} declares ${total} permits but Kanata admits at most ${cap} (capped by ${cappedBy}); the bot refuses to start.`,
        });
      else if (total < cap) out.push({ level: 'warning', message: `Group ${name} uses ${total} of the ${cap} permits Kanata admits.` });
      else out.push({ level: 'ok', message: `Group ${name}: ${total} permits, matching Kanata's limit.` });
    }
  }
  if (keyLimits) {
    const sum = groups.reduce((n, g) => n + (Number.isInteger(g.permits) ? (g.permits ?? 0) : 0), 0);
    if (sum > keyLimits.max_in_flight)
      out.push({
        level: 'error',
        message: `All groups declare ${sum} permits but the key admits ${keyLimits.max_in_flight}; the bot refuses to start.`,
      });
    if (keyLimits.shared)
      out.push({ level: 'warning', message: "This key is shared with the owner's other clients; size its limits for both." });
  }
  return out;
}

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
  const choices = [{ value: 'off', label: 'Off' }, ...levels.map((e) => ({ value: e, label: e[0]!.toUpperCase() + e.slice(1) }))];
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
