<!--
  The guide's blocks in message order (boards G_Editor, G_EditorBosses,
  G_EditorStates, G_PhoneEditor): each row its number, kind, the messages it
  renders to and its tools (drag handle for pointers; Move up / Move down for
  keys and touch, off at the ends; Remove). One block is open at a time: a
  text block as a Markdown field with its live counter (UTF-16 units, red and
  refused past 2,000), a bosses block as its heading and one entry per boss
  (v4 name · Lv and difficulties, the knowledge lead, an optional note and
  the Large entry image checkbox), then Add boss. Add offers Text, Bosses or
  Spacer. A moved block keeps focus on the button that moved it.
-->
<script lang="ts">
  import type { BossRow, GuideBlock, GuideBossEntry, GuidePreview } from '@kanade/api-types';
  import { Icon, Select, StatusChip } from '@kanade/ui';
  import { tick } from 'svelte';
  import { CONTENT_LIMIT, KIND_LABEL, NOTE_LIMIT, count, leadOf, messagesLabel, newBlock, plain, textSummary, units } from './guides';

  let {
    blocks = $bindable(),
    openId = $bindable(null),
    saved,
    positions,
    preview,
    bosses,
    problems,
    flags = {},
    disabled = false,
  }: {
    blocks: GuideBlock[];
    /** The open block. */
    openId: string | null;
    /** The saved draft's block ids: the others are marked unsaved. */
    saved: ReadonlySet<string>;
    /** The messages each block renders to (0-based), by block id. */
    positions: Map<string, number[]>;
    /** The last good render: each entry's lead, and art that is missing. */
    preview: GuidePreview | null;
    bosses: BossRow[];
    /** The server's refusals by block id. */
    problems: Record<string, string[]>;
    /** A block's flag line ("Unconfirmed: posted or not?"). */
    flags?: Record<string, string>;
    disabled?: boolean;
  } = $props();
  const uid = $props.id();
  let list = $state<HTMLOListElement>();

  const bossOf = (key: string) => bosses.find((b) => b.key === key);

  /** An entry's embed in the render: the block's embeds in entry order. */
  function embedOf(block: GuideBlock, index: number) {
    const embeds = (preview?.slots ?? []).filter((s) => s.block_id === block.id).flatMap((s) => s.embeds);
    return embeds[index];
  }
  const missingArt = (block: GuideBlock, index: number) => (preview?.warnings ?? []).filter((w) => w.block_id === block.id && w.entry === index);

  async function move(index: number, by: -1 | 1, which: 'up' | 'down') {
    const to = index + by;
    if (to < 0 || to >= blocks.length) return;
    const next = [...blocks];
    [next[index], next[to]] = [next[to]!, next[index]!];
    blocks = next;
    await tick();
    // Focus stays on the moved block's own button (or its other one at an end).
    const row = list?.querySelector<HTMLElement>(`[data-block="${CSS.escape(next[to]!.id)}"]`);
    (row?.querySelector<HTMLButtonElement>(`.gblock__${which}:not(:disabled)`) ?? row?.querySelector<HTMLButtonElement>(`.gblock__${which === 'up' ? 'down' : 'up'}`))?.focus();
  }

  async function remove(index: number) {
    const gone = blocks[index]!;
    blocks = blocks.filter((_, i) => i !== index);
    if (openId === gone.id) openId = null;
    await tick();
    const rows = list?.querySelectorAll<HTMLElement>('.gblock');
    (rows?.[Math.min(index, (rows?.length ?? 1) - 1)]?.querySelector<HTMLButtonElement>('.gblock__remove') ?? addFirst)?.focus();
  }

  let addFirst = $state<HTMLButtonElement>();
  async function add(kind: GuideBlock['kind']) {
    const block = newBlock(kind);
    blocks = [...blocks, block];
    openId = kind === 'spacer' ? openId : block.id;
    await tick();
    list?.querySelector<HTMLElement>(`[data-block="${CSS.escape(block.id)}"] textarea, [data-block="${CSS.escape(block.id)}"] input`)?.focus();
  }

  async function open(block: GuideBlock) {
    openId = block.id;
    await tick();
    list?.querySelector<HTMLElement>(`[data-block="${CSS.escape(block.id)}"] textarea, [data-block="${CSS.escape(block.id)}"] input`)?.focus();
  }

  function update(index: number, block: GuideBlock) {
    blocks = blocks.map((b, i) => (i === index ? block : b));
  }

  function entries(index: number, change: (entries: GuideBossEntry[]) => GuideBossEntry[]) {
    const block = blocks[index]!;
    if (block.kind === 'bosses') update(index, { ...block, entries: change(block.entries) });
  }

  // Pointer drag (the handle): HTML drag and drop between rows; keys and touch use Move up / Move down.
  let dragging = $state<number | null>(null);
  function drop(index: number) {
    if (dragging === null || dragging === index) return;
    const next = [...blocks];
    const [moved] = next.splice(dragging, 1);
    next.splice(index, 0, moved!);
    blocks = next;
    dragging = null;
  }

  let adding = $state<Record<string, string>>({});
</script>

{#snippet tool(icon: 'move' | 'chevron-down' | 'x', label: string, cls: string, off: boolean, run: () => void)}
  <button class="draftop__x {cls}" type="button" aria-label={label} disabled={off || disabled} onclick={run}><Icon name={icon} /></button>
{/snippet}

<section class="drafts__ops guide-blocks" aria-labelledby="{uid}-blocks" data-fid="guide-blocks">
  <div class="drafts__opshead"><h3 class="cap" id="{uid}-blocks">Blocks · {blocks.length}</h3></div>
  {#if blocks.length}
    <ol class="gblocks" aria-label="Blocks, in message order" bind:this={list}>
      {#each blocks as block, i (block.id)}
        {@const what = `block ${i + 1}`}
        {@const isOpen = openId === block.id}
        {@const at = messagesLabel(positions.get(block.id) ?? [])}
        {@const errors = problems[block.id] ?? []}
        <li
          class={['gblock', isOpen && 'gblock--open', !saved.has(block.id) && 'gblock--new', dragging === i && 'gblock--dragging']}
          data-fid="guide-block"
          data-block={block.id}
          ondragover={(event) => dragging !== null && event.preventDefault()}
          ondrop={(event) => {
            event.preventDefault();
            drop(i);
          }}
        >
          <span class="draftop__ord mono">{i + 1}</span>
          <div class="gblock__main">
            <div class="gblock__head">
              <span class="draftop__kind">{KIND_LABEL[block.kind]}</span>
              {#if at}<span class="gblock__msgs mono">{at}</span>{/if}
              {#if !saved.has(block.id)}<StatusChip tone="neutral">unsaved</StatusChip>{/if}
              {#if block.kind === 'text'}
                {@const n = units(block.content)}
                <span class={['gblock__count', 'mono', n > CONTENT_LIMIT && 'gblock__count--over']} data-fid="guide-counter" role="status" aria-label="{count(n)} of {count(CONTENT_LIMIT)} characters">{count(n)} / {count(CONTENT_LIMIT)}</span>
              {/if}
              <span class="gblock__tools">
                <button
                  class="draftop__x gblock__grip"
                  type="button"
                  aria-label="Drag to move {what}"
                  draggable="true"
                  {disabled}
                  ondragstart={(event) => {
                    dragging = i;
                    event.dataTransfer?.setData('text/plain', block.id);
                  }}
                  ondragend={() => (dragging = null)}><Icon name="move" /></button
                >
                {@render tool('chevron-down', `Move ${what} up`, 'gblock__up', i === 0, () => void move(i, -1, 'up'))}
                {@render tool('chevron-down', `Move ${what} down`, 'gblock__down', i === blocks.length - 1, () => void move(i, 1, 'down'))}
                {@render tool('x', `Remove ${what}`, 'gblock__remove', false, () => void remove(i))}
              </span>
            </div>
            {#if flags[block.id]}<span class="draftop__flag"><Icon name="alert-triangle" />{flags[block.id]}</span>{/if}
            {#if block.kind === 'text'}
              {#if isOpen}
                {@const over = units(block.content) - CONTENT_LIMIT}
                <label class="vh" for="{uid}-text-{block.id}">Block {i + 1} text, Discord Markdown</label>
                <textarea
                  class="gblock__text"
                  id="{uid}-text-{block.id}"
                  value={block.content}
                  aria-invalid={over > 0 || errors.length > 0}
                  aria-describedby="{uid}-err-{block.id}"
                  {disabled}
                  oninput={(event) => update(i, { ...block, content: event.currentTarget.value })}
                ></textarea>
                <p class="field__error" role="alert" id="{uid}-err-{block.id}">
                  {#if over > 0}{count(over)} over Discord’s 2,000 limit for one message. Shorten it, or move the end into a new text block below. It can’t be saved until it fits.{:else}{errors.join(' ')}{/if}
                </p>
              {:else}
                <button class="gblock__summary gblock__open" type="button" aria-label="Edit {what}: {textSummary(block.content) || 'empty text'}" onclick={() => void open(block)}
                  >{textSummary(block.content) || 'Empty: write the message’s text.'}</button
                >
                {#if errors.length}<p class="field__error" role="alert">{errors.join(' ')}</p>{/if}
              {/if}
            {:else if block.kind === 'spacer'}
              <p class="gblock__summary">An empty message: a gap between the messages around it.</p>
            {:else if isOpen}
              <label class="field gblock__heading">
                <span>Heading</span>
                <small class="gentry__hint">Optional · the first message’s text</small>
                <input
                  value={block.heading ?? ''}
                  placeholder="e.g. ## Entry"
                  {disabled}
                  oninput={(event) => {
                    const value = event.currentTarget.value;
                    // An empty heading is absent (the API refuses a blank one).
                    update(i, value ? { ...block, heading: value } : { kind: 'bosses', id: block.id, entries: block.entries });
                  }}
                />
              </label>
              <ul class="gentries" aria-label="Bosses in block {i + 1}">
                {#each block.entries as entry, k (entry.key)}
                  {@const boss = bossOf(entry.key)}
                  {@const embed = embedOf(block, k)}
                  {@const lead = embed ? leadOf(embed.description, entry.note) : null}
                  {@const noart = missingArt(block, k)}
                  {@const name = embed?.title.split(' · ')[0] ?? boss?.name ?? entry.key}
                  <li class="gentry" data-fid="guide-entry">
                    <span class="inbox__art drafts__art" aria-hidden="true">{#if boss?.portrait}<img class="portrait portrait--md" src={boss.portrait} alt="" />{/if}</span>
                    <div class="gentry__main">
                      <p class="gentry__name"><b>{name}</b>{boss ? ` · Lv${boss.level}` : ''}</p>
                      <p class="gentry__sub">{boss?.difficulties.map((d) => d.name).join(' · ') ?? entry.key}</p>
                      <p class="gentry__lead" data-fid="guide-entry-lead">
                        <span class="cap">Knowledge lead</span>
                        {#if lead}<span>{lead}</span>{:else if embed}<span class="gentry__none">None for {name} yet. Add one in Bosses › Knowledge.</span>{:else}<span class="gentry__none">Shown once the preview is drawn.</span>{/if}
                      </p>
                      <label class="field gentry__note" data-fid="guide-entry-note">
                        <span>Note</span>
                        <small class="gentry__hint">Optional · {count(units(entry.note ?? ''))} / {count(NOTE_LIMIT)}</small>
                        <textarea
                          rows="4"
                          placeholder="Shown under the lead"
                          value={entry.note ?? ''}
                          aria-invalid={units(entry.note ?? '') > NOTE_LIMIT}
                          {disabled}
                          oninput={(event) => {
                            const value = event.currentTarget.value;
                            entries(i, (list) => list.map((e, j) => (j === k ? (value ? { ...e, note: value } : { key: e.key, large_image: e.large_image }) : e)));
                          }}
                        ></textarea>
                      </label>
                      <label class="ack gentry__image" data-fid="guide-entry-image">
                        <input
                          type="checkbox"
                          checked={entry.large_image}
                          {disabled}
                          onchange={(event) => {
                            const on = event.currentTarget.checked;
                            entries(i, (list) => list.map((e, j) => (j === k ? { ...e, large_image: on } : e)));
                          }}
                        />
                        <span>Large entry image <span class="gentry__hint">{entry.large_image ? 'under the embed; the portrait stays top right' : 'off: portrait only'}</span></span>
                      </label>
                      {#if noart.length}
                        <div class="flash flash--warn gplan__flash" role="note" data-fid="guide-entry-noart">
                          <b>No {noart.map((w) => (w.art === 'entry' ? 'entry art' : 'portrait')).join(' or ')} for {name}.</b> Its embed posts without {noart.length > 1 ? 'those pictures' : 'that picture'}. Add the files under the boss art folder and the guide shows “Out of date”.
                        </div>
                      {/if}
                    </div>
                    <span class="gblock__tools gentry__tools">
                      {@render tool('chevron-down', `Move ${name} up`, 'gblock__up', k === 0, () => entries(i, (list) => list.map((e, j) => (j === k - 1 ? list[k]! : j === k ? list[k - 1]! : e))))}
                      {@render tool('chevron-down', `Move ${name} down`, 'gblock__down', k === block.entries.length - 1, () => entries(i, (list) => list.map((e, j) => (j === k ? list[k + 1]! : j === k + 1 ? list[k]! : e))))}
                      {@render tool('x', `Remove ${name}`, 'gblock__remove', false, () => entries(i, (list) => list.filter((_, j) => j !== k)))}
                    </span>
                  </li>
                {/each}
              </ul>
              <div class="gentries__add">
                <Select
                  value={adding[block.id] ?? ''}
                  options={bosses.filter((b) => !block.entries.some((e) => e.key === b.key)).map((b) => ({ value: b.key, label: b.name, sub: `Lv${b.level}`, keywords: b.key }))}
                  label="Add boss"
                  placeholder="Add boss…"
                  noun="bosses"
                  {disabled}
                  onchange={(key) => {
                    if (!key) return;
                    entries(i, (list) => [...list, { key, large_image: false }]);
                    adding = { ...adding, [block.id]: '' };
                  }}
                />
                <span class="gentry__hint">One embed each; up to 10 embeds share a message.</span>
              </div>
              {#if errors.length}<p class="field__error" role="alert">{errors.join(' ')}</p>{/if}
            {:else}
              <button class="gblock__summary gblock__summary--bosses gblock__open" type="button" aria-label="Edit {what}: {block.entries.length} boss{block.entries.length === 1 ? '' : 'es'}" onclick={() => void open(block)}>
                {#if block.heading}<span>{plain(block.heading)}</span>{/if}
                {#each block.entries as entry (entry.key)}
                  {@const boss = bossOf(entry.key)}
                  <span class="gblock__boss">{#if boss?.portrait}<img class="portrait portrait--sm" src={boss.portrait} alt="" />{/if}{boss?.name ?? entry.key}{entry.large_image ? ' · large image' : ''}</span>
                {:else}<span>No bosses yet: open it and add one.</span>{/each}
              </button>
              {#if errors.length}<p class="field__error" role="alert">{errors.join(' ')}</p>{/if}
            {/if}
          </div>
        </li>
      {/each}
    </ol>
  {:else}
    <p class="note drafts__noops">No blocks yet. Add text, bosses or a spacer; each becomes one or more messages.</p>
  {/if}
  <div class="gblocks__add" data-fid="guide-add-block">
    <span class="cap">Add</span>
    <button class="btn" type="button" bind:this={addFirst} {disabled} onclick={() => void add('text')}><Icon name="plus" />Text</button>
    <button class="btn" type="button" {disabled} onclick={() => void add('bosses')}><Icon name="plus" />Bosses</button>
    <button class="btn" type="button" {disabled} onclick={() => void add('spacer')}><Icon name="plus" />Spacer</button>
  </div>
</section>
