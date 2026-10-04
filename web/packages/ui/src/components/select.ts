// The dropdown's pure model (P_Select, P_SelectSpec): option rows with group
// headings, search filtering, the keyboard's active row, type-ahead and the
// multi-select's trigger words. Select.svelte and MultiSelect.svelte draw it.
import type { IconName } from './Icon.svelte';

export interface SelectOption {
  value: string;
  label: string;
  /** Secondary text under the label. */
  sub?: string;
  /** Rows sharing a group sit under one heading, in the order given. */
  group?: string;
  icon?: IconName;
  /** A letter in a cookie monogram (members). */
  mono?: string;
  disabled?: boolean;
  /** Extra words the search matches (aliases). */
  keywords?: string;
}

export interface SelectGroup {
  /** Absent for options outside any group. */
  label?: string;
  options: SelectOption[];
}

/** A search box appears above this many options. */
export const SEARCH_ABOVE = 10;

/** Below 840 px or on a coarse pointer the pill wraps the phone's own picker (user decision). */
export const NATIVE_QUERY = '(max-width: 839px), (pointer: coarse)';

export const hasSearch = (options: readonly SelectOption[]) => options.length > SEARCH_ABOVE;

const fold = (s: string) => s.toLocaleLowerCase('en').normalize('NFKD').replace(/\p{M}/gu, '');

export function matches(option: SelectOption, query: string): boolean {
  const q = fold(query.trim());
  if (!q) return true;
  return [option.label, option.sub ?? '', option.keywords ?? ''].some((text) => fold(text).includes(q));
}

export const filterOptions = (options: readonly SelectOption[], query: string) => options.filter((o) => matches(o, query));

/** Runs of options sharing a group, in order; a group with no shown option drops out. */
export function groupsOf(options: readonly SelectOption[]): SelectGroup[] {
  const out: SelectGroup[] = [];
  for (const option of options) {
    const last = out[out.length - 1];
    if (last && last.label === option.group) last.options.push(option);
    else out.push({ label: option.group, options: [option] });
  }
  return out;
}

/** The label split around the first search match, for underlining it. */
export function marked(label: string, query: string): [string, string, string] {
  const q = fold(query.trim());
  const i = q ? fold(label).indexOf(q) : -1;
  // Folding can change lengths (decomposed accents); only mark when it did not.
  if (i < 0 || fold(label).length !== label.length) return [label, '', ''];
  return [label.slice(0, i), label.slice(i, i + q.length), label.slice(i + q.length)];
}

const enabled = (options: readonly SelectOption[]) => options.filter((o) => !o.disabled);

/** The row the list opens on: the current value when enabled and shown, else the first enabled row. */
export function startRow(options: readonly SelectOption[], value: string): string | null {
  const here = options.find((o) => o.value === value && !o.disabled);
  return here ? here.value : (enabled(options)[0]?.value ?? null);
}

/** ↑/↓ from the active row, skipping disabled rows and stopping at the ends. */
export function step(options: readonly SelectOption[], active: string | null, dir: 1 | -1): string | null {
  const list = enabled(options);
  if (!list.length) return null;
  const i = list.findIndex((o) => o.value === active);
  if (i < 0) return (dir > 0 ? list[0] : list[list.length - 1])!.value;
  return list[Math.max(0, Math.min(list.length - 1, i + dir))]!.value;
}

/** Home and End. */
export function edge(options: readonly SelectOption[], end: 'first' | 'last'): string | null {
  const list = enabled(options);
  return (end === 'first' ? list[0] : list[list.length - 1])?.value ?? null;
}

/** The active row after a navigation key (↑ ↓ Home End), or undefined for any other key. */
export function navigate(key: string, options: readonly SelectOption[], active: string | null): string | null | undefined {
  switch (key) {
    case 'ArrowDown':
      return step(options, active, 1);
    case 'ArrowUp':
      return step(options, active, -1);
    case 'Home':
      return edge(options, 'first');
    case 'End':
      return edge(options, 'last');
    default:
      return undefined;
  }
}

/**
 * Type-ahead: the next enabled row whose label starts with what was typed.
 * One repeated letter cycles through its matches; a longer buffer keeps the
 * active row while it still matches.
 */
export function typeAhead(options: readonly SelectOption[], active: string | null, buffer: string): string | null {
  const list = enabled(options);
  if (!list.length || !buffer) return null;
  const typed = fold(buffer);
  const cycling = [...typed].every((c) => c === typed[0]);
  const needle = cycling ? typed[0]! : typed;
  const i = list.findIndex((o) => o.value === active);
  const from = cycling ? i + 1 : Math.max(i, 0);
  const order = [...list.slice(from), ...list.slice(0, from)];
  return order.find((o) => fold(o.label).startsWith(needle))?.value ?? null;
}

/** Toggles one value, keeping the options' order. */
export function toggle(options: readonly SelectOption[], values: readonly string[], value: string): string[] {
  const on = new Set(values);
  if (on.has(value)) on.delete(value);
  else on.add(value);
  return options.filter((o) => on.has(o.value)).map((o) => o.value);
}

/** Every enabled option (All). */
export const allOf = (options: readonly SelectOption[]) => enabled(options).map((o) => o.value);

/** The multi-select trigger: "3 of 8"; one picked names it; "none" and "all 8" at the ends. */
export function countWords(options: readonly SelectOption[], values: readonly string[]): string {
  const picked = options.filter((o) => values.includes(o.value));
  if (picked.length === 0) return 'none';
  if (picked.length === 1) return picked[0]!.label;
  if (picked.length === options.length) return `all ${options.length}`;
  return `${picked.length} of ${options.length}`;
}
