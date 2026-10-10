<!--
  "Copy transcript" for the open item of a log (Chat, Extractions, Rewrites),
  for agent debugging: the format choice, the copy itself and, without a
  clipboard, a viewer to copy by hand. The caller wraps this and its primary
  "Copy transcript" button (calling `copy()`) in `.transcript-actions`, so
  the pair wraps together and the button's layout tag stays a literal the
  build can strip.
-->
<script lang="ts">
  import { Select, type Toaster } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import '@kanade/ui/styles/transcript.scss';
  import { copyText } from '../shared/copy';
  import TextModal from '../shared/TextModal.svelte';
  import { FORMAT_LABEL, type TranscriptFormat } from './transcript';

  let {
    build,
    toaster,
    format = $bindable('markdown'),
  }: {
    /** The item's whole transcript in a format. */
    build: (format: TranscriptFormat) => string;
    toaster?: Toaster;
    /** Bound by callers that keep it while another item opens. */
    format?: TranscriptFormat;
  } = $props();

  let viewer = $state({ open: false, eyebrow: '', text: '' });

  export async function copy() {
    const label = FORMAT_LABEL[format];
    const text = build(format);
    if (await copyText(text)) toaster?.show({ message: `Transcript copied as ${label}.`, tone: 'ok' });
    else viewer = { open: true, eyebrow: label, text };
  }
</script>

<div class="transcript-format">
  <Select
    label="Transcript format"
    bind:value={() => format, (v) => (format = v as TranscriptFormat)}
    options={[
      { value: 'markdown', label: FORMAT_LABEL.markdown },
      { value: 'json', label: FORMAT_LABEL.json },
    ]}
  />
</div>

<TextModal bind:open={viewer.open} title="Transcript" eyebrow={viewer.eyebrow} text={viewer.text} {toaster} />
