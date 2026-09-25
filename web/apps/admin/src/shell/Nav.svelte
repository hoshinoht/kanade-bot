<script lang="ts">
  import { GROUPS, PINNED, SECTIONS, type Section } from '../routes';

  let {
    active,
    inbox = 0,
    onsignout,
  }: { active: string; inbox?: number; /** Ends the session before the sign-in page shows. */ onsignout?: (event: MouseEvent) => void } = $props();
  let more = $state(false);

  // A route change closes the phone menu, as v4's portal.js did on navigation.
  $effect(() => {
    void active;
    more = false;
  });
</script>

{#snippet link(section: Section)}
  <a class="nav__link" href={section.href} aria-current={section.key === active ? 'page' : undefined}>
    {section.label}{#if section.key === 'inbox' && inbox > 0}<span class="pip"
        >{inbox}<span class="vh"> waiting</span></span
      >{/if}
  </a>
{/snippet}

{#snippet groups()}
  {#each GROUPS as group (group)}
    <div class="nav__group">
      <span class="nav__eyebrow" aria-hidden="true">{group}</span>
      <div class="nav__links" role="group" aria-label={group}>
        {#each SECTIONS.filter((s) => s.group === group) as section (section.key)}{@render link(section)}{/each}
      </div>
    </div>
  {/each}
{/snippet}

<!-- v4 base.html: grouped links on a desk; Week and Inbox pinned plus "More" on a phone.
     Exactly one layout is displayed, so nothing is announced twice. -->
<nav class="nav" aria-label="Sections">
  <div class="nav__desk">{@render groups()}</div>
  <div class="nav__phone">
    {#each SECTIONS.filter((s) => PINNED.includes(s.key)) as section (section.key)}{@render link(section)}{/each}
    <details class="nav__more" bind:open={more}>
      <summary class="nav__more-btn">More</summary>
      <div class="nav__sheet">
        {@render groups()}
        <a class="nav__link" href="/login" onclick={onsignout}>Sign out</a>
      </div>
    </details>
  </div>
</nav>
