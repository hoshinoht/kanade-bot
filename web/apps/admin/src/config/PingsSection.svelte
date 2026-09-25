<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import { PendingLabel } from '@kanade/ui';
  import type { Save } from './save';

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

<h3 class="settings__title">Pings</h3>
<form class="filters" onsubmit={submit} aria-describedby="{uid}-note">
  <label class="field"><span>Morning ping</span><input class="mono" bind:value={time} size="6" inputmode="numeric" autocomplete="off" aria-invalid={badTime} /></label>
  <label class="field"><span>Countdowns (minutes)</span><input class="mono" bind:value={countdowns} size="10" autocomplete="off" aria-invalid={badCountdowns} /></label>
  <button class="btn btn--primary" type="submit"><PendingLabel pending={saving} label="Saving…">Save</PendingLabel></button>
</form>
<p class="field__error" role="alert">{error}</p>
<p class="note" id="{uid}-note">The server applies pings when the bot restarts; then every ping that has not fired yet is re-placed.</p>
