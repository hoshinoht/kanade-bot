// The estimate share chart, "Who carries" (boards in
// docs/research/2026-10-15-estimate-share-chart, option A): one 100% bar per
// boss, each member's part their effective % over the party's combined %, in
// party order. Only the effective % counts: a party card's whole-party figure
// (`ScouterReading.percentage`'s fallback) never becomes a share, and a member
// without an estimate is counted and named, never drawn as 0%.

/** The fixed member colours (`--who-1` … `--who-6`); a party looks up at most six characters. */
export const WHO_COLOURS = 6;

export interface ShareMember {
  /** Stable key (the member's id). */
  id: string;
  /** As the party list (both apps) names them. */
  name: string;
  /** The character each boss brings, per catalog token (a member may bring another to a later boss). */
  igns: Readonly<Record<string, string>>;
  /** Their number: their place in the party, 1-based; `null` (no character) is never numbered. */
  n: number | null;
  /** Effective % per catalog token; a missing or `null` entry is no estimate for that boss. */
  effective: Readonly<Record<string, number | null | undefined>>;
  /** Per catalog token, the scouter assumed a 100% critical rate: hatched and starred. */
  assumed: Readonly<Record<string, boolean | undefined>>;
}

export interface SharePart {
  id: string;
  name: string;
  /** The character that boss brings, when known. */
  ign: string | null;
  n: number;
  /** 1 … 6: `--who-<colour>`. */
  colour: number;
  effective: number;
  /** Whole %, the bar's parts adding up to exactly 100. */
  share: number;
  assumed: boolean;
}

export interface ShareBar {
  token: string;
  /** The party total: the sum of the members' effective %; `null` with no part. */
  total: number | null;
  /** Every member has an effective % for this boss. */
  complete: boolean;
  /** Party order. */
  parts: SharePart[];
  /** No estimate for this boss, in party order. */
  missing: ShareMember[];
}

/** A member's colour follows their number, wrapping past six. */
export const whoColour = (n: number) => ((n - 1) % WHO_COLOURS) + 1;

/** Whole-% shares of `values` adding up to exactly 100 (largest remainder; ties to the earlier member). */
export function wholeShares(values: readonly number[]): number[] {
  const sum = values.reduce((a, b) => a + b, 0);
  if (!(sum > 0)) return values.map(() => 0);
  const raw = values.map((v) => (v / sum) * 100);
  const out = raw.map(Math.floor);
  let left = 100 - out.reduce((a, b) => a + b, 0);
  const order = raw.map((v, i) => [v - Math.floor(v), i] as const).sort((a, b) => b[0] - a[0] || a[1] - b[1]);
  for (const [, i] of order) {
    if (left <= 0) break;
    out[i]! += 1;
    left -= 1;
  }
  return out;
}

const known = (member: ShareMember, token: string): number | null => {
  const value = member.n === null ? null : member.effective[token];
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
};

/** One boss's bar over `members` (party order). */
export function shareBar(token: string, members: readonly ShareMember[]): ShareBar {
  const found = members.flatMap((m) => {
    const value = known(m, token);
    return value === null ? [] : [{ m, value }];
  });
  const shares = wholeShares(found.map((f) => f.value));
  const parts = found.map(({ m, value }, i) => ({
    id: m.id,
    name: m.name,
    ign: m.igns[token] ?? null,
    n: m.n!,
    colour: whoColour(m.n!),
    effective: value,
    share: shares[i]!,
    assumed: m.assumed[token] === true,
  }));
  const missing = members.filter((m) => known(m, token) === null);
  return {
    token,
    total: parts.length ? parts.reduce((sum, p) => sum + p.effective, 0) : null,
    complete: missing.length === 0,
    parts,
    missing,
  };
}

/** The bar's accessible name: "HCarling shares: Asahi (AsahiDK) 31%, Mika (MikaBish) 16% (assumes 100% critical rate)". */
export function shareLabel(label: string, bar: ShareBar): string {
  const parts = bar.parts.map((p) => `${p.name}${p.ign ? ` (${p.ign})` : ''} ${p.share}%${p.assumed ? ' (assumes 100% critical rate)' : ''}`).join(', ');
  const none = bar.missing.length ? `; ${bar.missing.length} with no estimate` : '';
  return `${label} shares: ${parts}${none}`;
}

/** "Asahi", "Asahi and Ren", "Asahi, Ren and Mika". */
export function joinNames(names: readonly string[]): string {
  return names.length < 2 ? names.join('') : `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`;
}
