// M3 Expressive motion values for script-driven (Web Animations) motion; the
// CSS copies live in styles/_easing.scss. Keep the two in step.

/** Standard easing for enter/forward motion. */
export const STANDARD = 'cubic-bezier(0.2, 0, 0, 1)';
/** Accelerating exit (M3 standard accelerate). */
export const EXIT = 'cubic-bezier(0.3, 0, 1, 1)';
export const ENTER_MS = 250;
export const EXIT_MS = 200;

/** Standard spatial spring (damping 0.9, stiffness 700), sampled; identical to `$spring`/`$spring-ms` in _easing.scss. */
export const SPRING =
  'linear(0, 0.049, 0.16, 0.295, 0.429, 0.552, 0.658, 0.745, 0.814, 0.868, 0.908, 0.938, 0.959, 0.974, 0.985, 0.992, 0.996, 0.999, 1, 1.001, 1.001, 1.002, 1.001, 1.001, 1)';
export const SPRING_MS = 250;

/** Expressive spatial spring (damping 0.8, stiffness 380), sampled over 500 ms: about 1.5% overshoot (Experiment E). */
export const SPRING_BOUNCY =
  'linear(0, 0.044, 0.149, 0.28, 0.417, 0.546, 0.66, 0.755, 0.832, 0.891, 0.935, 0.967, 0.988, 1.002, 1.01, 1.014, 1.015, 1.015, 1.013, 1.011, 1.009, 1.007, 1.005, 1.004, 1.003, 1.002, 1.001, 1.001, 1, 1, 1)';
export const SPRING_BOUNCY_MS = 500;

/** True when the user asked for reduced motion (and in environments without matchMedia). */
export function reducedMotion(): boolean {
  if (typeof matchMedia !== 'function') return true;
  return matchMedia('(prefers-reduced-motion: reduce)').matches;
}
