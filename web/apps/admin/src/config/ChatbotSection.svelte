<!-- Config → Chatbot (B_Config): the on/off switch applies at once; the answer limits save with the bar. -->
<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import { changes } from './dirty';
  import { followSaved } from './follow.svelte';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';
  import SwitchCard from './SwitchCard.svelte';

  let { chatbot, save }: { chatbot: ConfigView['chatbot']; save: Save } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  let member = $state({ ...chatbot.member_rate });
  // svelte-ignore state_referenced_locally
  let guild = $state({ ...chatbot.guild_rate });
  followSaved(
    () => ({ member: { ...chatbot.member_rate }, guild: { ...chatbot.guild_rate } }),
    () => ({ member, guild }),
    (saved) => ({ member, guild } = saved),
  );
  let error = $state('');
  let saving = $state(false);

  const RATES = [
    { key: 'member', who: 'Per person', count: 'Answers per person', window: 'Their window (s)' },
    { key: 'guild', who: 'Whole guild', count: 'Answers per guild', window: 'Its window (s)' },
  ] as const;
  const draft = $derived({ member, guild });
  const saved = $derived({ member: chatbot.member_rate, guild: chatbot.guild_rate });
  const pending = $derived(
    changes(
      RATES.flatMap(({ key, who }) => [
        { label: `${who} answers`, from: saved[key].count, to: draft[key].count },
        { label: `${who} window`, from: saved[key].window_s, to: draft[key].window_s },
      ]),
    ),
  );
  const perHour = (r: { count: number; window_s: number }) =>
    r.count > 0 && r.window_s > 0 ? `≈ ${Math.round((r.count * 3600) / r.window_s)} per hour` : '';
  function was(key: 'member' | 'guild'): string {
    const a = saved[key];
    const b = draft[key];
    const old = [a.count !== b.count ? `${a.count} answers` : '', a.window_s !== b.window_s ? `${a.window_s} s` : ''].filter(Boolean);
    return old.length ? `was ${old.join(', ')}` : '';
  }

  function discard() {
    member = { ...chatbot.member_rate };
    guild = { ...chatbot.guild_rate };
    error = '';
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    error = await save({ chatbot: { member_rate: member, guild_rate: guild } }, 'Answer limits saved.');
    saving = false;
  }
</script>

<SettingsPanel title="Chatbot">
  {#snippet lead()}Its models are under <a href="/config?section=models">Models</a>; who it answers is on <a href="/members">Members</a>.{/snippet}
  {#if !chatbot.configured}
    <p class="settings__box settings__box--risk" role="alert">
      Not configured — set {#each chatbot.missing_env as name, i (i)}{i > 0 ? ' and ' : ''}<code>{name}</code>{/each} in the
      environment and restart. Until both are set the bot answers nobody.
      {#if chatbot.missing_env.includes('KANADE_CHAT_CATEGORY_IDS')}Chat categories can also be saved in <a href="/config?section=channels">Channels</a>, with no restart.{/if}
    </p>
  {/if}
  <SwitchCard
    title={(on) => `Chatbot is ${on ? 'on' : 'off'}`}
    chip={false}
    on={chatbot.enabled}
    disabled={!chatbot.configured}
    confirm="Turn the chatbot off?"
    action={(next) => (next ? 'Turn the chatbot on' : 'Turn the chatbot off')}
    apply={(on) =>
      save({ chatbot: { enabled: on } }, on ? 'The chatbot is on.' : 'The chatbot is off.', {
        patch: { chatbot: { enabled: !on } },
        done: on ? 'The chatbot is off again.' : 'The chatbot is on again.',
      })}
  >
    {chatbot.enabled ? 'Members can @mention the bot and it answers in the channel.' : 'Nobody gets an answer until it is turned back on.'}
  </SwitchCard>
  <!-- One line per limit, read as a sentence: "Per person  [4] answers every [300] seconds". -->
  <form class="settings__card" data-fid="cfg-card" id="{uid}-form" onsubmit={submit} aria-labelledby="{uid}-rates">
    <p class="settings__cardtitle" id="{uid}-rates">How often it answers</p>
    {#each RATES as rate (rate.key)}
      {@const r = rate.key === 'member' ? member : guild}
      {@const s = saved[rate.key]}
      <div class="rate" data-fid="cfg-rate" role="group" aria-label={rate.who}>
        <span class="rate__who">{rate.who}</span>
        <input
          class="rate__field"
          class:settings__changed={r.count !== s.count}
          type="number"
          min="1"
          max="100"
          bind:value={r.count}
          aria-label={rate.count}
        />
        <span class="rate__unit">answers every</span>
        <input
          class="rate__field rate__field--wide"
          class:settings__changed={r.window_s !== s.window_s}
          type="number"
          min="10"
          max="86400"
          bind:value={r.window_s}
          aria-label={rate.window}
        />
        <span class="rate__unit">seconds</span>
        <span class="rate__hint">{[was(rate.key), perHour(r)].filter(Boolean).join(' · ')}</span>
      </div>
    {/each}
    <p class="settings__cardnote">Per-person overrides and live windows are on <a href="/limits">Limits</a>.</p>
    {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  </form>
  {#snippet bar()}
    <SaveBar form="{uid}-form" label="Save chatbot" changes={pending} {saving} ondiscard={discard} />
  {/snippet}
</SettingsPanel>
