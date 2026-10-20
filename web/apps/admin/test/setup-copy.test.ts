import { readFileSync } from 'node:fs';
import type { SetupItem, SetupReason, SetupView } from '@kanade/api-types';
import { describe as group, expect, it } from 'vitest';
import { botPage, describe, listed, summary } from '../src/setup/copy';
import { requiredLeft } from '../src/setup/status.svelte';

const context = { channelName: (id: string) => (id === '10' ? '#boss-schedule' : null), when: () => 'Tue 29 Sep 09:00', inviteUrl: null };
const item = (fields: Partial<SetupItem> & Pick<SetupItem, 'id' | 'state'>): SetupItem => ({ required: true, ...fields });
const INVITE = 'https://discord.com/oauth2/authorize?client_id=3000000000000000001&scope=bot+applications.commands&permissions=93248';

group('setup copy', () => {
  it('names the intents still off and links the Bot page of the invite link’s application', () => {
    const row = describe(item({ id: 'discord_intents', state: 'missing', missing_intents: ['server_members'] }), { ...context, inviteUrl: INVITE });
    expect(row.line).toBe('Turn on Server Members Intent in the Discord Developer Portal.');
    expect(row.action).toEqual({ kind: 'external', href: 'https://discord.com/developers/applications/3000000000000000001/bot', label: 'Open the Bot page' });
    expect(describe(item({ id: 'discord_intents', state: 'missing', missing_intents: ['server_members', 'message_content'] }), context).line).toBe(
      'Turn on Server Members Intent and Message Content Intent in the Discord Developer Portal.',
    );
    expect(botPage(null)).toBe('https://discord.com/developers/applications');
  });

  it('offers the invite link for missing permissions, counting the channels', () => {
    const row = describe(item({ id: 'bot_access', state: 'missing', channels_lacking: 1, invite_url: INVITE }), context);
    expect(row.line).toBe('1 channel lacks a permission the bot needs; invite it again or fix the channel overrides.');
    expect(row.action).toEqual({ kind: 'external', href: INVITE, label: 'Invite the bot' });
    expect(describe(item({ id: 'bot_access', state: 'missing', channels_lacking: 3 }), context).action).toEqual({ kind: 'config', section: 'access', label: 'Config › Channel access' });
  });

  it('links the Config section that fixes a step, and the runbook for deployment settings', () => {
    expect(describe(item({ id: 'digest_channel', state: 'missing' }), context).action).toEqual({ kind: 'config', section: 'channels', label: 'Config › Channels' });
    expect(describe(item({ id: 'digest_channel', state: 'ok', channel_id: '10' }), context)).toEqual({ name: 'Digest channel', line: 'The weekly digest posts in #boss-schedule.', action: null });
    expect(describe(item({ id: 'discord_gateway', state: 'missing', reason: 'gateway_off' }), context).action).toEqual({ kind: 'runbook', label: 'Set in kanade.toml / the deploy runbook' });
    expect(describe(item({ id: 'backups', state: 'ok', required: false, latest_backup_at: '2026-09-29T01:00:00Z', recipients: false }), context).line).toBe(
      'Backups are on but not encrypted (the latest is from Tue 29 Sep 09:00); add age recipients.',
    );
  });

  it('counts only required steps left', () => {
    const view: SetupView = {
      complete: false,
      items: [item({ id: 'discord_gateway', state: 'missing' }), item({ id: 'model_gateway', state: 'warn' }), item({ id: 'chat', state: 'off', required: false })],
    };
    expect(requiredLeft(view)).toBe(2);
    expect(requiredLeft(null)).toBe(0);
    expect(summary(2)).toBe('2 required steps left');
    expect(summary(1)).toBe('1 required step left');
    expect(summary(0)).toBe('Setup complete');
    expect(listed(['A', 'B', 'C'])).toBe('A, B and C');
  });

  // Every reason the API can send, with each item that sends it (src/api/admin/setup.rs), says its own line.
  const BY_REASON: Record<SetupReason, [Pick<SetupItem, 'id' | 'state'> & Partial<SetupItem>, string][]> = {
    gateway_off: [[{ id: 'discord_gateway', state: 'missing' }, 'Not connected to Discord: check the bot token and guild, then restart the bot.']],
    connecting: [[{ id: 'discord_gateway', state: 'missing' }, 'Connecting to Discord; refresh in a minute.']],
    gateway_closed: [[{ id: 'discord_gateway', state: 'missing' }, 'Discord closed the connection, usually over a missing intent; fix that, then restart the bot.']],
    not_connected: [[{ id: 'bot_access', state: 'missing' }, 'Checked once the bot is connected to Discord.']],
    unavailable: [
      [{ id: 'discord_intents', state: 'warn' }, "Couldn't read the bot's Discord application; refresh to try again."],
      [{ id: 'character_data', state: 'warn' }, "Couldn't read the character data health; refresh to try again."],
      [{ id: 'scouter', state: 'warn' }, "Couldn't read the scouter's health; refresh to try again."],
    ],
    unreachable: [
      [{ id: 'model_gateway', state: 'warn' }, "The model gateway didn't answer in time; check that it is running."],
      [{ id: 'scouter', state: 'warn' }, "The scouter didn't answer."],
    ],
    no_listing: [[{ id: 'model_gateway', state: 'ok' }, 'The model gateway answers; it lists no models, so the declared models are used.']],
    not_running: [[{ id: 'extraction_model', state: 'warn' }, "The saved extraction model isn't running; check its capacity group."]],
    env_missing: [[{ id: 'chat', state: 'warn', missing_env: ['KANADE_BRAVE_API_KEY'] }, 'The chatbot still needs KANADE_BRAVE_API_KEY.']],
    member_sign_in_missing: [[{ id: 'public_portal', state: 'warn' }, 'The member portal needs member Discord sign-in.']],
    nexon_no_key: [[{ id: 'character_data', state: 'warn' }, 'Add a NEXON Open API key to fetch character data.']],
    nexon_key_refused: [[{ id: 'character_data', state: 'warn' }, 'NEXON refused the API key; replace it.']],
    nexon_backoff: [[{ id: 'character_data', state: 'warn' }, 'NEXON is limiting requests; fetches resume on their own.']],
    verification_required: [[{ id: 'scouter', state: 'warn' }, 'The scouter needs a manual check before it answers again.']],
  };

  it('words every reason the API sends', () => {
    // The generated union, read at run time: a new reason fails here until it has its line.
    const generated = readFileSync(new URL('../../../packages/api-types/src/generated.ts', import.meta.url), 'utf8');
    const union = /export type SetupReason = ([^;]+);/.exec(generated)![1]!;
    const reasons = [...union.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
    expect(Object.keys(BY_REASON).sort()).toEqual([...reasons].sort());
    for (const [reason, cases] of Object.entries(BY_REASON) as [SetupReason, (typeof BY_REASON)[SetupReason]][]) {
      for (const [fields, line] of cases) expect(describe(item({ ...fields, reason }), context).line, `${fields.id} ${reason}`).toBe(line);
    }
  });

  it('says the model gateway answers without a model list, ready and with nothing to do', () => {
    const row = describe(item({ id: 'model_gateway', state: 'ok', reason: 'no_listing' }), context);
    expect(row).toEqual({ name: 'Model gateway', line: 'The model gateway answers; it lists no models, so the declared models are used.', action: null });
    expect(describe(item({ id: 'model_gateway', state: 'ok' }), context).line).toBe('The model gateway answers.');
  });
});
