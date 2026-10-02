<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import { changes } from './dirty';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';

  let { pings, save }: { pings: ConfigView['pings']; save: Save } = $props();
  const uid = $props.id();

  // Seeded from the saved values once; a refused save keeps what was typed.
  // svelte-ignore state_referenced_locally
  let time = $state(pings.day_of_ping_time);
  // svelte-ignore state_referenced_locally
  let countdowns = $state(pings.countdown_minutes.join(', '));
  let error = $state('');
  let saving = $state(false);
  // Only the field that failed validation carries aria-invalid.
  let badTime = $state(false);
  let badCountdowns = $state(false);

  const pending = $derived(
    changes([
      { label: 'Morning ping', from: pings.day_of_ping_time, to: time.trim() },
      { label: 'Countdowns', from: pings.countdown_minutes.join(', '), to: countdowns.split(/[\s,]+/).filter(Boolean).join(', ') },
    ]),
  );

  function discard() {
    time = pings.day_of_ping_time;
    countdowns = pings.countdown_minutes.join(', ');
    error = '';
    badTime = badCountdowns = false;
  }

  const validTime = (value: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(value.trim());
  const validCountdowns = (value: string) => {
    const parts = value.split(/[\s,]+/).filter(Boolean);
    return parts.length > 0 && parts.every((n) => /^\d+$/.test(n));
  };

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    badTime = !validTime(time);
    badCountdowns = !validCountdowns(countdowns);
    if (badTime || badCountdowns) {
      error = badTime ? 'The morning ping is HH:MM, for example 09:00.' : 'Countdowns are whole minutes, separated by commas.';
      return;
    }
    const minutes = countdowns
      .split(/[\s,]+/)
      .filter(Boolean)
      .map(Number);
    saving = true;
    error = await save({ pings: { day_of_ping_time: time.trim(), countdown_minutes: minutes } }, 'Pings saved; they take effect when the bot restarts.');
    saving = false;
    badTime = error.includes('HH:MM');
    badCountdowns = !badTime && error.includes('Countdown');
  }
</script>

<SettingsPanel title="Pings">
  {#snippet lead()}Reminder timing for every run.{/snippet}
  <form class="settings__card" data-fid="cfg-card" id="{uid}-form" onsubmit={submit} aria-label="Pings" aria-describedby="{uid}-note">
    <div class="pings">
    <div class="pings__label"><b>Morning ping</b><span class="settings__cardnote">day-of card for every run</span></div>
    <label class="pings__field"
      ><span class="vh">Morning ping</span><input
        class="mono pings__time"
        class:settings__changed={time.trim() !== pings.day_of_ping_time}
        bind:value={time}
        size="6"
        inputmode="numeric"
        autocomplete="off"
        aria-invalid={badTime}
      /><span class="settings__cardnote">guild time · 24-hour</span></label
    >
    <div class="pings__label"><b>Countdowns</b><span class="settings__cardnote">minutes before start, separated by commas</span></div>
    <label class="pings__field"
      ><span class="vh">Countdowns (minutes)</span><input
        class="mono"
        class:settings__changed={pending.some((c) => c.label === 'Countdowns')}
        bind:value={countdowns}
        size="10"
        autocomplete="off"
        aria-invalid={badCountdowns}
      /></label
    >
    </div>
  </form>
  {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  <p class="settings__box" id="{uid}-note">The server applies pings when the bot restarts; then every ping that has not fired yet is re-placed.</p>
  {#snippet bar()}
    <SaveBar form="{uid}-form" label="Save pings" changes={pending} {saving} ondiscard={discard} />
  {/snippet}
</SettingsPanel>

<style>
  .pings {
    display: grid;
    grid-template-columns: 12.5rem minmax(0, 1fr);
    align-items: center;
    gap: 1rem 1.25rem;
  }

  .pings__label b {
    font-size: var(--fs-body-sm);
  }

  .pings__label {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pings__field {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.625rem;
  }

  .pings__time {
    width: 6.25rem;
  }

  @media (max-width: 599px) {
    .pings {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
