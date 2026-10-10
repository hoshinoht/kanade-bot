/**
 * The bit of Discord markdown the bot's cards use: `**bold**` runs and line
 * breaks. Anything else stays literal text (mentions are left in for
 * `Mentions` to name).
 */
export type Run = { bold: boolean; text: string };

/** A run's leading whitespace apart from the rest (`Mentions` trims its start). */
export function lead(text: string): [string, string] {
  const space = /^\s*/.exec(text)![0];
  return [space, text.slice(space.length)];
}

/** One card text as lines of bold/plain runs; an unclosed `**` stays literal. */
export function cardLines(text: string): Run[][] {
  return text.split('\n').map((line) => {
    const parts = line.split('**');
    if (parts.length % 2 === 0) return [{ bold: false, text: line }];
    return parts.map((part, i) => ({ bold: i % 2 === 1, text: part })).filter((run) => run.text !== '');
  });
}

/** One line of card text: bold/plain runs, `sub` for a `-# ` subtext line. */
export type Line = { sub: boolean; runs: Run[] };

const RELATIVE: [Intl.RelativeTimeFormatUnit, number][] = [
  ['day', 86_400],
  ['hour', 3_600],
  ['minute', 60],
  ['second', 1],
];

/**
 * Discord timestamps (`<t:unix:t|R|F>`) as Discord draws them for the
 * reader: `t` the clock and `F` the full date in `zone`, `R` relative to
 * `now` (the server's clock, never the browser's).
 */
export function stamps(text: string, now: number, zone?: string): string {
  return text.replace(/<t:(-?\d+):([tRF])>/g, (_, unix: string, style: string) => {
    const at = Number(unix) * 1000;
    if (style === 'R') {
      const seconds = Math.round((at - now) / 1000);
      const [unit, size] = RELATIVE.find(([, size]) => Math.abs(seconds) >= size) ?? RELATIVE[3]!;
      return new Intl.RelativeTimeFormat('en', { numeric: 'always' }).format(Math.trunc(seconds / size), unit);
    }
    const options: Intl.DateTimeFormatOptions =
      style === 't'
        ? { hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZone: zone }
        : { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZone: zone };
    return new Intl.DateTimeFormat('en-GB', options).format(at);
  });
}

/** Card text as lines, timestamps drawn and `-# ` lines marked as subtext. */
export function cardText(text: string, now: number, zone?: string): Line[] {
  return stamps(text, now, zone)
    .split('\n')
    .map((line) => {
      const sub = line.startsWith('-# ');
      return { sub, runs: cardLines(sub ? line.slice(3) : line)[0] ?? [] };
    });
}
