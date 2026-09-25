// Revertible design experiments (docs/v5/pwa-design-guidelines.md "Experiments").
// One switch: `?experiments=on|off` (remembered) or the palette command; off
// restores today's behaviour at every usage site.

const KEY = 'kanade.experiments';
/** Default while the experiments are under review. */
const DEFAULT_ON = true;

class Experiments {
  on = $state(DEFAULT_ON);
}

export const experiments = new Experiments();

function reflect(on: boolean): void {
  if (typeof document !== 'undefined') document.documentElement.dataset.experiments = on ? 'on' : 'off';
}

function remember(on: boolean): void {
  try {
    localStorage.setItem(KEY, on ? 'on' : 'off');
  } catch {
    // Storage can be unavailable; the choice then lasts for this page only.
  }
}

function recalled(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

/** Reads the query override (and remembers it), else the stored choice, else the default. */
export function initExperiments(search: string = typeof location === 'undefined' ? '' : location.search): boolean {
  const asked = /(?:^|[?&])experiments=(on|off)(?:&|$)/.exec(search)?.[1];
  let on: boolean;
  if (asked === 'on' || asked === 'off') {
    on = asked === 'on';
    remember(on);
  } else {
    const stored = recalled();
    on = stored === 'on' ? true : stored === 'off' ? false : DEFAULT_ON;
  }
  experiments.on = on;
  reflect(on);
  return on;
}

export function setExperiments(on: boolean): void {
  experiments.on = on;
  remember(on);
  reflect(on);
}
