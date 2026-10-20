/**
 * Every word the Setup page says. The API sends facts only (ids, states,
 * reasons, counts); this table turns each item into a name, one line for its
 * state and reason, and the one action that fixes it.
 */
import type {
  PrivilegedIntent,
  SecretCheck,
  SecretName,
  SecretSource,
  SetupItem,
  SetupItemId,
  SetupSource,
  SetupState,
} from '@kanade/api-types';
import type { IconName } from '@kanade/ui';

/** Never colour alone: each state has its word and glyph. */
export const STATE: Record<SetupState, { label: string; tone: 'ok' | 'risk' | 'warn' | 'neutral'; icon: IconName }> = {
  ok: { label: 'Ready', tone: 'ok', icon: 'check' },
  missing: { label: 'Missing', tone: 'risk', icon: 'x' },
  warn: { label: 'Check', tone: 'warn', icon: 'alert-triangle' },
  off: { label: 'Off', tone: 'neutral', icon: 'minus' },
};

export const SOURCE: Record<SetupSource, string> = {
  environment: 'Environment',
  file: 'kanade.toml',
  saved: 'Saved in Config',
  derived: 'Derived',
};

export const NAME: Record<SetupItemId, string> = {
  discord_gateway: 'Discord connection',
  discord_intents: 'Privileged intents',
  bot_access: 'Bot permissions',
  digest_channel: 'Digest channel',
  watched_channels: 'Watched channels',
  model_gateway: 'Model gateway',
  extraction_model: 'Extraction model',
  chat: 'Chatbot',
  persona: 'Persona',
  admin_sign_in: 'Admin sign-in',
  public_portal: 'Member portal',
  roster: 'Character roster',
  character_data: 'Character data',
  guides: 'Guides',
  scouter: 'Estimates',
  backups: 'Backups',
};

const INTENT: Record<PrivilegedIntent, string> = {
  server_members: 'Server Members Intent',
  message_content: 'Message Content Intent',
};

/** Outside the app: deployment settings, then host-only steps. */
export const RUNBOOK_TOML = 'Set in kanade.toml / the deploy runbook';
export const RUNBOOK = 'See the deploy runbook';

export type SetupAction =
  /** A Config section (`/config?section=`) that fixes it. */
  | { kind: 'config'; section: string; label: string }
  /** Discord itself, in a new tab. */
  | { kind: 'external'; href: string; label: string }
  /** Nothing in this app can fix it. */
  | { kind: 'runbook'; label: string };

export interface SetupRow {
  name: string;
  line: string;
  action: SetupAction | null;
}

/** What the lines need beyond the item: a channel's name and a time in guild terms. */
export interface CopyContext {
  channelName: (id: string) => string | null;
  when: (iso: string) => string;
  /** `bot_access`'s invite link, which also names the application for `discord_intents`. */
  inviteUrl: string | null;
}

const config = (section: string, page: string): SetupAction => ({ kind: 'config', section, label: `Config › ${page}` });
const runbook = (label = RUNBOOK_TOML): SetupAction => ({ kind: 'runbook', label });
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "A", "A and B", "A, B and C". */
export function listed(words: string[]): string {
  return words.length < 2 ? (words[0] ?? '') : `${words.slice(0, -1).join(', ')} and ${words.at(-1)}`;
}

/** The Developer Portal's Bot page (where the intent switches are) for the invite link's application. */
export function botPage(inviteUrl: string | null): string {
  const id = inviteUrl ? new URL(inviteUrl).searchParams.get('client_id') : null;
  return id && /^\d+$/.test(id) ? `https://discord.com/developers/applications/${id}/bot` : 'https://discord.com/developers/applications';
}

export function describe(item: SetupItem, context: CopyContext): SetupRow {
  const name = NAME[item.id];
  const row = (line: string, action: SetupAction | null = null): SetupRow => ({ name, line, action });
  const { state, reason } = item;
  switch (item.id) {
    case 'discord_gateway':
      if (state === 'ok') return row('Connected to Discord.');
      if (reason === 'connecting') return row('Connecting to Discord; refresh in a minute.');
      if (reason === 'gateway_closed') return row('Discord closed the connection, usually over a missing intent; fix that, then restart the bot.', runbook());
      return row('Not connected to Discord: check the bot token and guild, then restart the bot.', runbook());
    case 'discord_intents': {
      if (state === 'ok') return row('Server Members Intent and Message Content Intent are on.');
      if (state === 'warn') return row("Couldn't read the bot's Discord application; refresh to try again.");
      const off = (item.missing_intents ?? []).map((intent) => INTENT[intent]);
      return row(`Turn on ${listed(off.length ? off : Object.values(INTENT))} in the Discord Developer Portal.`, {
        kind: 'external',
        href: botPage(context.inviteUrl),
        label: 'Open the Bot page',
      });
    }
    case 'bot_access': {
      const invite: SetupAction | null = item.invite_url ? { kind: 'external', href: item.invite_url, label: 'Invite the bot' } : null;
      if (state === 'ok') return row('The bot has all six permissions in every watched and digest channel.');
      if (reason === 'not_connected') return row('Checked once the bot is connected to Discord.', invite);
      const lacking = item.channels_lacking ?? 0;
      return row(`${plural(lacking, 'channel lacks', 'channels lack')} a permission the bot needs; invite it again or fix the channel overrides.`, invite ?? config('access', 'Channel access'));
    }
    case 'digest_channel': {
      if (state !== 'ok') return row('Pick the channel the weekly digest posts in.', config('channels', 'Channels'));
      const channel = item.channel_id ? context.channelName(item.channel_id) : null;
      return row(channel ? `The weekly digest posts in ${channel}.` : 'The weekly digest has a channel.');
    }
    case 'watched_channels':
      if (state !== 'ok') return row('Pick the party channels the bot reads.', config('channels', 'Channels'));
      return row(`Reading ${plural(item.count ?? 0, 'watched channel or category', 'watched channels and categories')}.`);
    case 'model_gateway':
      if (reason === 'no_listing') return row('The model gateway answers; it lists no models, so the declared models are used.');
      if (state === 'ok') return row('The model gateway answers.');
      if (state === 'warn') return row("The model gateway didn't answer in time; check that it is running.", runbook());
      return row('No model gateway is set.', runbook());
    case 'extraction_model':
      if (state === 'ok') return row('A model is reading party chat.');
      if (state === 'warn') return row("The saved extraction model isn't running; check its capacity group.", config('models', 'Models'));
      return row('Pick the model that reads party chat.', config('models', 'Models'));
    case 'chat':
      if (state === 'ok') return row('The chatbot is on and has everything it needs.');
      if (state === 'off') return row('The chatbot is off.', config('chatbot', 'Chatbot'));
      return row(`The chatbot still needs ${listed(item.missing_env ?? [])}.`, runbook());
    case 'persona':
      if (state === 'ok') return row('A persona is active.');
      return row('Pick the persona the bot speaks as.', config('persona', 'Persona'));
    case 'admin_sign_in':
      if (state === 'ok') return row('Admins can sign in with Discord or Tailscale.');
      return row('Only the break-glass token signs in; add Discord sign-in for admins.', runbook());
    case 'public_portal':
      if (state === 'off') return row('No public listener, so the member portal is off.', runbook());
      if (state === 'warn') return row('The member portal needs member Discord sign-in.', runbook());
      if (item.open === false) return row('The member portal is set up but closed.', config('self-service', 'Self-service'));
      return row('The member portal is open.');
    case 'roster':
      return state === 'ok' ? row('Members can keep a character roster.') : row('The character roster module is off.', runbook());
    case 'guides':
      return state === 'ok' ? row('Guides can be published to Discord.') : row('The guides module is off.', runbook());
    case 'character_data': {
      if (state === 'ok') return row('NEXON character data is updating.');
      if (state === 'off') return row('The character data module is off.', runbook());
      const fix = config('character-data', 'Character data');
      if (reason === 'nexon_no_key') return row('Add a NEXON Open API key to fetch character data.', fix);
      if (reason === 'nexon_key_refused') return row('NEXON refused the API key; replace it.', fix);
      if (reason === 'nexon_backoff') return row('NEXON is limiting requests; fetches resume on their own.', fix);
      return row("Couldn't read the character data health; refresh to try again.", fix);
    }
    case 'scouter': {
      if (state === 'ok') return row('The scouter answers estimate lookups.');
      if (state === 'off') return row('No scouter is set, so estimates are off.', runbook());
      const fix = config('character-data', 'Character data');
      if (reason === 'unreachable') return row("The scouter didn't answer.", fix);
      if (reason === 'verification_required') return row('The scouter needs a manual check before it answers again.', fix);
      return row("Couldn't read the scouter's health; refresh to try again.", fix);
    }
    case 'backups': {
      if (state !== 'ok') return row('No backup directory, so no snapshots can be taken.', runbook(RUNBOOK));
      const latest = item.latest_backup_at ? `the latest is from ${context.when(item.latest_backup_at)}` : 'none taken yet';
      if (item.recipients === false) return row(`Backups are on but not encrypted (${latest}); add age recipients.`, runbook(RUNBOOK));
      return row(`Encrypted backups are on; ${latest}.`);
    }
  }
}

/** The page's headline: what is left of the required steps. */
export function summary(left: number): string {
  return left === 0 ? 'Setup complete' : `${plural(left, 'required step', 'required steps')} left`;
}

/** Secrets Setup may save, by what an admin calls them. */
export const SECRET_NAME: Record<SecretName, string> = {
  discord_token: 'Bot token',
  model_api_key: 'Model gateway key',
  discord_client_secret: 'Admin sign-in client secret',
  public_discord_client_secret: 'Member sign-in client secret',
  nexon_api_key: 'NEXON API key',
};

/** Where the secret comes from; a host file can't be replaced here. */
export const SECRET_SOURCE: Record<SecretSource, { label: string; tone: 'ok' | 'neutral'; icon: IconName }> = {
  absent: { label: 'Not set', tone: 'neutral', icon: 'minus' },
  host: { label: 'On the host', tone: 'ok', icon: 'check' },
  saved: { label: 'Saved here', tone: 'ok', icon: 'check' },
};

export const SECRET_HOST_LINE = 'Set by a file in the secrets directory, which always wins; change it there.';

/** What a check found, in one sentence. */
export function checkLine(name: SecretName, check: SecretCheck): string {
  switch (check.result) {
    case 'ok':
      if (name === 'discord_token') {
        const missing = check.missing_intents ?? [];
        const bot = check.bot_name ? `This is ${check.bot_name}` : 'Discord accepts this token';
        return missing.length === 0
          ? `${bot}; both privileged intents are on.`
          : `${bot}, but turn on ${listed(missing.map((intent) => INTENT[intent]))} in the Developer Portal.`;
      }
      if (name === 'model_api_key') {
        const models = check.models ?? 0;
        return models === 0 ? 'The model gateway accepts this key; it lists no models.' : `The model gateway accepts this key and lists ${plural(models, 'model', 'models')}.`;
      }
      return 'Accepted.';
    case 'refused':
      return name === 'discord_token' ? 'Discord refused this token: copy it again from the Bot page.' : 'The model gateway refused this key.';
    case 'unreachable':
      return "Couldn't reach the service to check it; you can still save it.";
    case 'not_checked':
      return "This secret is checked when it's first used.";
  }
}

/** The pending-restart note, with or without "Restart now". */
export function restartLine(available: boolean): string {
  return available
    ? 'Saved secrets are read when Kanade starts: restart it to use them.'
    : 'Saved secrets are read when Kanade starts: restart it (docker compose restart bot) to use them.';
}
