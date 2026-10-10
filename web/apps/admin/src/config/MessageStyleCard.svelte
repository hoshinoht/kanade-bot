<!--
  Discord message style (Notifications): classic or redesigned reminder and
  digest cards. Applies at once, with Undo on the toast; presentation only.
-->
<script lang="ts">
  import type { MessageStyle } from '@kanade/api-types';
  import type { Save } from './save';

  let { style, save }: { style: MessageStyle; save: Save } = $props();
  const uid = $props.id();

  const STYLES: { id: MessageStyle; label: string }[] = [
    { id: 'classic', label: 'Classic' },
    { id: 'redesigned', label: 'Redesigned' },
  ];
  const label = (id: MessageStyle) => STYLES.find((s) => s.id === id)!.label.toLowerCase();

  let busy = $state(false);
  let error = $state('');

  async function choose(next: MessageStyle) {
    if (busy || next === style) return;
    const before = style;
    busy = true;
    error = await save({ notifications: { message_style: next } }, `Discord messages use the ${label(next)} style.`, {
      patch: { notifications: { message_style: before } },
      done: `Back to the ${label(before)} style.`,
    });
    busy = false;
  }
</script>

<div class="settings__card settings__switchcard" data-fid="cfg-card">
  <div class="settings__switchtext">
    <p class="settings__cardtitle" id="{uid}-t">Discord message style</p>
    <p class="settings__cardnote" id="{uid}-d">
      How reminder and digest cards look in Discord; the attendance mode still decides who is pinged.
    </p>
    {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  </div>
  <div class="seg" role="group" aria-labelledby="{uid}-t" aria-describedby="{uid}-d" data-fid="cfg-message-style">
    {#each STYLES as s (s.id)}
      <button type="button" class="seg__btn" aria-pressed={style === s.id} aria-disabled={busy} onclick={() => void choose(s.id)}>{s.label}</button>
    {/each}
  </div>
</div>
