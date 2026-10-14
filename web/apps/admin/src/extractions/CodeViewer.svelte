<!--
  The Prompt / Raw code viewer (B_ExtractPrompt): a 44 px header with the
  title, a size chip, "find in …" (highlights matches), Wrap (on by default)
  and Copy; then the text with line numbers in a gutter screen readers and
  selections skip. Only the body scrolls.
-->
<script lang="ts">
  import { LiveRegion } from '@kanade/ui';
  import { matches, segments } from './code';

  let {
    title,
    text,
    find: findLabel,
    panel,
    tab,
  }: { title: string; text: string; /** "find in prompt" */ find: string; /** The tab panel's id and its tab's id. */ panel: string; tab: string } = $props();
  const uid = $props.id();
  let find = $state('');
  let wrap = $state(true);
  let note = $state('');
  const lines = $derived(text.split('\n'));
  const hits = $derived(matches(text, find));

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      note = `${title} copied.`;
    } catch {
      note = 'Copy was blocked; select the text instead.';
    }
  }
</script>

<div class="extract-code" data-fid="extract-code" id={panel} role="tabpanel" aria-labelledby={tab}>
  <div class="extract-code__bar" data-fid="extract-code-bar">
    <h3 class="extract-code__title">{title}</h3>
    <span class="extract-code__chip mono">{text.length.toLocaleString('en')} chars</span>
    {#if find}<span class="extract-code__chip" id="{uid}-hits">{hits} match{hits === 1 ? '' : 'es'}</span>{/if}
    <span class="extract-code__gap"></span>
    <input
      class="extract-code__find"
      type="search"
      bind:value={find}
      placeholder={findLabel}
      aria-label={findLabel}
      aria-describedby={find ? `${uid}-hits` : undefined}
      autocomplete="off"
      spellcheck="false"
    />
    <button class="btn extract-code__btn" type="button" aria-pressed={wrap} onclick={() => (wrap = !wrap)}>Wrap{wrap ? ' ✓' : ''}</button>
    <button class="btn extract-code__btn" type="button" onclick={() => void copy()}>Copy</button>
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <pre class="extract-code__body" class:extract-code__body--nowrap={!wrap} data-fid="extract-code-body" tabindex="0" aria-label={title}>{#each lines as line, i (i)}<span
        class="extract-code__line"><span class="extract-code__ln" aria-hidden="true">{i + 1}</span>{#each segments(line, find) as s, j (j)}{#if s.hit}<mark>{s.text}</mark>{:else}{s.text}{/if}{/each}</span
      >{/each}</pre>
  <LiveRegion message={note} />
</div>
