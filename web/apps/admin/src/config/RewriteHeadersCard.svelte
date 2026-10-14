<!--
  Reminder headers (Notifications): rewrite every header already posted this
  boss week. The server queues the run and answers at once; the posts are
  edited in place (nobody is pinged again) and each call lands in the Rewrites log.
-->
<script lang="ts">
  import { Modal, type Toaster } from '@kanade/ui';
  import { send } from '../resource.svelte';

  let { toaster }: { toaster: Toaster } = $props();
  const uid = $props.id();

  let busy = $state(false);
  let error = $state('');
  let asking = $state(false);

  async function rewrite() {
    if (busy) return;
    busy = true;
    const result = await send((c) => c.post<{ message: string }>('/api/admin/headers/rewrite', undefined));
    busy = false;
    error = result.ok ? '' : result.message;
    if (result.ok) toaster.show({ message: result.value.message, tone: 'ok' });
  }
</script>

<section class="settings__card settings__switchcard" data-fid="cfg-card" aria-labelledby="{uid}-t">
  <div class="settings__switchtext">
    <p class="settings__cardtitle" id="{uid}-t">This week's posted headers</p>
    <p class="settings__cardnote" id="{uid}-d">
      Writes new lines for the reminders and digest already posted this boss week and edits them in place; nobody is pinged again.
    </p>
    {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  </div>
  <button type="button" class="btn" aria-describedby="{uid}-d" aria-disabled={busy} onclick={() => (asking = !busy)}
    >Rewrite this week's headers…</button
  >
</section>

<Modal bind:open={asking} title="Rewrite this week's headers?" narrow>
  <p>Every reminder and digest posted this boss week gets a new header line. Lines the checks refuse keep the old one.</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    <button
      class="btn btn--primary"
      type="button"
      onclick={() => {
        close();
        void rewrite();
      }}>Rewrite headers</button
    >
  {/snippet}
</Modal>
