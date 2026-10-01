<!--
  The eleven destinations in their three groups, shared by the navigation rail
  and the phone's drawer (M3E spec "Navigation rail"). Every item is a real
  link with a visible label; the group headings show where there is room and
  are the groups' accessible names everywhere.
-->
<script lang="ts">
  import { Icon } from '@kanade/ui';
  import { GROUPS, SECTIONS } from '../routes';

  let {
    active,
    inbox = 0,
    onnavigate,
  }: { active: string; inbox?: number; /** A link was followed (the drawer closes). */ onnavigate?: (event: MouseEvent) => void } = $props();
</script>

<nav class="navlist" aria-label="Sections">
  {#each GROUPS as group (group)}
    <div class="navlist__group" role="group" aria-label={group}>
      <span class="navlist__heading" aria-hidden="true">{group}</span>
      {#each SECTIONS.filter((s) => s.group === group) as section (section.key)}
        <!-- No whitespace inside: the link's text is exactly its label (and count). -->
        <a class="navlist__item" href={section.href} aria-current={section.key === active ? 'page' : undefined}
          aria-label={section.key === 'inbox' && inbox > 0 ? `Inbox ${inbox} waiting` : undefined}
          onclick={onnavigate}
          ><span class="navlist__ind"><Icon name={section.icon} /></span><span class="navlist__label">{section.label}</span
          >{#if section.key === 'inbox' && inbox > 0}<span class="navlist__badge">{inbox}<span class="vh"> waiting</span></span>{/if}</a
        >
      {/each}
    </div>
  {/each}
</nav>
