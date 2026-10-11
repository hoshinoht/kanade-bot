<!--
  The destinations in their three groups, shared by the navigation rail and
  the phone's drawer (M3E spec "Navigation rail"), then the More item holding
  the rarely visited ones (rail option B, `NavMore`). Every item is a real
  link with a visible label; the group headings show where there is room and
  are the groups' accessible names everywhere.
-->
<script lang="ts">
  import { Icon, pulse } from '@kanade/ui';
  import { arrival } from '../resource.svelte';
  import { GROUPS, MORE, SECTIONS } from '../routes';
  import NavMore from './NavMore.svelte';

  let {
    active,
    badges = {},
    counts = {},
    inline = false,
    onnavigate,
  }: {
    active: string;
    /** Attention badges by section key (the Inbox's waiting count), said as "N waiting". */
    badges?: Partial<Record<string, number>>;
    /** Glance counts trailing a destination where there is room (the drawer); the page says them in words. */
    counts?: Partial<Record<string, number>>;
    /** The drawer: More opens in place instead of as a menu. */
    inline?: boolean;
    /** A link was followed (the drawer closes). */
    onnavigate?: (event: MouseEvent) => void;
  } = $props();
</script>

<nav class="navlist" aria-label="Sections">
  {#each GROUPS as group (group)}
    <div class="navlist__group" role="group" aria-label={group}>
      <span class="navlist__heading" aria-hidden="true">{group}</span>
      {#each SECTIONS.filter((s) => s.group === group && !s.more) as section (section.key)}
        {@const badge = badges[section.key] ?? 0}
        <!-- No whitespace inside: the link's text is exactly its label (and count). -->
        <a class="navlist__item" href={section.href} aria-current={section.key === active ? 'page' : undefined}
          aria-label={badge > 0 ? `${section.label} ${badge} waiting` : undefined}
          onclick={onnavigate}
          ><span class="navlist__ind"><Icon name={section.icon} /></span><span class="navlist__label">{section.label}</span
          >{#if badge > 0}<span class="navlist__badge" {@attach pulse(badge, arrival.seq)}>{badge}<span class="vh"> waiting</span></span
          >{:else if counts[section.key] !== undefined}<span class="navlist__count" aria-hidden="true" {@attach pulse(counts[section.key], arrival.seq)}>{counts[section.key]}</span>{/if}</a
        >
      {/each}
    </div>
  {/each}
  {#if MORE.length}
    <div class="navlist__group navlist__group--more">
      <NavMore {active} {badges} {counts} {inline} {onnavigate} />
    </div>
  {/if}
</nav>
