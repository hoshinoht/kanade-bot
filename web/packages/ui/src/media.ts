/**
 * The phone frame: narrow screens, and phone landscape (a 96 px rail of eleven
 * destinations does not fit under 500 px of height). Selected rows never grow here.
 */
export const PHONE_QUERY = '(max-width: 599px), (max-height: 500px)';

/**
 * List-detail screens show one pane at a time below 900 px and list + detail
 * from 900 px (user decision 2026-10-05); range syntax leaves no gap at
 * fractional widths. SCSS twin: `$single-pane` /
 * `$two-pane` in styles/_breakpoints.scss; change both together.
 */
export const SINGLE_PANE_QUERY = '(width < 900px)';
export const TWO_PANE_QUERY = '(width >= 900px)';
