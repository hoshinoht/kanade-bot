<!-- History → Checkpoints (B_HistoryCk): the chain verification card, then the
     backups whose manifests anchor the history. "Verify again" refetches the
     endpoint, which re-checks the chain and every manifest (read only). -->
<script lang="ts">
  import type { BackupAnchor, Checkpoints } from '@kanade/api-types';
  import { Icon, LiveRegion, clockTime } from '@kanade/ui';
  import { onMount } from 'svelte';
  import type { Resource } from '../resource.svelte';
  import { localAt } from './describe';

  let { checkpoints, timezone }: { checkpoints: Resource<Checkpoints>; timezone: string } = $props();

  const ANCHOR: Record<BackupAnchor, { label: string; note: string }> = {
    matches: { label: 'matches', note: 'the history still holds this head' },
    older_schema: { label: 'older schema', note: 'restore it with the image of that schema' },
    mismatch: { label: 'mismatch', note: 'the history no longer holds this head' },
  };

  // Tonal chips for the two settled states; older schema is drawn dashed in _history.
  const TONE: Record<BackupAnchor, string> = { matches: 'status-chip--ok', older_schema: '', mismatch: 'status-chip--risk' };

  let busy = $state(false);
  let checkedAt = $state<number | null>(null);
  let now = $state(Date.now());
  let message = $state('');

  async function verify() {
    if (busy) return;
    busy = true;
    // Cleared first so an unchanged result is announced again.
    message = '';
    await checkpoints.load();
    busy = false;
    const data = checkpoints.data;
    if (checkpoints.error || !data) return;
    checkedAt = now = Date.now();
    const v = data.verified;
    message = v.ok ? `Chain verified: ${records(v.checked)}, head #${v.head.seq}.` : `Chain check failed: the history no longer matches its hash chain${brokenAt(v.first_broken)}.`;
  }

  onMount(() => {
    void verify();
    const id = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(id);
  });

  // Older servers send no `first_broken`; then the failure names no record.
  const brokenAt = (seq: number | null | undefined) => (seq === null || seq === undefined ? '' : ` from record #${seq}`);
  const records = (n: number) => `${n.toLocaleString('en')} record${n === 1 ? '' : 's'}`;
  const short = (hash: string) => hash.slice(0, 12);
  const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
  const checked = $derived(checkedAt === null ? '' : now - checkedAt < 60_000 ? 'just now' : `at ${clockTime(new Date(checkedAt).toISOString(), timezone, false)}`);
  const caveats = $derived((['older_schema', 'mismatch'] as const).filter((a) => checkpoints.data?.backups.some((b) => b.anchor === a)));
</script>

<div class="history-checkpoints" data-fid="history-checkpoints">
  {#if checkpoints.error}
    <p class="flash flash--error history-checkpoints__error" role="alert">
      Could not verify the history: {checkpoints.error}
      {#if !checkpoints.data}<button class="btn" type="button" aria-disabled={busy} onclick={verify}>Try again</button>{/if}
    </p>
  {/if}
  {#if checkpoints.data}
    {@const v = checkpoints.data.verified}
    <div class="history-verify" class:history-verify--risk={!v.ok} data-fid="history-verify" aria-busy={busy}>
      <span class="history-verify__glyph" aria-hidden="true"><Icon name={v.ok ? 'check' : 'x'} /></span>
      <p class="history-verify__text">
        <strong>{v.ok ? 'Chain verified' : 'Chain check failed'}</strong>
        <span>
          {#if !v.ok}The history no longer matches its hash chain{brokenAt(v.first_broken)} ·{/if}
          {records(v.checked)} · head <span class="mono">#{v.head.seq} · {short(v.head.hash)}</span>
          {#if checked}· checked {checked}{/if}
        </span>
      </p>
      <button class="btn history-verify__again" type="button" aria-disabled={busy} onclick={verify}>{busy ? 'Verifying…' : 'Verify again'}</button>
    </div>
    {#if checkpoints.data.backups.length}
      <div class="history-backups__wrap">
        <table class="history-backups" data-fid="history-backups">
          <caption class="vh">Backups anchoring the history, newest first</caption>
          <thead><tr><th scope="col">Backup</th><th scope="col">Taken</th><th scope="col">History head</th><th scope="col">Revision</th><th scope="col">Anchored</th></tr></thead>
          <tbody>
            {#each checkpoints.data.backups as backup (backup.file)}
              {@const anchor = ANCHOR[backup.anchor]}
              <tr data-fid="history-backup">
                <th scope="row" class="mono">{backup.file}</th>
                <td class="mono">{localAt(backup.created_at, timezone)}</td>
                <td class="mono">#{backup.history_head.seq} · {short(backup.history_head.hash)}</td>
                <td class="mono">{backup.revision}</td>
                <td>
                  <span class="status-chip history-anchor history-anchor--{backup.anchor} {TONE[backup.anchor]}" title={anchor.note}
                    >{#if backup.anchor === 'matches'}<Icon name="check" />{:else if backup.anchor === 'mismatch'}<Icon name="x" />{:else}<span class="history-anchor__ring" aria-hidden="true"></span>{/if}{anchor.label}<span class="vh"> — {anchor.note}</span></span
                  >
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if caveats.length}
        <p class="history-backups__note">
          <Icon name="info" />
          <span>{#each caveats as a, i (a)}{i ? ' ' : ''}<strong>{cap(ANCHOR[a].label)}:</strong> {ANCHOR[a].note}.{/each}</span>
        </p>
      {/if}
    {:else if checkpoints.data.backup_dir_configured}
      <div class="empty history-checkpoints__empty"><strong>No backups recorded yet</strong>Backups taken with the deploy script appear here.</div>
    {:else}
      <div class="empty history-checkpoints__empty">
        <strong>No backup directory</strong>This server has no backup directory configured (<span class="mono">KANADE_BACKUP_DIR</span>), so no backup anchors the history.
      </div>
    {/if}
  {:else if !checkpoints.error}<p class="note" aria-busy="true">Verifying the history…</p>{/if}
</div>
<LiveRegion {message} />
