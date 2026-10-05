import type { Page } from '@playwright/test';

/**
 * In-page text clipping audit, the app's counterpart of the boards'
 * `render/measure.mjs` (docs/v5/design/verification.md "Measuring text in the
 * app"). For every rendered element that holds its own text:
 *
 *   clip-x / clip-y  its content is wider/taller than its own box under overflow hidden/clip
 *   spill            its text runs past its own border box (overflow visible)
 *   cut              its text is partly cut by a clipping ancestor (overflow hidden/clip)
 *   hidden           its text lies wholly outside a clipping ancestor that does not scroll
 *   off-screen       its text runs past the viewport horizontally
 *   ellipsis         cut on purpose (`text-overflow: ellipsis` or a line clamp) with its
 *                    full text in a `title`/`aria-label`; `ellipsis-bare` without one
 *
 * Skipped: hidden, visually hidden (`.vh`, ≤ 1 px) and aria-hidden text, form
 * fields, SVG, and text inside a scroll container (overflow auto/scroll),
 * which the user can scroll to.
 */
export interface Finding {
  kind: 'clip-x' | 'clip-y' | 'spill' | 'cut' | 'hidden' | 'off-screen' | 'ellipsis' | 'ellipsis-bare';
  /** Up to four ancestors, tag.class.class, ending at the element. */
  path: string;
  text: string;
  /** Overflow in px and what clipped it. */
  detail: string;
  /** Indexes of the allow-list selectors the element, the clipping box or an ancestor matches. */
  allowed: number[];
}

export function auditText(page: Page, allow: string[]): Promise<Finding[]> {
  return page.evaluate((allow) => {
    const W = document.documentElement.clientWidth;
    const SKIP = new Set(['SCRIPT', 'STYLE', 'TEMPLATE', 'NOSCRIPT', 'TEXTAREA', 'SELECT', 'OPTION', 'INPUT', 'TITLE']);
    const styles = new Map<Element, CSSStyleDeclaration>();
    const style = (el: Element) => {
      let cs = styles.get(el);
      if (!cs) styles.set(el, (cs = getComputedStyle(el)));
      return cs;
    };
    const name = (e: Element) => {
      const cls = typeof e.className === 'string' ? e.className.trim().split(/\s+/).filter(Boolean).slice(0, 2) : [];
      return e.tagName.toLowerCase() + cls.map((c) => `.${c}`).join('');
    };
    const path = (el: Element) => {
      const parts: string[] = [];
      for (let e: Element | null = el; e && e !== document.body && parts.length < 4; e = e.parentElement) parts.unshift(name(e));
      return parts.join(' > ');
    };
    const clips = (v: string) => v === 'hidden' || v === 'clip';
    const scrolls = (cs: CSSStyleDeclaration) => /auto|scroll/.test(cs.overflowX + cs.overflowY);
    const transformed = (cs: CSSStyleDeclaration) => cs.transform !== 'none' || cs.filter !== 'none' || /paint|strict|content/.test(cs.contain);
    // The full text of an ellipsised value: a title or aria-label on it or its row/control.
    const titled = (el: Element) => {
      const stop = el.closest('a, button, label, li, tr, td, th, [role]');
      for (let e: Element | null = el; e; e = e.parentElement) {
        if (e.getAttribute('title') || e.getAttribute('aria-label')) return true;
        if (e === stop) break;
      }
      return false;
    };
    // Screen-reader-only boxes (`.vh` and its mixins): clip-path inset(50%) or an absolute 1 px clipped box.
    // A flex item squeezed to nothing is not one of these: its text is reported.
    const visuallyHidden = (el: Element) => {
      for (let e: Element | null = el; e; e = e.parentElement) {
        const cs = style(e);
        if (cs.clipPath === 'inset(50%)') return true;
        if (cs.position === 'absolute' && cs.overflowX !== 'visible') {
          const r = e.getBoundingClientRect();
          if (r.width <= 1 && r.height <= 1) return true;
        }
      }
      return false;
    };
    const allowedBy = (...els: Element[]) => allow.flatMap((sel, i) => (els.some((e) => e.closest(sel)) ? [i] : []));

    const out: {
      kind: string;
      path: string;
      text: string;
      detail: string;
      allowed: number[];
    }[] = [];
    for (const el of document.body.querySelectorAll('*')) {
      if (SKIP.has(el.tagName) || el instanceof SVGElement || !(el instanceof HTMLElement)) continue;
      const nodes = [...el.childNodes].filter((n) => n.nodeType === Node.TEXT_NODE && n.textContent!.trim());
      if (!nodes.length) continue;
      if (!el.checkVisibility({ opacityProperty: true, visibilityProperty: true })) continue;
      if (el.closest('[aria-hidden="true"], .vh, [hidden]') || visuallyHidden(el)) continue;
      const box = el.getBoundingClientRect();
      if (box.width <= 1 || box.height <= 1) continue;
      const cs = style(el);
      const text = nodes.map((n) => n.textContent!.trim()).join(' ').replace(/\s+/g, ' ').slice(0, 60);
      const push = (kind: string, detail: string, at: Element = el) =>
        out.push({ kind, path: path(el), text, detail, allowed: allowedBy(el, at) });

      // The own text's extent (child elements are measured on their own).
      const rects = nodes.flatMap((n) => {
        const range = document.createRange();
        range.selectNodeContents(n);
        return [...range.getClientRects()].filter((r) => r.width > 0 && r.height > 0);
      });
      if (!rects.length) continue;
      const t = {
        left: Math.min(...rects.map((r) => r.left)),
        right: Math.max(...rects.map((r) => r.right)),
        top: Math.min(...rects.map((r) => r.top)),
        bottom: Math.max(...rects.map((r) => r.bottom)),
      };
      // Vertically, the glyphs fill the em box, not the font's taller content
      // area: tight line heights (clocks, mono numerals) are not clipping.
      const inset = Math.max(0, (Math.min(...rects.map((r) => r.height)) - parseFloat(cs.fontSize)) / 2);
      const em = { top: t.top + inset, bottom: t.bottom - inset };

      // Intentional truncation of the element's own line(s).
      const clamped = cs.webkitLineClamp !== 'none' && cs.webkitLineClamp !== '' && el.scrollHeight > el.clientHeight + 1;
      if ((cs.textOverflow === 'ellipsis' && el.scrollWidth > el.clientWidth + 1) || clamped) {
        push(titled(el) ? 'ellipsis' : 'ellipsis-bare', clamped ? `clamp ${el.scrollHeight}>${el.clientHeight}` : `${el.scrollWidth}>${el.clientWidth}`);
        continue;
      }
      const block = !/^(inline|contents)$/.test(cs.display);
      if (block && !scrolls(cs)) {
        if (clips(cs.overflowX) && el.scrollWidth > el.clientWidth + 1) push('clip-x', `${el.scrollWidth}>${el.clientWidth}`);
        if (clips(cs.overflowY) && Math.max(box.top - em.top, em.bottom - box.bottom) > 1) push('clip-y', `${Math.round(em.bottom - box.bottom)}px below its box`);
        if (cs.overflowX === 'visible' && cs.overflowY === 'visible') {
          const d = Math.max(box.left - t.left, t.right - box.right, box.top - em.top, em.bottom - box.bottom);
          if (d > 1) push('spill', `${Math.round(d)}px past its box`);
        }
      }

      // Clipping ancestors, honouring containing blocks: an absolute or fixed
      // box escapes overflow on ancestors that are not its containing block.
      let escape = cs.position === 'absolute' ? 'abs' : cs.position === 'fixed' ? 'fixed' : '';
      let scrolled = false;
      let reported = false;
      for (let a = el.parentElement; a && a !== document.documentElement; a = a.parentElement) {
        const acs = style(a);
        const holds = escape === '' || (escape === 'abs' ? acs.position !== 'static' || transformed(acs) : transformed(acs));
        if (holds) escape = '';
        if (acs.position === 'absolute') escape = 'abs';
        else if (acs.position === 'fixed') escape = 'fixed';
        if (!holds || (acs.overflowX === 'visible' && acs.overflowY === 'visible')) continue;
        if (scrolls(acs)) {
          scrolled = true;
          break;
        }
        const r = a.getBoundingClientRect();
        const dx = clips(acs.overflowX) ? Math.max(r.left - t.left, t.right - r.right) : 0;
        const dy = clips(acs.overflowY) ? Math.max(r.top - em.top, em.bottom - r.bottom) : 0;
        const d = Math.max(dx, dy);
        if (d <= 1) continue;
        const outside = t.right <= r.left || t.left >= r.right || em.bottom <= r.top || em.top >= r.bottom;
        const ellipsis = acs.textOverflow === 'ellipsis' && dx > 1 && dy <= 1;
        push(ellipsis ? (titled(el) || titled(a) ? 'ellipsis' : 'ellipsis-bare') : outside ? 'hidden' : 'cut', `${Math.round(d)}px past ${name(a)}`, a);
        reported = true;
        break;
      }
      if (!scrolled && !reported && (t.right > W + 1 || t.left < -1)) push('off-screen', `${Math.round(t.left)}..${Math.round(t.right)} vs ${W}`);
    }
    return out as Finding[];
  }, allow);
}
