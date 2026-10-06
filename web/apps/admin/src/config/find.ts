// "Find a setting": picks the card (or env/access table row) inside an open
// section that best matches the search, then brings it into view. Matching
// reads the rendered words, so a setting is found by what its card says.

import { reducedMotion } from '@kanade/ui';

/** The things a search can land on, most specific last. */
const CANDIDATES = '.settings__card, .rescan__card, .settings__table tbody tr';
const TITLE = '.settings__cardtitle, legend, h3, h4, th[scope="row"], b';
const FLASH_MS = 1600;

export interface Found {
  title: string;
  /** Opens its pill tab if needed, scrolls it into view in the panel, rings it and focuses its control. */
  reveal(): Promise<void>;
}

const squash = (text: string | null | undefined) => (text ?? '').replace(/\s+/g, ' ').trim().toLowerCase();

/** 4 the phrase in its title, 3 every word there, 2 the phrase in its text, 1 every word, else 0. */
export function score(query: string, title: string, text: string): number {
  const phrase = squash(query);
  const words = phrase.split(' ').filter(Boolean);
  if (!words.length) return 0;
  const t = squash(title);
  const all = squash(text);
  if (t.includes(phrase)) return 4;
  if (words.every((w) => t.includes(w))) return 3;
  if (all.includes(phrase)) return 2;
  if (words.every((w) => all.includes(w))) return 1;
  return 0;
}

/** The pill tab panel (Persona, Models) that holds `el` inside `section`, if any. */
function pillPanel(el: Element, section: Element): HTMLElement | null {
  const panel = el.parentElement?.closest<HTMLElement>('[role="tabpanel"]');
  return panel && panel !== section && section.contains(panel) ? panel : null;
}

export function findSetting(section: HTMLElement, query: string): Found | null {
  let best: { el: HTMLElement; title: HTMLElement | null; score: number; size: number } | null = null;
  for (const el of section.querySelectorAll<HTMLElement>(CANDIDATES)) {
    const panel = pillPanel(el, section);
    const pill = panel ? (document.getElementById(panel.getAttribute('aria-labelledby') ?? '')?.textContent ?? '') : '';
    // The label that names what was asked for, else the card's first one.
    const titles = [...el.querySelectorAll<HTMLElement>(TITLE)];
    const title = titles.find((t) => score(query, t.textContent ?? '', '') > 0) ?? titles[0] ?? null;
    const text = `${pill} ${el.textContent ?? ''}`;
    const s = score(query, title?.textContent ?? '', text);
    if (!s) continue;
    // Equal scores: the smaller (more specific) element wins, so a table row beats its card.
    if (!best || s > best.score || (s === best.score && text.length < best.size)) best = { el, title, score: s, size: text.length };
  }
  if (!best) return null;
  const { el, title } = best;
  return { title: title?.textContent?.replace(/\s+/g, ' ').trim() || 'the matching setting', reveal: () => reveal(el, title, section) };
}

const FIELDS = 'input[type="radio"]:checked, input:not([type="hidden"]):not([type="radio"]):not([disabled]), select:not([disabled]), textarea:not([disabled])';
const FOCUSABLE = 'input[type="radio"], button:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';

/** The control the found label introduces: the first field after it, else the first focusable one, else the card's first. */
function controlFor(el: HTMLElement, title: HTMLElement | null): HTMLElement | null {
  const after = (selector: string) =>
    [...el.querySelectorAll<HTMLElement>(selector)].find((c) => !title || title.contains(c) || title.compareDocumentPosition(c) & Node.DOCUMENT_POSITION_FOLLOWING);
  return after(FIELDS) ?? after(FOCUSABLE) ?? el.querySelector<HTMLElement>(FIELDS) ?? el.querySelector<HTMLElement>(FOCUSABLE);
}

async function reveal(el: HTMLElement, title: HTMLElement | null, section: HTMLElement): Promise<void> {
  const panel = pillPanel(el, section);
  if (panel?.hidden) {
    document.getElementById(panel.getAttribute('aria-labelledby') ?? '')?.click();
    await frame();
  }
  const reduced = reducedMotion();
  // Scroll only the panel's own scroller: scrollIntoView could move the fixed frame.
  const scroller = el.closest<HTMLElement>('.settings__scroll');
  if (scroller) {
    const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop - 12;
    scroller.scrollTo({ top: Math.max(0, top), behavior: reduced ? 'auto' : 'smooth' });
  }
  el.classList.remove('settings__found');
  void el.offsetWidth; // restart the ring when the same card is found twice
  el.classList.add('settings__found');
  window.setTimeout(() => el.classList.remove('settings__found'), FLASH_MS);
  const control = controlFor(el, title);
  if (control) control.focus({ preventScroll: true });
  else {
    el.tabIndex = -1;
    el.focus({ preventScroll: true });
  }
}

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

/** Re-runs `probe` each frame until it returns a value or `frames` pass. */
export async function settleFrames<T>(probe: () => T | null, frames: number): Promise<T | null> {
  for (let i = 0; i < frames; i++) {
    const hit = probe();
    if (hit) return hit;
    await frame();
  }
  return probe();
}
