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
  import { COLORWAYS, currentColorway, Icon, LiveRegion, LoadError, LoadingState, RowContent, SINGLE_PANE_QUERY, Toaster } from '@kanade/ui';
  import { tick } from 'svelte';
  import { SvelteMap, SvelteSet } from 'svelte/reactivity';
  import { directory } from '../names/directory.svelte';
  import { Resource, send } from '../resource.svelte';
  import AccessSection from './AccessSection.svelte';
  import ChannelsSection from './ChannelsSection.svelte';
  import ChatbotSection from './ChatbotSection.svelte';
  import DigestSection from './DigestSection.svelte';
  import EnvSection from './EnvSection.svelte';
  import ModelsSection from './ModelsSection.svelte';
  import PersonaSection from './PersonaSection.svelte';
  import PingsSection from './PingsSection.svelte';
  import ProfanitySection from './ProfanitySection.svelte';
  import RunLengthsSection from './RunLengthsSection.svelte';
  import type { ConfigPatch, RoleProfileSave, Undo } from './save';
  import SectionScope from './SectionScope.svelte';
  import SelfServiceSection from './SelfServiceSection.svelte';
  import SettingsPanel from './SettingsPanel.svelte';
  import SwitchCard from './SwitchCard.svelte';
  import { findSetting, settleFrames } from './find';
  import ThemeTiles from './ThemeTiles.svelte';
  import RescanPanel from '../extractions/RescanPanel.svelte';
  import type { Channel } from '@kanade/api-types';

  let {
    toaster,
    section = '',
    onsection,
    onrunlengths,
    onquiet,
  }: {
    toaster: Toaster;
    section?: string;
    onsection?: (key: string) => void;
    /** Run lengths saved: the planner's keyboard step follows the new default. */
    onrunlengths?: (defaultMinutes: number) => void;
    /** A save answered: the shell's quiet-mode chip follows the saved value at once. */
    onquiet?: (on: boolean) => void;
  } = $props();

  // `terms` hold the cards' own titles too, so a search names a setting, not only a section.
  const SECTIONS = [
    { key: 'pings', label: 'Pings', group: 'Bot', terms: 'morning ping time countdowns reminder' },
    { key: 'run-lengths', label: 'Run lengths', group: 'Bot', terms: 'duration boss default overrides' },
    { key: 'watching', label: 'Chat watching', group: 'Bot', terms: 'watching extractor messages pause' },
    { key: 'chatbot', label: 'Chatbot', group: 'Bot', terms: 'answer rate cap limits how often it answers per person' },
    { key: 'profanity', label: 'Profanity', group: 'Bot', terms: 'swear words blocked deny list deflection line allowed built-in questions replies check' },
    { key: 'persona', label: 'Persona', group: 'Bot', terms: 'active persona reply profiles visibility roles role overrides assignments' },
    { key: 'models', label: 'Models', group: 'Bot', terms: 'roles reasoning context windows capacity groups kanata extraction rewrite' },
    { key: 'self-service', label: 'Self-service', group: 'Members', terms: 'public portal cards links how members are answered' },
    { key: 'notifications', label: 'Notifications', group: 'Members', terms: 'quiet mode pings' },
    { key: 'digest', label: 'Weekly digest', group: 'Members', terms: 'post channel week' },
    { key: 'channels', label: 'Channels', group: 'Server', terms: 'watched channels categories chat categories ids environment seed' },
    { key: 'rescan', label: 'Re-read', group: 'Server', terms: 'channels extractor running' },
    { key: 'access', label: 'Channel access', group: 'Server', terms: 'manage messages permissions check' },
    { key: 'theme', label: 'Theme', group: 'Server', terms: 'colourway colorway mode system light dark' },
    { key: 'env', label: 'Set in the environment', group: 'Read-only', terms: 'environment variables timezone' },
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
  const GROUPS = ['Bot', 'Members', 'Server', 'Read-only'] as const;
  let query = $state('');
  const search = $derived(query.trim().toLowerCase());
  const shown = $derived(SECTIONS.filter((item) => !search || `${item.label} ${item.terms}`.toLowerCase().includes(search)));
  // Filtered-out tabs stay in the DOM (hidden) so the open panel keeps its
  // label; the roving tab stop moves to the first match when the open one is hidden.
  const stop = $derived<Key | undefined>(shown.some((item) => item.key === selected) ? selected : shown[0]?.key);
  let colorway = $state('');
  $effect(() => {
    const read = () => (colorway = COLORWAYS.find((way) => way.key === currentColorway())?.name ?? '');
    read();
    const observer = new MutationObserver(read);
    observer.observe(document.documentElement, { attributeFilter: ['data-colorway'] });
    return () => observer.disconnect();
  });
  const tabs: Record<string, HTMLButtonElement> = {};
  // Unsaved edits live in the section components; keeping visited sections
  // mounted keeps those edits across tab switches.
  const visited = new SvelteSet<Key>();
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
    const query = window.matchMedia(SINGLE_PANE_QUERY);
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
    const item = shown[(index + shown.length) % shown.length];
    if (!item) return;
    onsection?.(item.key);
    tabs[item.key]?.focus({ preventScroll: true });
  }

  function onKeydown(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = {
      ArrowDown: index + 1,
      ArrowRight: index + 1,
      ArrowUp: index - 1,
      ArrowLeft: index - 1,
      Home: 0,
      End: shown.length - 1,
    };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    select(target);
  }

  // Enter opens the first matching section; when one of its settings matches
  // too, the panel scrolls to it, rings it briefly and focuses its control.
  let found = $state('');
  let jump = 0;
  async function searchKeydown(event: KeyboardEvent) {
    if (event.key !== 'Enter' || !shown.length) return;
    event.preventDefault();
    const item = shown[0]!;
    const asked = search;
    const request = ++jump;
    found = '';
    onsection?.(item.key);
    await tick();
    const panel = document.getElementById(`${uid}-panel-${item.key}`);
    // Sections load their own data; wait (bounded) for a matching card to render.
    const hit = panel ? await settleFrames(() => findSetting(panel, asked), 90) : null;
    if (request !== jump) return;
    if (!hit) {
      tabs[item.key]?.focus({ preventScroll: true });
      return;
    }
    await hit.reveal();
    found = `${item.label}: ${hit.title}`;
  }

  // Save bars report here (`section/form`); a section with any dirty form gets a dot in the list.
  const dirtyForms = new SvelteMap<string, boolean>();
  const reportDirty = (id: string, dirty: boolean) => {
    if (dirty) dirtyForms.set(id, true);
    else dirtyForms.delete(id);
  };
  const isDirty = (key: Key) => [...dirtyForms.keys()].some((id) => id.startsWith(`${key}/`));

  function hint(item: Key): string {
    const c = config.data;
    if (!c) return '';
    switch (item) {
      case 'pings': return c.pings.day_of_ping_time;
      case 'watching': return c.watching.paused ? 'paused' : 'on';
      case 'chatbot': return c.chatbot.enabled ? 'on' : 'off';
      case 'channels': return `${c.watching.channel_ids.length}`;
      case 'profanity': return checks(c.profanity);
      case 'persona': return c.persona.personas.find((p) => p.key === c.persona.active)?.name ?? c.persona.active;
      // The roles that have a model, as on the board ("3").
      case 'models': return `${Object.values(c.models.roles).filter((r) => r.alias).length}`;
      case 'self-service': return c.self_service.public_portal ? 'open' : 'closed';
      case 'theme': return colorway.toLowerCase();
      case 'env': return `${c.env.length}`;
      default: return '';
    }
  }

  /** Which sides the profanity guardrail checks, in one word or two. */
  function checks(p: ConfigView['profanity']): string {
    if (p.check_questions && p.check_replies) return 'on';
    if (p.check_questions) return 'questions';
    return p.check_replies ? 'replies' : 'off';
  }

  /** Expanded-row facts, one per line. */
  function summary(item: Key): string[] {
    const c = config.data;
    if (!c) return [];
    switch (item) {
      case 'pings': return [c.pings.day_of_ping_time, `Countdowns ${c.pings.countdown_minutes.join(', ')} min`];
      case 'run-lengths': return [`${c.run_lengths.default_minutes} min default`, `${c.run_lengths.overrides.length} overrides`];
      case 'watching': return [c.watching.paused ? 'Watching paused' : 'Watching on', `Extractor ${c.watching.extract_enabled ? 'on' : 'off'}`];
      case 'chatbot': return [c.chatbot.enabled ? 'On' : 'Off', `${c.chatbot.member_rate.count} answers / ${c.chatbot.member_rate.window_s}s per person`];
      case 'profanity': return [`Checks ${checks(c.profanity)}`, `${c.profanity.extra_words.length} extra · ${c.profanity.allowed_words.length} allowed again`];
      case 'persona': return [hint(item), `${c.persona.profiles.length} reply profiles`, `${c.persona.role_profiles.length} role overrides`];
      case 'models': return Object.entries(c.models.roles).map(([role, model]) => `${role}: ${model.alias ?? 'unset'}`);
      case 'self-service': return [`Portal ${hint(item)}`, c.self_service.effective_mode.replaceAll('_', ' ')];
      case 'notifications': return [`Quiet mode ${c.notifications.quiet_mode ? 'on' : 'off'}`];
      case 'digest': return ['Weekly post', 'Channel and preview'];
      case 'channels': return [
        `${c.watching.channel_ids.length} watched channels`,
        `${c.watching.category_ids.length} watched categories`,
        `${c.chatbot.category_ids.length} chat categories`,
      ];
      case 'rescan': return [`${targets.data?.length ?? 0} available channels`];
      case 'access': return [`${missingManage.length} channels missing Manage Messages`];
      case 'theme': return [colorway, 'Kept in this browser'];
      case 'env': return c.env.map((setting) => setting.label);
    }
  }

  /** The first few facts; the open panel holds the rest. */
  const FACTS_SHOWN = 3;
  function facts(item: Key): { shown: string[]; more: number } {
    const all = summary(item).filter(Boolean);
    return all.length > FACTS_SHOWN + 1 ? { shown: all.slice(0, FACTS_SHOWN), more: all.length - FACTS_SHOWN } : { shown: all, more: 0 };
  }

  async function save(patch: ConfigPatch, done: string, undo?: Undo): Promise<string> {
    const result = await send((c) => c.patch<ConfigView>('/api/admin/config', patch));
    // A refusal stays inline, next to the fields or switch that caused it.
    if (!result.ok) return result.message;
    config.data = result.value;
    onquiet?.(result.value.notifications.quiet_mode);
    const notes = result.value.notices ?? [];
    const message = notes.length ? `${done} ${notes.join(' ')}` : done;
    // Switches and visibility apply at once: their toast offers Undo (10 s by default).
    if (undo)
      toaster.show({
        message,
        tone: 'ok',
        action: { label: 'Undo', run: () => void undoChange(undo) },
      });
    else toaster.show({ message, tone: 'ok' });
    return '';
  }

  async function undoChange(undo: Undo) {
    const error = await save(undo.patch, undo.done);
    if (error) toaster.show({ message: `Couldn't undo: ${error}`, tone: 'error' });
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

  // After a successful post a failed re-read must not look like a page fault.
  async function refreshQuietly(): Promise<void> {
    const result = await send((client) => client.get<ConfigView>('/api/admin/config'));
    if (result.ok) config.data = result.value;
  }
</script>

<!-- v4 config.html: no page head; the window is the page, titled in its own
  bar with the one line that says what the settings are. -->
<!-- On a phone the top bar already says "Config": the line stays for screen
  readers only, unless it carries the problem alert. -->
<PageLine class={missingManage.length ? '' : 'pageline--echo'}>
  <h1 id="{uid}-h">Config</h1>
  <p class="pageline__context">runtime settings take effect at once and survive a restart</p>
  {#snippet side()}
    {#if missingManage.length}
      <!-- Replaces the old flash banner; the full explanation lives in Channel access. -->
      <a
        class="mchip settings__problem"
        href="/config?section=access"
        onclick={async (event) => {
          event.preventDefault();
          query = '';
          onsection?.('access');
          // A search may have hidden the tab; focus once it is shown again.
          await tick();
           tabs.access?.focus({ preventScroll: true });
        }}
        ><Icon name="alert-triangle" /><span class="settings__problem-text"
          ><b>Manage Messages missing in {missingManage.length} channel{missingManage.length === 1 ? '' : 's'}</b><span class="settings__problem-fix">&nbsp;· fix in Channel access</span></span
        ></a
      >
    {/if}
  {/snippet}
</PageLine>

<section class="card settings window-fill" data-fid="window" aria-labelledby="{uid}-w">
  <div class="card__head settings__head" data-fid="window-bar">
    <h2 class="card__title" id="{uid}-w">Settings</h2>
    <div class="settings__search" data-fid="window-search" role="search">
      <label class="vh" for="{uid}-search">Find a setting</label>
      <input
        id="{uid}-search"
        type="search"
        bind:value={query}
        onkeydown={searchKeydown}
        placeholder="find a setting…"
        autocomplete="off"
        spellcheck="false"
        aria-controls="{uid}-toc"
        aria-describedby="{uid}-matches"
      />
      <span class="vh" id="{uid}-matches" role="status">{search ? `${shown.length} section${shown.length === 1 ? '' : 's'} match; Enter opens the first` : ''}</span>
    </div>
    <LiveRegion message={found ? `Found ${found}` : ''} />
  </div>
  <div class="settings__body">
    <div class="settings__toc" data-fid="cfg-toc" id="{uid}-toc" bind:this={toc} role="tablist" aria-label="Settings sections" aria-orientation={narrow ? 'horizontal' : 'vertical'}>
      {#each GROUPS as group (group)}
        <div class="settings__group" role="presentation" hidden={!shown.some((item) => item.group === group)}>
          <p class="cap" aria-hidden="true">{group}</p>
          {#each SECTIONS.filter((item) => item.group === group) as item (item.key)}
            {@const index = shown.indexOf(item)}
            <button
              type="button"
              role="tab"
              class="settings__tab expandable-row"
              data-fid="cfg-item"
              id="{uid}-tab-{item.key}"
              aria-selected={selected === item.key}
              aria-controls="{uid}-panel-{item.key}"
              tabindex={stop === item.key ? 0 : -1}
              hidden={index < 0}
              bind:this={tabs[item.key]}
              onclick={() => onsection?.(item.key)}
              onkeydown={(event) => onKeydown(event, index)}
            >
              <!-- The sideways strip (below 900 px) keeps every tab one line tall. -->
              <RowContent expanded={selected === item.key && !narrow}>
                {#snippet compact()}<span class="settings__scan"><span class="settings__label">{item.label}</span>{#if hint(item.key)}<span class="settings__hint">{hint(item.key)}</span>{/if}</span>{/snippet}
                <span class="settings__expanded"><span class="settings__label">{item.label}</span><span class="settings__summary">{#each facts(item.key).shown as fact, i (i)}<span>{#if i}<span class="vh">, </span>{/if}{fact}</span>{/each}{#if facts(item.key).more}<span class="settings__more"><span class="vh">, </span>+{facts(item.key).more} more</span>{/if}</span></span>
              </RowContent>
              {#if isDirty(item.key)}<span class="settings__dirty"><span class="vh">, unsaved changes</span></span>{/if}
              {#if item.key === 'access' && missingManage.length}<span class="settings__flag"
                  ><span aria-hidden="true">⚠ {missingManage.length}</span><span class="vh">, {missingManage.length} need attention</span></span
                >
              {:else if item.key === 'models' && modelsBlocked}<span class="settings__flag"><Icon name="alert-triangle" label="needs attention" /></span>{/if}
            </button>
          {/each}
        </div>
      {/each}
      {#if !shown.length}<p class="settings__none" aria-hidden="true">No settings match “{query}”.</p>{/if}
    </div>
    <div class="settings__detail" data-fid="cfg-detail">
      <!-- A failed first read fills the open panel below; a failed re-read keeps the drafts and says so here. -->
      {#if config.error && config.data}<p class="flash flash--error" role="status">{config.error}</p>{/if}
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
            <SectionScope section={item.key} report={reportDirty}>
            {#if item.key === 'theme'}
              <SettingsPanel title="Theme">
                {#snippet lead()}Kept in this browser only; nothing is sent to the server.{/snippet}
                <div class="settings__card" data-fid="cfg-card"><ThemeTiles /></div>
                <p class="settings__box"><Icon name="info" />Changes apply at once, with no save step. The command palette (Ctrl K) has the same choices.</p>
              </SettingsPanel>
            {:else if item.key === 'digest'}
              <DigestSection {toaster} last={config.data?.last_digest ?? null} onposted={refreshQuietly} />
            {:else if item.key === 'rescan'}
              <SettingsPanel title="Re-read the party channels">
                {#snippet lead()}Runs the extractor again over stored messages.{/snippet}
                <RescanPanel targets={targets.data ?? []} details />
              </SettingsPanel>
            {:else if item.key === 'access'}
              <AccessSection {toaster} />
            {:else if config.data}
              {@const c = config.data}
              {#if item.key === 'pings'}
                <PingsSection pings={c.pings} {save} />
              {:else if item.key === 'run-lengths'}
                <RunLengthsSection runLengths={c.run_lengths} {save} onsaved={onrunlengths} />
              {:else if item.key === 'watching'}
                <SettingsPanel title="Chat watching">
                  {#snippet lead()}What the bot reads in the party channels.{/snippet}
                  <SwitchCard
                    title={() => 'Watching'}
                    on={!c.watching.paused}
                    stateOff="paused"
                    confirm="Pause watching?"
                    action={(next) => (next ? 'Resume watching' : 'Pause watching')}
                    apply={(on) =>
                      save({ watching: { paused: !on } }, on ? 'Watching resumed.' : 'Watching paused.', {
                        patch: { watching: { paused: on } },
                        done: on ? 'Watching paused again.' : 'Watching resumed again.',
                      })}>Reads new messages in the watched channels and categories (set in <a href="/config?section=channels">Channels</a>).</SwitchCard
                  >
                  <SwitchCard
                    title={() => 'Extractor'}
                    on={c.watching.extract_enabled}
                    confirm="Turn the extractor off?"
                    action={(next) => (next ? 'Turn the extractor on' : 'Turn the extractor off')}
                    apply={(on) =>
                      save({ watching: { extract_enabled: on } }, on ? 'The extractor is on.' : 'The extractor is off.', {
                        patch: { watching: { extract_enabled: !on } },
                        done: on ? 'The extractor is off again.' : 'The extractor is on again.',
                      })}
                    >Proposes schedule changes from what it reads. With the extractor off, messages are still stored, so a re-read can catch up later.</SwitchCard
                  >
                  <p class="settings__box">Switches apply at once and offer Undo for 10 s. Turning either off asks first.</p>
                  <p class="settings__cardnote">Catch up later from <a href="/config?section=rescan">Re-read</a> · progress is on <a href="/extractions">Extractions</a>.</p>
                </SettingsPanel>
              {:else if item.key === 'chatbot'}
                <ChatbotSection chatbot={c.chatbot} {save} />
              {:else if item.key === 'channels'}
                <ChannelsSection watching={c.watching} chatbot={c.chatbot} {save} />
              {:else if item.key === 'profanity'}
                <ProfanitySection profanity={c.profanity} {save} />
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
                <SettingsPanel title="Notifications">
                  <SwitchCard
                    title={() => 'Quiet mode'}
                    on={c.notifications.quiet_mode}
                    confirm="Turn quiet mode off?"
                    action={(next) => (next ? 'Turn quiet mode on' : 'Turn quiet mode off')}
                    apply={(on) =>
                      save({ notifications: { quiet_mode: on } }, on ? 'Quiet mode is on.' : 'Quiet mode is off.', {
                        patch: { notifications: { quiet_mode: !on } },
                        done: on ? 'Quiet mode is off again.' : 'Quiet mode is on again.',
                      })}
                    >While on, the bot posts everything as usual but notifies nobody: names still show, no pings go out, and each message is marked 🔕 in
                    Discord.</SwitchCard
                  >
                  <p class="settings__box">Applies at once, with Undo for 10 s.</p>
                </SettingsPanel>
              {:else}
                <EnvSection env={c.env} {toaster} />
              {/if}
            {:else if config.error}
              <LoadError thing="the settings" reason={config.error} onretry={() => void config.load()} level={3} />
            {:else if config.loading}
              <LoadingState text="Loading the settings…" />
            {/if}
            </SectionScope>
          </div>
        {/if}
      {/each}
    </div>
  </div>
</section>
