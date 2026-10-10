<!--
  Config → Channels: where the bot reads and chats. Each list saves on its
  own (an explicit list save); a list nobody saved follows its env seed.
-->
<script lang="ts">
  import type { Channel, ConfigView } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import IdListCard from './IdListCard.svelte';
  import type { Save } from './save';
  import SettingsPanel from './SettingsPanel.svelte';

  let { watching, chatbot, save }: { watching: ConfigView['watching']; chatbot: ConfigView['chatbot']; save: Save } = $props();

  const directory = new Resource<Channel[]>('/api/admin/channels');
  $effect(() => void directory.load());
  const options = $derived((directory.data ?? []).map((c) => ({ value: c.id, label: c.name })));
  const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
</script>

<SettingsPanel title="Channels">
  {#snippet lead()}Where the bot reads party chat and where the chatbot answers.{/snippet}
  <IdListCard
    title="Watched channels"
    env="KANADE_WATCH_CHANNEL_IDS"
    ids={watching.channel_ids}
    source={watching.channel_ids_source}
    noun="channels"
    {options}
    save={(ids) => save({ watching: { channel_ids: ids } }, `Watched channels saved (${count(ids.length, 'channel', 'channels')}).`)}
    >The extractor reads new messages in these channels and their threads.</IdListCard
  >
  <IdListCard
    title="Watched categories"
    env="KANADE_WATCH_CATEGORY_IDS"
    ids={watching.category_ids}
    source={watching.category_ids_source}
    noun="categories"
    save={(ids) => save({ watching: { category_ids: ids } }, `Watched categories saved (${count(ids.length, 'category', 'categories')}).`)}
    >Every channel in these Discord categories is watched too. Enter each category by its id.</IdListCard
  >
  <IdListCard
    title="Chat categories"
    env="KANADE_CHAT_CATEGORY_IDS"
    ids={chatbot.category_ids}
    source={chatbot.category_ids_source}
    noun="categories"
    save={(ids) => save({ chatbot: { category_ids: ids } }, `Chat categories saved (${count(ids.length, 'category', 'categories')}).`)}
    >The chatbot answers in every channel of these categories. Enter each category by its id.</IdListCard
  >
  <p class="settings__box">
    <Icon name="info" />The environment variables are only the starting values: a list nobody saved here follows its variable at every restart.
    Saving a list here overrides its variable from then on. Each list saves on its own and applies at once.
  </p>
</SettingsPanel>
