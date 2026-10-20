import type { SetupItem, SetupView } from '@kanade/api-types';
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
});
