<!--
  "On this page": the guide's top sections (mission, facts, HP, the
  difficulty's notes) and its tabs, sticky at the top of the aside. A section
  entry scrolls the knowledge panel to it (never the document); a tab entry
  selects the tab and scrolls to the tab strip. The entry for what is at the
  top of the panel is marked current (scroll-spy: IntersectionObserver on the
  panel). Hidden wherever the aside sits under the guide.
-->
<script lang="ts">
  import type { GuideTab, TocSection } from './guide';
  import { tick } from 'svelte';
  import { reducedMotion } from '@kanade/ui';

  let {
    main,
    sections,
    tabs,
    tab,
    tabId,
    onTab,
  }: {
    main: HTMLElement | undefined;
    sections: TocSection[];
    tabs: { id: GuideTab; label: string }[];
    tab: GuideTab;
    /** The id of a tab's button, so a tab entry can move focus to it. */
    tabId: (tab: GuideTab) => string;
    onTab: (tab: GuideTab) => void;
  } = $props();

  let current = $state('');
  const panel = $derived(main?.closest<HTMLElement>('.knowledge-detail__body') ?? null);
  const targets = () => [
    ...sections.map((section) => ({ key: section.key, el: main?.querySelector<HTMLElement>(section.selector) ?? null })),
    { key: `tab:${tab}`, el: main?.querySelector<HTMLElement>('.guide-tabs') ?? null },
  ];

  /** The last target whose top has passed the upper third of the panel (the first while none has). */
  function spy() {
    if (!panel) return;
    const box = panel.getBoundingClientRect();
    const line = box.top + Math.min(160, box.height / 3);
    const atEnd = panel.scrollTop + panel.clientHeight >= panel.scrollHeight - 2;
    const live = targets().filter((target) => target.el);
    let found = live[0]?.key ?? '';
    for (const target of live) if (target.el!.getBoundingClientRect().top <= line) found = target.key;
    if (atEnd && panel.scrollTop > 0) found = live.at(-1)?.key ?? found;
    current = found;
  }

  $effect(() => {
    void sections;
    void tab;
    if (!panel) return;
    const observer = new IntersectionObserver(spy, { root: panel, threshold: [0, 1], rootMargin: '0px 0px -66% 0px' });
    for (const target of targets()) if (target.el) observer.observe(target.el);
    // The end of the panel is not a crossing: the last entry takes over there.
    panel.addEventListener('scrollend', spy);
    spy();
    return () => {
      observer.disconnect();
      panel.removeEventListener('scrollend', spy);
    };
  });

  function reveal(el: HTMLElement) {
    if (!panel) return;
    const top = el.getBoundingClientRect().top - panel.getBoundingClientRect().top + panel.scrollTop - 8;
    const still = reducedMotion();
    panel.scrollTo({ top: Math.max(0, top), behavior: still ? 'auto' : 'smooth' });
  }

  function toSection(section: TocSection) {
    const el = main?.querySelector<HTMLElement>(section.selector);
    if (!el) return;
    reveal(el);
    if (!el.hasAttribute('tabindex')) el.setAttribute('tabindex', '-1');
    el.focus({ preventScroll: true });
    current = section.key;
  }

  async function toTab(next: GuideTab) {
    onTab(next);
    await tick();
    const strip = main?.querySelector<HTMLElement>('.guide-tabs');
    if (strip) reveal(strip);
    document.getElementById(tabId(next))?.focus({ preventScroll: true });
    current = `tab:${next}`;
  }
</script>

<nav class="guide-toc" aria-label="On this page">
  <h2 class="cap">On this page</h2>
  <ul>
    {#each sections as section (section.key)}
      <li><button type="button" aria-current={current === section.key ? 'location' : undefined} onclick={() => toSection(section)}>{section.label}</button></li>
    {/each}
  </ul>
  <h3 class="cap">Guide</h3>
  <ul>
    {#each tabs as entry (entry.id)}
      <li>
        <button type="button" aria-current={current === `tab:${entry.id}` ? 'location' : undefined} onclick={() => toTab(entry.id)}>{entry.label}</button>
      </li>
    {/each}
  </ul>
</nav>
