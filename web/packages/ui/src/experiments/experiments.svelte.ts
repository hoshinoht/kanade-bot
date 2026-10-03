// Revertible design experiments (docs/v5/pwa-design-guidelines.md "Experiments").
// One switch: `?experiments=on|off` (remembered) or the palette command; off
// restores today's behaviour at every usage site.

const KEY = 'kanade.experiments';
/** Default while the experiments are under review. */
const DEFAULT_ON = true;

/** Experiment E (planner overshoot) is opt-in on top of the switch, while the user decides. */
const OVERSHOOT_KEY = 'kanade.overshoot';

class Experiments {
  on = $state(DEFAULT_ON);
  /** The user's opt-in; it applies only while the switch is on (see `overshoot`). */
  overshootChosen = $state(false);

  /** Experiment E: the planner's pick-up and landing overshoot. Off by default. */
  get overshoot(): boolean {
    return this.on && this.overshootChosen;
  }
}

export const experiments = new Experiments();

function reflect(on: boolean): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.experiments = on ? 'on' : 'off';
  document.documentElement.dataset.overshoot = experiments.overshootChosen ? 'on' : 'off';
}

function remember(on: boolean, key = KEY): void {
  try {
    localStorage.setItem(key, on ? 'on' : 'off');
  } catch {
    // Storage can be unavailable; the choice then lasts for this page only.
  }
}

function recalled(key = KEY): string | null {
  try {
    return localStorage.getItem(key);
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
  const overshoot = /(?:^|[?&])overshoot=(on|off)(?:&|$)/.exec(search)?.[1];
  if (overshoot === 'on' || overshoot === 'off') remember(overshoot === 'on', OVERSHOOT_KEY);
  experiments.overshootChosen = (overshoot ?? recalled(OVERSHOOT_KEY)) === 'on';
  reflect(on);
  return on;
}

export function setExperiments(on: boolean): void {
  experiments.on = on;
  remember(on);
  reflect(on);
}

/** Experiment E opt-in (`?overshoot=on|off` or the palette); it shows only while the switch is on. */
export function setOvershoot(on: boolean): void {
  experiments.overshootChosen = on;
  remember(on, OVERSHOOT_KEY);
  reflect(experiments.on);
}
