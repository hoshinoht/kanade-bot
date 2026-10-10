// Session-list words shared by the admin Account page and the member portal's
// devices: times read against the server's clock, device names and glyphs.

/** "now" within a minute, else "12 min ago" / "3 h ago" against the server's clock. */
export function seenWords(iso: string, now: string): string {
  const ms = Date.parse(now) - Date.parse(iso);
  if (Number.isNaN(ms)) return '';
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return 'now';
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}

/** "2 Oct 21:14" for an instant in `timeZone`. */
export function dayTime(iso: string, timeZone: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const parts = new Intl.DateTimeFormat('en-US', { timeZone, day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(date);
  const part = (type: string) => parts.find((p) => p.type === type)?.value ?? '';
  return `${part('day')} ${part('month')} ${part('hour')}:${part('minute')}`;
}

/** "Firefox · macOS", or a plain fallback when the server did not recognise the browser. */
export function deviceName(session: { device: string | null }): string {
  return session.device ?? 'Unknown browser';
}

/** Phones and tablets get the phone glyph. */
export function isHandheld(session: { device: string | null }): boolean {
  return /iPhone|iPad|Android/.test(session.device ?? '');
}
