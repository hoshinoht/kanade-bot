/**
 * One request's typed outcome, for forms and rows that keep their input on
 * failure (the admin app's `send`, the member portal's character routes).
 * `unbuilt`: the server does not serve the route (an unbuilt feature or a
 * module that is off, the generic 404). `reauth`: a 401 `reauth_required`
 * (the portal's fresh-sign-in window). `nextAt`: when a refused member
 * lookup may start (429 `member_limited`).
 */
export type Outcome<T = unknown> =
  | { ok: true; value: T }
  | { ok: false; message: string; status?: number | null; code?: string | null; unbuilt?: boolean; reauth?: boolean; nextAt?: string | null };
