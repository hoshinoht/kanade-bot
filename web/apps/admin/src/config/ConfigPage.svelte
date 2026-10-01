<!--
  v4 config.html as one settings window. Each section saves itself with a
  partial PATCH; a refusal stays inline next to the fields that caused it.
  Env-only settings are shown read-only with the reason they are env-only.
  Sections mount on first visit and then stay mounted, so unsaved edits
  survive tab switches (the same visited pattern as the ui Tabs).
-->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/settings.scss';
  import type { ConfigView, Role, RoleProfileWrite } from '@kanade/api-types';
  import { Icon, ThemePicker, Toaster } from '@kanade/ui';
  import { SvelteSet } from 'svelte/reactivity';
  import { directory } from '../names/directory.svelte';
  import { Resource, send } from '../resource.svelte';
  import AccessSection from './AccessSection.svelte';
  import ChatbotSection from './ChatbotSection.svelte';
  import DigestSection from './DigestSection.svelte';
  import EnvSection from './EnvSection.svelte';
  import ModelsSection from './ModelsSection.svelte';
  import PersonaSection from './PersonaSection.svelte';
  import PingsSection from './PingsSection.svelte';
  import type { ConfigPatch, RoleProfileSave } from './save';
  import SelfServiceSection from './SelfServiceSection.svelte';
  import Toggle from './Toggle.svelte';
  import RescanPanel from '../extractions/RescanPanel.svelte';
  import type { Channel } from '@kanade/api-types';

  let {
    toaster,
    section = '',
    onsection,
  }: { toaster: Toaster; section?: string; onsection?: (key: string) => void } = $props();

  const SECTIONS = [
    { key: 'pings', label: 'Pings' },
    { key: 'watching', label: 'Chat watching' },
    { key: 'chatbot', label: 'Chatbot' },
    { key: 'persona', label: 'Persona' },
    { key: 'models', label: 'Models' },
    { key: 'self-service', label: 'Self-service' },
    { key: 'notifications', label: 'Notifications' },
    { key: 'theme', label: 'Theme' },
    { key: 'digest', label: 'Weekly digest' },
    { key: 'rescan', label: 'Re-read' },
    { key: 'access', label: 'Channel access' },
    { key: 'env', label: 'Set in the environment' },
  ] as const;
  type Key = (typeof SECTIONS)[number]['key'];

  const config = new Resource<ConfigView>('/api/admin/config');
  const targets = new Resource<Channel[]>('/api/admin/rescan/targets');
  let roleChoices = $state<Role[] | null>(null);
  let roleDirectoryError = $state('');
  let roleDirectoryLoading = $state(false);
  let roleDirectoryRequest = 0;
  $effect(() => {
    void config.load();
  });

  const uid = $props.id();
  const selected = $derived<Key>(SECTIONS.some((s) => s.key === section) ? (section as Key) : 'pings');
  const tabs: Record<string, HTMLButtonElement> = {};
  // Unsaved edits live in the section components; keeping visited sections
  // mounted keeps those edits across tab switches.
  const visited = new SvelteSet<Key>(['pings']);
  $effect.pre(() => {
    visited.add(selected);
  });
  $effect(() => {
    if (visited.has('rescan') && !targets.data) void targets.load();
  });

  async function refreshRoleDirectory(): Promise<void> {
    const request = ++roleDirectoryRequest;
    roleChoices = null;
    roleDirectoryError = '';
    roleDirectoryLoading = true;
    const result = await send((client) => client.get<Role[]>('/api/admin/roles'));
    if (request !== roleDirectoryRequest) return;
    if (result.ok) roleChoices = result.value;
    else roleDirectoryError = result.message;
    roleDirectoryLoading = false;
  }

  // Role names are an authorization-sensitive directory, not a display cache:
  // fetch a fresh guild list every time Persona becomes the active section.
  $effect(() => {
    if (selected !== 'persona') return;
    void refreshRoleDirectory();
    return () => {
      roleDirectoryRequest += 1;
    };
  });

  // A tab that would stop the bot is flagged in the list: the server's own startup check says so.
  const modelsBlocked = $derived(config.data?.models.capacity_check.some((c) => c.level === 'error') ?? false);
  // Role names for the shared lookup (a role mention reads @name, never its id).
  $effect(() => {
    if (config.data)
      directory.setRoles(
        config.data.persona.role_profiles.flatMap(({ role_id, role_name, profile }) =>
          role_name === null ? [] : [{ role_id, role_name, profile }],
        ),
      );
  });
  const missingManage = $derived(config.data?.manage_messages.missing ?? []);

  // On phones the list is a sideways strip: bring a deep-linked section's tab
  // into view by scrolling the strip itself (scrollIntoView could also move
  // the frame, which never scrolls).
  let toc: HTMLDivElement | undefined = $state();
  // Same breakpoint as _settings.scss: a sideways strip on phones, a column otherwise.
  let narrow = $state(false);
  $effect(() => {
    const query = window.matchMedia('(max-width: 899px)');
    const update = () => (narrow = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });
  function reveal(strip: HTMLElement, tab: HTMLElement) {
    if (strip.scrollWidth <= strip.clientWidth) return;
    const start = tab.getBoundingClientRect().left - strip.getBoundingClientRect().left + strip.scrollLeft;
    if (start < strip.scrollLeft || start + tab.offsetWidth > strip.scrollLeft + strip.clientWidth)
      strip.scrollLeft = start - (strip.clientWidth - tab.offsetWidth) / 2;
  }
  $effect(() => {
    const tab = tabs[selected];
    const strip = toc;
    if (!strip || !tab) return;
    reveal(strip, tab);
    // Web fonts (and the wide/narrow switch) resize the tabs after first paint.
    const watch = new ResizeObserver(() => reveal(strip, tab));
    for (const each of Object.values(tabs)) watch.observe(each);
    watch.observe(strip);
    return () => watch.disconnect();
  });

  function select(index: number) {
    const item = SECTIONS[(index + SECTIONS.length) % SECTIONS.length]!;
    onsection?.(item.key);
    tabs[item.key]?.focus();
  }

  function onKeydown(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = {
      ArrowDown: index + 1,
      ArrowRight: index + 1,
      ArrowUp: index - 1,
      ArrowLeft: index - 1,
      Home: 0,
      End: SECTIONS.length - 1,
    };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    select(target);
  }

  async function save(patch: ConfigPatch, done: string): Promise<string> {
    const result = await send((c) => c.patch<ConfigView>('/api/admin/config', patch));
    if (!result.ok) return result.message;
    config.data = result.value;
    const notes = result.value.notices ?? [];
    toaster.show({ message: notes.length ? `${done} ${notes.join(' ')}` : done, tone: 'ok' });
    return '';
  }

  async function saveRoleProfiles(assignments: RoleProfileWrite[], digest: string): Promise<RoleProfileSave> {
    const result = await send((client) =>
      client.patch<ConfigView>('/api/admin/config', {
        persona: { role_profiles: assignments, role_profiles_digest: digest },
      }),
    );
    if (!result.ok) return result;
    config.data = result.value;
    const notes = result.value.notices ?? [];
    toaster.show({ message: notes.length ? `Role assignments saved. ${notes.join(' ')}` : 'Role assignments saved.', tone: 'ok' });
    return result;
  }

  async function refreshConfig(): Promise<ConfigView | null> {
    const result = await send((client) => client.get<ConfigView>('/api/admin/config'));
    if (!result.ok) {
      config.error = result.message;
      return null;
    }
    config.data = result.value;
    config.error = '';
    return result.value;
  }
</script>

<!-- v4 config.html: no page head; the window is the page, titled in its own
  bar with the one line that says what the settings are. -->
<!-- On a phone the top bar already says "Config": the line stays for screen readers only. -->
<PageLine class="pageline--echo">
  <h1 id="{uid}-h">Config</h1>
  <p class="pageline__context">runtime settings take effect at once and survive a restart</p>
</PageLine>

{#if config.error}<p class="flash flash--error" role="status">{config.error}</p>{/if}

{#if missingManage.length}
  <!-- v4 used the empty-state box; a compact flash keeps the settings window
    the tallest thing on the page (task-first hierarchy). -->
  <p class="flash flash--error settings__banner" role="status">
    <strong>Missing “Manage Messages” in {missingManage.join(', ')}.</strong>
    <span class="settings__banner-why"
      >The bot needs it to take somebody's old reaction off, so ✅ and ❌ stay one-or-the-other. Until it is granted, a person who switches
      answer is counted as both there.</span
    >
    Fix it in <strong>Edit Channel → Permissions</strong>, or re-invite the bot with the permissions in the README.
  </p>
{/if}

<section class="card settings window-fill" aria-labelledby="{uid}-w">
  <div class="card__head">
    <h2 class="card__title" id="{uid}-w">Settings</h2>
    <span class="id">env-only settings are listed last</span>
  </div>
  <div class="settings__body">
    <div class="settings__toc" bind:this={toc} role="tablist" aria-label="Settings sections" aria-orientation={narrow ? 'horizontal' : 'vertical'}>
      {#each SECTIONS as item, index (item.key)}
        <button
          type="button"
          role="tab"
          class="settings__tab"
          id="{uid}-tab-{item.key}"
          aria-selected={selected === item.key}
          aria-controls="{uid}-panel-{item.key}"
          tabindex={selected === item.key ? 0 : -1}
          bind:this={tabs[item.key]}
          onclick={() => onsection?.(item.key)}
          onkeydown={(event) => onKeydown(event, index)}
        >
          {item.label}
          {#if item.key === 'models' && modelsBlocked}<span class="settings__flag"><Icon name="alert-triangle" label="needs attention" /></span>{/if}
        </button>
      {/each}
    </div>
    <div class="settings__detail">
      {#each SECTIONS as item (item.key)}
        {#if visited.has(item.key)}
          <div
            class="settings__panel"
            role="tabpanel"
            id="{uid}-panel-{item.key}"
            aria-labelledby="{uid}-tab-{item.key}"
            tabindex="0"
            hidden={selected !== item.key}
          >
            {#if item.key === 'theme'}
              <h3 class="settings__title">Theme</h3>
              <ThemePicker />
              <p class="note">Kept in this browser only.</p>
            {:else if item.key === 'digest'}
              <DigestSection {toaster} />
            {:else if item.key === 'rescan'}
              <h3 class="settings__title">Re-read the party channels</h3>
              <RescanPanel targets={targets.data ?? []} />
              <p class="note">Progress and past runs are also on <a href="/extractions">Extractions</a>.</p>
            {:else if item.key === 'access'}
              <AccessSection {toaster} />
            {:else if config.data}
              {@const c = config.data}
              {#if item.key === 'pings'}
                <PingsSection pings={c.pings} {save} />
              {:else if item.key === 'watching'}
                <h3 class="settings__title">Chat watching</h3>
                <div class="settings__actions">
                  <Toggle on={!c.watching.paused} onLabel="Pause watching" offLabel="Resume watching" apply={(on) => save({ watching: { paused: !on } }, on ? 'Watching resumed.' : 'Watching paused.')} />
                  <Toggle on={c.watching.extract_enabled} onLabel="Turn the extractor off" offLabel="Turn the extractor on" apply={(on) => save({ watching: { extract_enabled: on } }, on ? 'The extractor is on.' : 'The extractor is off.')} />
                </div>
                <p class="note">
                  Watching is <strong>{c.watching.paused ? 'paused' : 'on'}</strong> · extractor <strong>{c.watching.extract_enabled ? 'on' : 'off'}</strong>.
                  Messages are stored either way, so a rescan can catch up later.
                </p>
              {:else if item.key === 'chatbot'}
                <ChatbotSection chatbot={c.chatbot} {save} />
              {:else if item.key === 'persona'}
                <PersonaSection
                  persona={c.persona}
                  {save}
                  {toaster}
                  refresh={refreshConfig}
                  roles={roleChoices}
                  rolesLoading={roleDirectoryLoading}
                  rolesError={roleDirectoryError}
                  refreshRoles={refreshRoleDirectory}
                  {saveRoleProfiles}
                />
              {:else if item.key === 'models'}
                <ModelsSection models={c.models} {save} />
              {:else if item.key === 'self-service'}
                <SelfServiceSection selfService={c.self_service} {save} />
              {:else if item.key === 'notifications'}
                <h3 class="settings__title">Notifications</h3>
                <div class="settings__actions">
                  <Toggle on={c.notifications.quiet_mode} onLabel="Turn quiet mode off" offLabel="Turn quiet mode on" apply={(on) => save({ notifications: { quiet_mode: on } }, on ? 'Quiet mode is on.' : 'Quiet mode is off.')} />
                </div>
                <p class="note">
                  Quiet mode is <strong>{c.notifications.quiet_mode ? 'on' : 'off'}</strong>. While it is on the bot posts everything as usual but
                  notifies nobody — names still show, no pings go out, and each message is marked 🔕 in Discord.
                </p>
              {:else}
                <EnvSection env={c.env} />
              {/if}
            {:else if config.loading}
              <p class="note" role="status">Loading the settings…</p>
            {/if}
          </div>
        {/if}
      {/each}
    </div>
  </div>
</section>
