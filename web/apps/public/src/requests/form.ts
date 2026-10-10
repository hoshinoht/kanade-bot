// "Ask for a change" (boards RequestForm*, PhoneRequest, Requests*): the
// request kinds, the draft a link or a press prefills (`?run=`, `?kind=`,
// `?fixed=&change=edit|remove&day=&time=` as Discord sends), the runs and
// weekly timings each kind may name, the body the contract takes, the words
// the summary and the list show, and the address a fresh sign-in returns to
// with the draft kept. Pure.
import type { Boss, Member, MemberRequest, MemberRequestOptions, MemberRequests, MemberRun, MemberTiming, MemberWeek } from '@kanade/api-types';
import { dayLabel, longDate } from '@kanade/ui';
import { safeNext } from '../landing';
import { tokenWords, WEEKDAYS, whenWords } from '../timings/ownership';

export type Kind = MemberRequest['kind'];
export type State = MemberRequest['state'];

export const KINDS: { id: Kind; label: string }[] = [
  { id: 'join', label: 'Join a run' },
  { id: 'leave', label: 'Leave a run' },
  { id: 'swap', label: 'Swap my place' },
  { id: 'new_fixed', label: 'New weekly run' },
  { id: 'change_fixed', label: 'Change a weekly run' },
];

/** The detail's overline: "Weekly change · sent today 11:02". */
export const KIND_NOUNS: Record<Kind, string> = {
  join: 'Join request',
  leave: 'Leave request',
  swap: 'Swap request',
  new_fixed: 'New weekly run',
  change_fixed: 'Weekly change',
};

/** `POST /api/public/requests`. */
export interface RequestBody {
  kind: Kind;
  run_id?: string;
  fixed_id?: string;
  with?: string;
  day?: number;
  time?: string;
  channel_id?: string;
  party?: string[];
  bosses?: string[];
  note?: string;
}

/** The form's state: a run (`run:<id>`) or a weekly timing (`fixed:<id>`) as the subject. */
export interface Draft {
  kind: Kind;
  subject: string;
  with: string;
  /** Weekly timings' weekday, Monday = 0. */
  day: number | null;
  time: string;
  /** A channel id from `options.channels`. */
  channel: string;
  /** Member ids from `options.members`; on a weekly change, empty keeps the party. */
  party: string[];
  /** Boss tokens as typed ("HLimbo, HStar"); a new weekly run only. */
  bosses: string;
  note: string;
}

export const EMPTY: Draft = { kind: 'leave', subject: '', with: '', day: null, time: '', channel: '', party: [], bosses: '', note: '' };

/** The server's cap on a request's note (stored as the request's draft title), in characters. */
export const NOTE_MAX = 200;

/**
 * The note as the server stores it: trimmed, with no control characters (a
 * line break typed into the box becomes a space, since the title is one line).
 */
export const cleanNote = (note: string): string =>
  note
    .replace(/\p{Cc}+/gu, ' ')
    .replace(/ {2,}/g, ' ')
    .trim();

const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

/** "thu", "Thursday", "3" → 3 (Monday = 0); null when it names no day. */
export function weekdayOf(text: string | null): number | null {
  if (!text) return null;
  if (/^[0-6]$/.test(text)) return Number(text);
  const index = WEEKDAYS.findIndex((d) => text.toLowerCase().startsWith(d.toLowerCase()));
  return index >= 0 ? index : null;
}

/**
 * The draft an address asks for. `?run=` picks the run (Join for someone
 * else's, else Leave); `?fixed=&change=edit` a weekly change with the
 * link's day and time, `change=remove` leaving that weekly timing; the rest
 * of the query is a draft kept across a fresh sign-in.
 */
export function prefill(params: URLSearchParams, isMine: (runId: string) => boolean): Draft {
  const draft: Draft = { ...EMPTY, party: [] };
  const kind = KINDS.find((k) => k.id === params.get('kind'))?.id;
  const run = params.get('run');
  const fixed = params.get('fixed');
  if (run) {
    draft.subject = `run:${run}`;
    draft.kind = kind ?? (isMine(run) ? 'leave' : 'join');
  } else if (fixed) {
    draft.subject = `fixed:${fixed}`;
    const change = params.get('change');
    draft.kind = kind ?? (change === 'remove' ? 'leave' : 'change_fixed');
  } else if (kind) draft.kind = kind;
  draft.day = weekdayOf(params.get('day'));
  const time = params.get('time') ?? '';
  draft.time = TIME.test(time) ? time : '';
  draft.with = params.get('with') ?? '';
  draft.channel = params.get('channel') ?? '';
  draft.party = (params.get('party') ?? '').split(',').filter(Boolean);
  draft.bosses = params.get('bosses') ?? '';
  draft.note = params.get('note') ?? '';
  return draft;
}

/** Whether an address's draft names a run whose kind depends on the member's own runs (read before prefilling). */
export const needsWeeks = (params: URLSearchParams): boolean => params.has('run') && !params.has('kind');

/**
 * The body for a draft, or what is still missing. `me` joins the party of a
 * weekly change (leaving is its own request); a new weekly run adds the
 * requester on the server.
 */
export function bodyOf(draft: Draft, me = ''): { body: RequestBody } | { missing: string } {
  const text = cleanNote(draft.note);
  const note = text ? { note: text } : {};
  const [type, id] = draft.subject.split(/:(.*)/s) as [string, string | undefined];
  const subject = type === 'run' && id ? { run_id: id } : type === 'fixed' && id ? { fixed_id: id } : null;
  if ([...text].length > NOTE_MAX) return { missing: `Keep the note to ${NOTE_MAX} characters.` };
  switch (draft.kind) {
    case 'join':
    case 'leave':
      return subject ? { body: { kind: draft.kind, ...subject, ...note } } : { missing: 'Pick a run.' };
    case 'swap':
      if (!subject) return { missing: 'Pick a run.' };
      return draft.with ? { body: { kind: 'swap', ...subject, with: draft.with, ...note } } : { missing: 'Pick who takes your place.' };
    case 'change_fixed': {
      if (type !== 'fixed' || !id) return { missing: 'Pick a weekly run.' };
      if (draft.time && !TIME.test(draft.time)) return { missing: 'Write the time as HH:MM.' };
      const party = draft.party.length ? (me && !draft.party.includes(me) ? [me, ...draft.party] : draft.party) : [];
      const fields = {
        ...(draft.day !== null ? { day: draft.day } : {}),
        ...(draft.time ? { time: draft.time } : {}),
        ...(draft.channel ? { channel_id: draft.channel } : {}),
        ...(party.length ? { party } : {}),
      };
      return Object.keys(fields).length ? { body: { kind: 'change_fixed', fixed_id: id, ...fields, ...note } } : { missing: 'Change the day, time, channel or party.' };
    }
    case 'new_fixed': {
      const bosses = draft.bosses.split(/[\s,+]+/).filter(Boolean);
      if (!bosses.length) return { missing: 'Name the bosses, for example HLimbo.' };
      if (draft.day === null || !TIME.test(draft.time)) return { missing: 'Pick a day and a time (HH:MM).' };
      if (!draft.channel) return { missing: 'Pick the party channel.' };
      return { body: { kind: 'new_fixed', bosses, day: draft.day, time: draft.time, channel_id: draft.channel, ...(draft.party.length ? { party: draft.party } : {}), ...note } };
    }
  }
}

/**
 * The form again after a fresh sign-in: the draft in the address, the note
 * left out when the whole would be longer than a sign-in may carry.
 */
export function draftReturn(draft: Draft): { path: string; noteKept: boolean } {
  const [type, id] = draft.subject.split(/:(.*)/s) as [string, string | undefined];
  const query = new URLSearchParams({ kind: draft.kind });
  if (type === 'run' && id) query.set('run', id);
  if (type === 'fixed' && id) query.set('fixed', id);
  if (draft.with) query.set('with', draft.with);
  if (draft.day !== null) query.set('day', String(draft.day));
  if (draft.time) query.set('time', draft.time);
  if (draft.channel) query.set('channel', draft.channel);
  if (draft.party.length) query.set('party', draft.party.join(','));
  if (draft.bosses) query.set('bosses', draft.bosses);
  const bare = `/requests/new?${query}`;
  const note = cleanNote(draft.note);
  if (!note) return { path: bare, noteKept: true };
  query.set('note', note);
  const full = `/requests/new?${query}`;
  return safeNext(full) === full ? { path: full, noteKept: true } : { path: bare, noteKept: false };
}

/** "Fri 22:00": a weekly timing, Monday = 0. */
export const weekly = (day: number, time: string): string => whenWords({ weekday: day, time });

/** "Leave HLimbo", "Change weekly XKalos": a request's headline. */
export function headline(kind: Kind, bosses: string): string {
  switch (kind) {
    case 'join':
      return `Join ${bosses}`;
    case 'leave':
      return `Leave ${bosses}`;
    case 'swap':
      return `Swap my place on ${bosses}`;
    case 'new_fixed':
      return `New weekly ${bosses}`;
    case 'change_fixed':
      return `Change weekly ${bosses}`;
  }
}

/** "Sun 11 Oct 20:00" for a run of a week; "own time" runs keep their day only. */
export function runWhen(run: Pick<MemberRun, 'day' | 'time'>, week: Pick<MemberWeek, 'days'>): string {
  const day = week.days[run.day];
  const date = day ? `${day.dow} ${longDate(day.date)}` : `day ${run.day + 1}`;
  return run.time ? `${date} ${run.time}` : date;
}

/** What a request proposes for a weekly timing: "Fri 21:30 → 21:00", "→ Sat · #kalos-four". */
export function proposalWords(request: Pick<MemberRequest, 'proposed'>, timing: Pick<MemberTiming, 'weekday' | 'time'> | null): string {
  const p = request.proposed;
  if (!p) return '';
  const to: string[] = [];
  if (p.day !== null && p.time !== null) to.push(weekly(p.day, p.time));
  else if (p.day !== null) to.push(WEEKDAYS[p.day] ?? '');
  else if (p.time !== null) to.push(p.time);
  if (p.channel) to.push(p.channel);
  if (p.party) to.push(p.party.map((m: Member) => m.name).join(', '));
  const from = timing ? `${weekly(timing.weekday, timing.time)} ` : '';
  return `${from}→ ${to.join(' · ')}`;
}

/** What a draft proposes for a weekly timing, in the shape a sent request reads back (for `proposalWords`). */
export function draftProposal(draft: Draft, options: MemberRequestOptions | null): MemberRequest['proposed'] {
  return {
    day: draft.day,
    time: TIME.test(draft.time) ? draft.time : null,
    channel: draft.channel ? (options?.channels.find((c) => c.id === draft.channel)?.name ?? draft.channel) : null,
    party: draft.party.length ? draft.party.map((id) => ({ id, name: options?.members.find((m) => m.id === id)?.name ?? id })) : null,
  };
}

export const STATE_WORDS: Record<State, string> = {
  waiting: 'waiting',
  approved: 'approved',
  rejected: 'declined',
  withdrawn: 'withdrawn',
  expired: 'expired',
};

/** The state chip's tone: waiting outlined (dashed), approved and declined washed, the rest plain. */
export const STATE_TONES: Record<State, 'warn' | 'ok' | 'risk' | ''> = {
  waiting: 'warn',
  approved: 'ok',
  rejected: 'risk',
  withdrawn: '',
  expired: '',
};

/** A run or weekly timing the form may name, as its "Which run" row shows it. */
export interface SubjectChoice {
  /** `run:<id>` or `fixed:<id>` (the draft's `subject`). */
  value: string;
  bosses: Boss[];
  title: string;
  /** "Sun 11 · 20:00", or "every Fri 22:00" for a weekly timing. */
  when: string;
  /** "Sun 11 Oct 20:00" (the summary's line). */
  long: string;
  channel: string;
  next: boolean;
  /** Why it cannot be picked for this kind ("not in this run · use Join"); empty when it can. */
  blocked: string;
}

/**
 * The rows "Which run" lists for a kind: Join the open runs the member is not
 * in; Leave and Swap their own (and a weekly timing a link named); a weekly
 * change their weekly timings; a new weekly run none. A run the draft names
 * that this kind cannot take stays in the list, blocked, saying which kind can.
 */
export function subjectChoices(kind: Kind, weeks: { this: MemberWeek | null; next: MemberWeek | null }, timings: MemberTiming[], subject: string): SubjectChoice[] {
  if (kind === 'new_fixed') return [];
  const weeklies: SubjectChoice[] = timings.map((t) => {
    const when = `every ${whenWords(t)}`;
    return { value: `fixed:${t.id}`, bosses: t.bosses, title: tokenWords(t), when, long: when, channel: '', next: false, blocked: '' };
  });
  if (kind === 'change_fixed') return weeklies;
  const list: SubjectChoice[] = kind === 'join' ? [] : weeklies.filter((c) => c.value === subject);
  for (const which of ['this', 'next'] as const) {
    const week = weeks[which];
    if (!week) continue;
    const runs = week.runs
      .filter((run) => run.status !== 'done' && run.status !== 'cancelled')
      .sort((a, b) => a.day - b.day || (a.time ?? '99').localeCompare(b.time ?? '99') || a.id.localeCompare(b.id));
    for (const run of runs) {
      const fits = kind === 'join' ? !run.mine : run.mine;
      if (!fits && `run:${run.id}` !== subject) continue;
      list.push({
        value: `run:${run.id}`,
        bosses: run.bosses,
        title: tokenWords(run),
        when: `${dayLabel(week, run.day)} · ${run.time ?? 'own time'}`,
        long: runWhen(run, week),
        channel: run.channel,
        next: which === 'next',
        blocked: fits ? '' : run.mine ? "you're in this run · use Leave" : 'not in this run · use Join',
      });
    }
  }
  return list;
}

/** The subject a kind keeps when the member switches to it: the same one if that kind can take it, else none. */
export function keptSubject(choices: SubjectChoice[], subject: string): string {
  return choices.some((c) => c.value === subject && !c.blocked) ? subject : '';
}

/** The summary's last words: who stays where until an admin decides, and when the request lapses. */
export function asideWords(kind: Kind): string {
  switch (kind) {
    case 'join':
      return 'You join the party once an admin approves it. It expires when that boss week ends.';
    case 'leave':
    case 'swap':
      return 'You stay in the party until an admin approves it. It expires when that boss week ends.';
    case 'change_fixed':
      return 'The weekly timing stays as it is until an admin approves it. It expires when this boss week ends.';
    case 'new_fixed':
      return 'Nothing is scheduled until an admin approves it. It expires when this boss week ends.';
  }
}

/** A waiting request's line: what holds until an admin decides, and when it lapses. */
export function waitingWords(kind: Kind, timing: string, expires: string): string {
  const holds =
    kind === 'join'
      ? "You're not in the party until one approves"
      : kind === 'change_fixed'
        ? `The weekly timing stays ${timing || 'as it is'} until one decides`
        : kind === 'new_fixed'
          ? 'Nothing is scheduled until one approves'
          : 'You stay in the party until one decides';
  return `Waiting on the admins. ${holds}, and Discord mentions you when they do.${expires ? ` It expires ${expires} if nobody decides.` : ''}`;
}

/** The title bar's count: "1 of 3 open · 1 of 6 today". */
export const counterWords = (data: Pick<MemberRequests, 'open' | 'max_open' | 'today' | 'max_today'>): string =>
  `${data.open} of ${data.max_open} open · ${data.today} of ${data.max_today} today`;

/** A reached limit, as the form's disabled key explains it; null while a request may be sent. */
export function limitOf(data: Pick<MemberRequests, 'open' | 'max_open' | 'today' | 'max_today'>): { limit: 'open' | 'today'; lead: string; text: string } | null {
  if (data.open >= data.max_open)
    return { limit: 'open', lead: `${data.open} requests are already waiting.`, text: 'Withdraw one in My requests, or send this once an admin decides.' };
  if (data.today >= data.max_today)
    return { limit: 'today', lead: `You've sent ${data.today} requests in the last day.`, text: 'Withdrawn ones count too. Send this once the oldest is a day old.' };
  return null;
}

/** The toast when the server refuses a send at a limit. */
export const limitToast = (limit: 'open' | 'today', data: Pick<MemberRequests, 'max_open' | 'max_today'> | null): string =>
  limit === 'open' ? `Limit reached: ${data?.max_open ?? 3} requests waiting. Nothing was sent.` : `Limit reached: ${data?.max_today ?? 6} requests today. Nothing was sent.`;

/** The stepper: Sent → Admin review → Decided, its last step named for how it ended. */
export function stepsOf(state: State): { last: 'todo' | 'done' | 'gone'; reviewed: boolean; label: string; words: string } {
  switch (state) {
    case 'waiting':
      return { last: 'todo', reviewed: false, label: 'Decided', words: 'Sent; waiting for an admin to decide.' };
    case 'approved':
      return { last: 'done', reviewed: true, label: 'Approved', words: 'Sent, reviewed and approved.' };
    case 'rejected':
      return { last: 'done', reviewed: true, label: 'Declined', words: 'Sent, reviewed and declined.' };
    case 'withdrawn':
      return { last: 'gone', reviewed: false, label: 'Withdrawn', words: 'Sent, then withdrawn before a decision.' };
    case 'expired':
      return { last: 'gone', reviewed: false, label: 'Expired, not decided', words: 'Sent; it expired before an admin decided.' };
  }
}

function parts(iso: string, timeZone: string): Record<string, string> | null {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  try {
    const format = new Intl.DateTimeFormat('en-US', { timeZone, weekday: 'short', day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
    return Object.fromEntries(format.formatToParts(date).map((p) => [p.type, p.value]));
  } catch {
    return null;
  }
}

/** "today 11:02", "Wed 30 Sep 19:40" (or "Sat 3 Oct" without the clock) in the guild's zone; `now` is the server's clock. */
export function instantWords(iso: string, timeZone: string, now: string, clock = true): string {
  const at = parts(iso, timeZone);
  if (!at) return iso;
  const today = parts(now, timeZone);
  const time = clock ? ` ${at.hour}:${at.minute}` : '';
  if (today && at.year === today.year && at.month === today.month && at.day === today.day) return `today${time}`;
  return `${at.weekday} ${at.day} ${at.month}${time}`;
}

/** The list row's second line after the when: who decided, the reason, or when it was sent. */
export function rowTail(request: Pick<MemberRequest, 'state' | 'sent_at' | 'decided_by' | 'reason'>, timeZone: string, now: string): string {
  switch (request.state) {
    case 'waiting':
      return `sent ${instantWords(request.sent_at, timeZone, now)}`;
    case 'approved':
      return request.decided_by ? `by ${request.decided_by}` : 'approved';
    case 'rejected':
      return request.reason ? `${request.decided_by ?? 'an admin'}: “${request.reason}”` : `declined by ${request.decided_by ?? 'an admin'}`;
    case 'withdrawn':
      return 'by you';
    case 'expired':
      return 'not decided in time';
  }
}
