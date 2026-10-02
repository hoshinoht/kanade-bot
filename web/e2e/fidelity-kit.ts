import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Browser, Page } from '@playwright/test';

// Layout-fidelity kit for e2e/fidelity.spec.ts: renders an M3E board, reads the
// skeleton of every [data-fid] region (board and app share the names), and
// diffs the two. Font family and colour are left out on purpose: the spec
// swaps the boards' Sometype Mono and hex values for the app's tokens.

export const MOCKUPS = process.env.KANADE_MOCKUPS ?? join(import.meta.dirname, '..', '..', 'docs', 'research', '2026-09-28-m3e-mockups');
/** Instances compared per repeated name (rows, messages, diff blocks). */
export const PER_NAME = 2;

const STYLE_KEYS = [
  'display', 'position', 'flexDirection', 'flexWrap', 'justifyContent', 'alignItems', 'alignSelf', 'gridTemplateColumns',
  'rowGap', 'columnGap', 'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft', 'marginTop',
  'borderTopLeftRadius', 'borderTopWidth', 'fontSize', 'fontWeight', 'lineHeight', 'letterSpacing', 'textTransform', 'overflowY',
] as const;
export type StyleKey = (typeof STYLE_KEYS)[number];

export interface Box {
  /** Relative to the nearest tagged ancestor (the viewport for top-level regions). */
  x: number;
  y: number;
  w: number;
  h: number;
  /** Gap to the parent's right and bottom edges: a small bottom gap means pinned to the bottom. */
  right: number;
  bottom: number;
  pw: number;
  ph: number;
}

export interface Region {
  fid: string;
  n: number;
  parent: string | null;
  visible: boolean;
  leaf: boolean;
  box: Box;
  abs: { x: number; y: number; w: number; h: number };
  children: string[];
  style: Record<StyleKey, string>;
}

export interface Finding {
  fid: string;
  n: number;
  /** 1 structure or missing, 2 layout and geometry, 3 spacing, type and shape, 4 app-only. */
  severity: 1 | 2 | 3 | 4;
  kind: string;
  board: string;
  app: string;
}

const between = (s: string, a: string, b: string) => {
  const i = s.indexOf(a);
  if (i < 0) return '';
  const j = s.indexOf(b, i + a.length);
  return s.slice(i + a.length, j);
};

export function boardExists(name: string): boolean {
  return existsSync(join(MOCKUPS, `${name}.dc.html`));
}

/** The same stand-in for the canvas runtime as the mockups' render/render.mjs, at 1× scale. */
export async function renderBoard(browser: Browser, name: string): Promise<{ page: Page; width: number; height: number; errors: string[] }> {
  const src = readFileSync(join(MOCKUPS, `${name}.dc.html`), 'utf8');
  const runtime = readFileSync(join(MOCKUPS, 'render', 'runtime.js'), 'utf8');
  const blobsPath = join(MOCKUPS, 'render', 'blobs.json');
  const blobs = existsSync(blobsPath) ? (JSON.parse(readFileSync(blobsPath, 'utf8')) as Record<string, string>) : {};
  const helmet = between(src, '<helmet>', '</helmet>');
  const body = between(src, '</helmet>', '</x-dc>');
  const tag = src.slice(src.indexOf('data-dc-script'));
  const props = JSON.parse(between(tag, "data-props='", "'").replace(/&amp;/g, '&').replace(/&#39;/g, "'")) as {
    $preview: { width: number; height: number };
  };
  const script = between(tag, '>', '</script>');
  const { width, height } = props.$preview;
  const esc = (s: string) => JSON.stringify(s).replace(/<\/script/gi, '<\\/script');
  const html = `<!doctype html><html><head><meta charset="utf-8">${helmet}</head><body style="margin:0"><div id="root"></div><script>${runtime}\n__renderBoard(${esc(body)}, ${esc(script)});</script></body></html>`;
  const page = await browser.newPage({ viewport: { width, height } });
  await page.route('http://kanade.local/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.startsWith('/_blob/')) {
      const path = blobs[url.pathname.slice(7)];
      return path && existsSync(path) ? route.fulfill({ path }) : route.fulfill({ status: 404 });
    }
    return route.fulfill({ contentType: 'text/html', body: html });
  });
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('http://kanade.local/');
  await settle(page);
  return { page, width, height, errors };
}

export async function settle(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await document.fonts.ready;
    const eager = [...document.images].filter((i) => i.loading !== 'lazy' || i.getBoundingClientRect().top < innerHeight);
    await Promise.race([Promise.all(eager.map((i) => i.decode().catch(() => {}))), new Promise((r) => setTimeout(r, 3000))]);
  });
}

export function skeleton(page: Page): Promise<Region[]> {
  return page.evaluate(({ limit, keys }) => {
    const owner = (el: Element) => el.parentElement?.closest<HTMLElement>('[data-fid]') ?? null;
    const seen = new Map<string, number>();
    const out = [];
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid]')) {
      const fid = el.dataset.fid!;
      const n = seen.get(fid) ?? 0;
      seen.set(fid, n + 1);
      if (n >= limit) continue;
      const parent = owner(el);
      const r = el.getBoundingClientRect();
      const p = parent?.getBoundingClientRect() ?? new DOMRect(0, 0, innerWidth, innerHeight);
      const cs = getComputedStyle(el);
      const kids = [...el.querySelectorAll<HTMLElement>('[data-fid]')].filter((c) => owner(c) === el).map((c) => c.dataset.fid!);
      const computed = cs as unknown as Record<string, string>;
      const style = Object.fromEntries(keys.map((k) => [k, computed[k] ?? '']));
      out.push({
        fid,
        n,
        parent: parent?.dataset.fid ?? null,
        visible: r.width > 0 && r.height > 0 && cs.visibility !== 'hidden',
        leaf: kids.length === 0,
        box: { x: r.x - p.x, y: r.y - p.y, w: r.width, h: r.height, right: p.right - r.right, bottom: p.bottom - r.bottom, pw: p.width, ph: p.height },
        abs: { x: r.x, y: r.y, w: r.width, h: r.height },
        children: kids.filter((k, i) => k !== kids[i - 1]),
        style,
      });
    }
    return out as Region[];
  }, { limit: PER_NAME, keys: [...STYLE_KEYS] });
}

const num = (v: string | undefined) => (v === undefined || v === 'normal' || v === 'none' ? 0 : parseFloat(v) || 0);
const tracks = (v: string) => (v === 'none' ? 0 : v.split(/\s+/).filter(Boolean).length);
const r1 = (v: number) => Math.round(v * 10) / 10;

export function diff(board: Region[], app: Region[]): Finding[] {
  const key = (r: Region) => `${r.fid}#${r.n}`;
  const inApp = new Map(app.map((r) => [key(r), r]));
  const inBoard = new Set(board.map(key));
  const out: Finding[] = [];
  const add = (r: Region, severity: Finding['severity'], kind: string, b: unknown, a: unknown) =>
    out.push({ fid: r.fid, n: r.n, severity, kind, board: String(b), app: String(a) });

  for (const b of board) {
    const a = inApp.get(key(b));
    if (!a || !a.visible) {
      // A later instance the app doesn't render (fewer rows in its data) is not a fidelity miss.
      if (b.n === 0) add(b, 1, a ? 'hidden in app' : 'missing in app', 'present', a ? 'not visible' : 'absent');
      continue;
    }
    if (b.parent !== a.parent) add(b, 1, 'different parent region', b.parent ?? '(top level)', a.parent ?? '(top level)');
    if (b.n === 0 && b.children.join(' ') !== a.children.join(' ')) add(b, 1, 'child order', b.children.join(' › ') || '—', a.children.join(' › ') || '—');

    const bs = b.style;
    const as = a.style;
    for (const k of ['display', 'position'] as const) if (bs[k] !== as[k]) add(b, 1, k, bs[k], as[k]);
    const flex = bs.display.includes('flex') && as.display.includes('flex');
    if (flex) for (const k of ['flexDirection', 'flexWrap'] as const) if (bs[k] !== as[k]) add(b, 1, k, bs[k], as[k]);
    if (flex || (bs.display.includes('grid') && as.display.includes('grid')))
      for (const k of ['justifyContent', 'alignItems'] as const) if (bs[k] !== as[k]) add(b, 2, k, bs[k], as[k]);
    if (bs.alignSelf !== as.alignSelf) add(b, 2, 'alignSelf', bs.alignSelf, as.alignSelf);
    if (bs.display.includes('grid') && tracks(bs.gridTemplateColumns) !== tracks(as.gridTemplateColumns))
      add(b, 1, 'grid columns', bs.gridTemplateColumns, as.gridTemplateColumns);

    // Geometry relative to the parent region; widths and heights as a share of it too.
    const bb = b.box;
    const ab = a.box;
    const pinned = (x: Box) => x.bottom <= 24;
    if (pinned(bb) !== pinned(ab) && Math.abs(bb.bottom - ab.bottom) > 32)
      add(b, 1, pinned(bb) ? 'pinned to the parent bottom in board only' : 'pinned to the parent bottom in app only', `${r1(bb.bottom)}px from bottom`, `${r1(ab.bottom)}px from bottom`);
    for (const [k, extent, tol] of [
      ['w', 'pw', 0.06],
      ['h', 'ph', 0.1],
    ] as const) {
      const delta = Math.abs(bb[k] - ab[k]);
      if (delta > Math.max(8, tol * bb[k]))
        add(b, 2, k === 'w' ? 'width' : 'height', `${r1(bb[k])}px (${Math.round((100 * bb[k]) / bb[extent])}% of parent)`, `${r1(ab[k])}px (${Math.round((100 * ab[k]) / ab[extent])}% of parent)`);
    }
    for (const k of ['x', 'y'] as const) {
      const delta = Math.abs(bb[k] - ab[k]);
      if (delta > Math.max(8, 0.05 * (k === 'x' ? bb.pw : bb.ph))) add(b, 2, k === 'x' ? 'left offset in parent' : 'top offset in parent', `${r1(bb[k])}px`, `${r1(ab[k])}px`);
    }

    for (const k of ['rowGap', 'columnGap', 'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft'] as const)
      if (Math.abs(num(bs[k]) - num(as[k])) > 2) add(b, 3, k, bs[k], as[k]);
    if (Math.abs(num(bs.borderTopLeftRadius) - num(as.borderTopLeftRadius)) > 3) add(b, 3, 'radius', bs.borderTopLeftRadius, as.borderTopLeftRadius);
    if (Math.abs(num(bs.borderTopWidth) - num(as.borderTopWidth)) > 0.5) add(b, 3, 'border width', bs.borderTopWidth, as.borderTopWidth);
    if (b.leaf) {
      if (Math.abs(num(bs.fontSize) - num(as.fontSize)) > 1) add(b, 3, 'font size', bs.fontSize, as.fontSize);
      if (Math.abs(num(bs.fontWeight) - num(as.fontWeight)) >= 100) add(b, 3, 'font weight', bs.fontWeight, as.fontWeight);
      if (bs.lineHeight !== 'normal' && as.lineHeight !== 'normal' && Math.abs(num(bs.lineHeight) - num(as.lineHeight)) > 2)
        add(b, 3, 'line height', bs.lineHeight, as.lineHeight);
      if (Math.abs(num(bs.letterSpacing) - num(as.letterSpacing)) > 0.5) add(b, 3, 'letter spacing', bs.letterSpacing, as.letterSpacing);
      if (bs.textTransform !== as.textTransform) add(b, 3, 'text transform', bs.textTransform, as.textTransform);
    }
  }
  for (const a of app) if (a.n === 0 && !inBoard.has(key(a)) && !board.some((r) => r.fid === a.fid)) add(a, 4, 'app only (untagged on the board?)', 'absent', 'present');

  const order = new Map(board.map((r, i) => [r.fid, i]));
  return out.sort((x, y) => x.severity - y.severity || (order.get(x.fid) ?? 1e9) - (order.get(y.fid) ?? 1e9) || x.n - y.n);
}

const SEVERITY = ['', 'structure', 'layout', 'detail', 'app-only'];
const COLOUR = ['', '#d62828', '#f77f00', '#3a86ff', '#8d99ae'];

export function markdown(name: string, board: string, path: string, findings: Finding[], notes: string[]): string {
  const rows = findings.map(
    (f, i) => `| ${i + 1} | ${SEVERITY[f.severity]} | \`${f.fid}\`${f.n ? ` #${f.n + 1}` : ''} | ${f.kind} | ${f.board} | ${f.app} |`,
  );
  return [
    `## ${name}: \`${board}\` vs \`${path}\``,
    '',
    ...(notes.length ? [...notes.map((n) => `> ${n}`), ''] : []),
    findings.length ? '| # | severity | region | what | board | app |\n|---|---|---|---|---|---|' : 'No differences above tolerance.',
    ...rows,
    '',
  ].join('\n');
}

/** Board left, app right, each finding's region outlined and numbered on both. */
export async function composite(
  browser: Browser,
  out: string,
  left: { png: Buffer; regions: Region[]; width: number; height: number },
  right: { png: Buffer; regions: Region[]; width: number; height: number },
  findings: Finding[],
): Promise<void> {
  const gap = 24;
  const head = 28;
  // One outline per region, labelled with all its finding numbers, in its most severe colour.
  const byRegion = new Map<string, { fid: string; n: number; severity: number; numbers: number[] }>();
  findings.forEach((f, i) => {
    const k = `${f.fid}#${f.n}`;
    const entry = byRegion.get(k) ?? { fid: f.fid, n: f.n, severity: f.severity, numbers: [] };
    entry.severity = Math.min(entry.severity, f.severity);
    entry.numbers.push(i + 1);
    byRegion.set(k, entry);
  });
  const boxes = (side: Region[], dx: number) =>
    [...byRegion.values()]
      .map((e) => ({ e, r: side.find((r) => r.fid === e.fid && r.n === e.n) }))
      .filter((x): x is { e: typeof x.e; r: Region } => Boolean(x.r?.visible))
      .map(
        ({ e, r }) =>
          `<div style="position:absolute;left:${dx + r.abs.x}px;top:${head + r.abs.y}px;width:${r.abs.w}px;height:${r.abs.h}px;outline:2px solid ${COLOUR[e.severity]};outline-offset:-1px"><span style="position:absolute;left:0;top:0;background:${COLOUR[e.severity]};color:#fff;font:700 11px system-ui;padding:0 3px;white-space:nowrap">${e.numbers.join(',')}</span></div>`,
      )
      .join('');
  const width = left.width + gap + right.width;
  const height = head + Math.max(left.height, right.height);
  const label = (x: number, text: string) => `<div style="position:absolute;left:${x}px;top:6px;font:600 13px system-ui;color:#eee">${text}</div>`;
  const html = `<body style="margin:0;background:#222;position:relative;width:${width}px;height:${height}px">${label(0, 'board')}${label(left.width + gap, 'app')}<img style="position:absolute;left:0;top:${head}px" src="data:image/png;base64,${left.png.toString('base64')}"><img style="position:absolute;left:${left.width + gap}px;top:${head}px" src="data:image/png;base64,${right.png.toString('base64')}">${boxes(left.regions, 0)}${boxes(right.regions, left.width + gap)}</body>`;
  const page = await browser.newPage({ viewport: { width, height } });
  await page.setContent(html);
  await page.screenshot({ path: out });
  await page.close();
}
