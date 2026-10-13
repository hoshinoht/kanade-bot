/**
 * A log window's title-bar text search, debounced into the URL: `text` is the
 * box (bind it), `q` the filter's current text, `apply` writes a new one. A q
 * changed from outside the box (Clear, Back, a deep link) replaces what it shows.
 * Construct during component init: it owns two effects.
 */
export class SearchBox {
  text = $state('');

  constructor(q: () => string, apply: (q: string) => void) {
    let seen = q();
    this.text = seen;
    $effect(() => {
      const text = this.text;
      if (text.trim() === q().trim()) return;
      const timer = setTimeout(() => apply(text), 250);
      return () => clearTimeout(timer);
    });
    $effect(() => {
      const next = q();
      if (next === seen) return;
      seen = next;
      if (next.trim() !== this.text.trim()) this.text = next;
    });
  }
}
