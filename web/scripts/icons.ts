// Renders the two apps' install icons (PNG) with the installed Chrome.
// Run: bun scripts/icons.ts. Output is committed under apps/*/public/icons.
import { chromium } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

interface Face {
  app: 'public' | 'admin';
  ground: string;
  win: string;
  surface: string;
  ink: string;
  accent: string;
  label: string;
}

const FACES: Face[] = [
  { app: 'public', ground: '#eec75f', win: '#5f6579', surface: '#fbf6e8', ink: '#4a4a58', accent: '#4d5c9e', label: '' },
  { app: 'admin', ground: '#232735', win: '#4f5871', surface: '#fbf6e8', ink: '#4a4a58', accent: '#e9c25e', label: 'ADMIN' },
];

/** A Kanade window on its ground; `inset` shrinks it into the maskable safe zone. */
function svg(face: Face, inset: number): string {
  const s = 512;
  const w = s - inset * 2;
  const x = inset;
  const bar = w * 0.17;
  const r = w * 0.09;
  const dot = w * 0.028;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${s}" height="${s}" viewBox="0 0 ${s} ${s}">
  <rect width="${s}" height="${s}" fill="${face.ground}"/>
  <rect x="${x}" y="${x}" width="${w}" height="${w}" rx="${r}" fill="${face.surface}" stroke="${face.win}" stroke-width="${w * 0.03}"/>
  <path d="M${x} ${x + r} a${r} ${r} 0 0 1 ${r} -${r} h${w - 2 * r} a${r} ${r} 0 0 1 ${r} ${r} v${bar - r} h-${w} z" fill="${face.win}"/>
  ${[0, 1, 2].map((i) => `<circle cx="${x + w * 0.1 + i * dot * 3.2}" cy="${x + bar / 2}" r="${dot}" fill="${face.surface}" opacity="0.6"/>`).join('')}
  <text x="${s / 2}" y="${x + bar + (w - bar) * (face.label ? 0.6 : 0.72)}" text-anchor="middle" font-family="Georgia, serif" font-weight="700" font-size="${w * 0.5}" fill="${face.ink}">K</text>
  ${face.label ? `<rect x="${x + w * 0.2}" y="${x + w * 0.76}" width="${w * 0.6}" height="${w * 0.13}" rx="${w * 0.065}" fill="${face.accent}"/><text x="${s / 2}" y="${x + w * 0.855}" text-anchor="middle" font-family="Menlo, monospace" font-weight="700" font-size="${w * 0.075}" letter-spacing="${w * 0.01}" fill="${face.ground}">${face.label}</text>` : ''}
</svg>`;
}

const browser = await chromium.launch({ channel: 'chrome' });
const page = await browser.newPage();
for (const face of FACES) {
  const dir = join(import.meta.dir, '..', 'apps', face.app, 'public', 'icons');
  mkdirSync(dir, { recursive: true });
  const shots: [string, number, number][] = [
    [`${face.app}-512.png`, 512, 40],
    [`${face.app}-192.png`, 192, 40],
    [`${face.app}-180.png`, 180, 40],
    [`${face.app}-maskable-512.png`, 512, 104],
  ];
  for (const [name, size, inset] of shots) {
    await page.setViewportSize({ width: size, height: size });
    await page.setContent(
      `<html><body style="margin:0"><img src="data:image/svg+xml;base64,${Buffer.from(svg(face, inset)).toString('base64')}" width="${size}" height="${size}"></body></html>`,
    );
    await page.screenshot({ path: join(dir, name), clip: { x: 0, y: 0, width: size, height: size } });
  }
}
await browser.close();
console.log('icons written');
