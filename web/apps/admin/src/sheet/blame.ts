/**
 * Blame lines in people's words. The server names fields as the domain does
 * (docs/v5/history.md): `slot`, `bosses`, `participants`, `channel`,
 * `status`, `status_pin`, `rsvp:<member id>`, `attended:<member id>`.
 */
import { answerWord, shortAt, statusWord, type Names } from '../history/describe';

const FIELD: Record<string, string> = {
  slot: 'Day and time',
  bosses: 'Bosses',
  participants: 'Roster',
  channel: 'Home channel',
  status: 'Status',
  status_pin: 'Held status',
};

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj | null => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Obj) : null);

export function blameField(field: string, names: Names): string {
  const known = FIELD[field];
  if (known) return known;
  const [kind, id] = field.split(/:(.*)/s);
  if (kind === 'rsvp' && id) return `${names(id)}'s answer`;
  if (kind === 'attended' && id) return `${names(id)}'s attendance`;
  // A field this app does not know yet still shows, under its own name.
  return field;
}

export function blameValue(field: string, value: unknown, timeZone: string): string {
  if (value === null || value === undefined) return 'cleared';
  const kind = field.split(':')[0];
  const row = obj(value);
  if (field === 'slot' && row) return row.datetime ? shortAt(String(row.datetime), timeZone) : 'no time';
  if (field === 'status') return statusWord(value);
  if (field === 'status_pin' && row) return statusWord(row.status);
  if (kind === 'rsvp' && row) return answerWord(row.state);
  if (kind === 'attended' && row) return row.attended ? 'attended' : 'missed';
  if (field === 'bosses' && Array.isArray(value)) return value.join(' + ');
  if (Array.isArray(value)) return `${value.length} ${value.length === 1 ? 'person' : 'people'}`;
  return row ? JSON.stringify(row) : String(value);
}
