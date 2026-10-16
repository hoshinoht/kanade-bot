<!--
  "Who carries" (boards in docs/research/2026-10-15-estimate-share-chart,
  option A; per boss in 2026-10-16-per-boss-picks): one 100% bar per boss of
  the run, each member's part their effective % over that boss's party's
  combined %, in party order. Every part carries its number ("1·28%";
  narrower parts drop the %, then the number, which the title and the bar's
  accessible name keep); an assumed critical rate is hatched and starred. A
  member keeps their number and colour on every bar even when they bring
  another character to a later boss (`by` says who brings whom), and a key
  names the numbers. Who has no estimate and who is counted at an assumed
  critical rate are the Party total's notes (`PartyTotalNotes`), never 0%
  parts. Shares come from `shares.ts`; the app passes the members and bosses.
-->
<script lang="ts">
  import '@kanade/ui/styles/screens/share-chart.scss';
  import Icon from '../components/Icon.svelte';
  import { shareBar, shareLabel, whoColour, type ShareMember } from './shares';

  let {
    bosses,
    members,
    you = null,
    by = null,
    fids = {},
    ...rest
  }: {
    /** The run's bosses in order: catalog token and its full name ("Karling Hard"). */
    bosses: { token: string; title: string }[];
    /** Party order; `n` is each member's number. */
    members: ShareMember[];
    /** The member the key calls "You". */
    you?: string | null;
    /** Who brings another character to a later boss ("Per boss: Asahi brings AsahiMech to HMaleficStar; …"). */
    by?: string | null;
    fids?: { keys?: string; by?: string };
    [data: `data-${string}`]: string | undefined;
  } = $props();

  const bars = $derived(bosses.map((boss) => ({ boss, bar: shareBar(boss.token, members) })));
  // The key names who has a part somewhere.
  const keyed = $derived(members.filter((m) => m.n !== null && bars.some(({ bar }) => bar.parts.some((p) => p.id === m.id))));
  const multi = $derived(bosses.length > 1);

  // Part widths through CSSOM (no inline styles under the CSP).
  const grow = (value: number) => (node: HTMLElement) => {
    node.style.setProperty('flex-grow', String(value));
  };
</script>

<figure class="share" {...rest}>
  <figcaption class="share__head"><span class="share__title">Who carries</span><span class="share__sub">Each member’s share of the party total</span></figcaption>
  <div class="share__rows">
    {#each bars as { boss, bar } (boss.token)}
      <div class="share__row" class:share__row--one={!multi}>
        {#if multi}
          <span class="share__boss"
            ><abbr title={boss.title}>{boss.token}</abbr><span class="mono">{bar.total === null ? '—' : `${bar.total.toFixed(1)}%`}</span></span
          >
        {/if}
        {#if bar.parts.length}
          <div class="share__bar" role="img" aria-label={shareLabel(boss.token, bar)}>
            {#each bar.parts as part (part.id)}
              <span
                class="share__seg share--who-{part.colour}"
                class:share__seg--assumed={part.assumed}
                title="{part.name}{part.ign ? ` · ${part.ign}` : ''} · {part.share}% of {bar.total?.toFixed(1)}%"
                {@attach grow(part.effective)}><b class="share__n">{part.n}</b><span class="share__pct">{part.share}%{part.assumed ? '*' : ''}</span></span
              >
            {/each}
          </div>
        {:else}
          <div class="share__bar share__bar--none" role="img" aria-label="{boss.token}: no estimate yet">no estimate yet</div>
        {/if}
      </div>
    {/each}
  </div>
  {#if keyed.length}
    <ul class="share__keys" aria-label="Who is which number" data-fid={fids.keys}>
      {#each keyed as member (member.id)}
        <li>
          <span
            class="share__sw share--who-{whoColour(member.n ?? 1)}"
            class:share__sw--assumed={Object.values(member.assumed).some(Boolean)}
            aria-hidden="true">{member.n}</span
          >{member.id === you ? 'You' : member.name}
        </li>
      {/each}
    </ul>
  {/if}
  {#if by}
    <p class="share__notes"><span class="share__note share__note--by" data-fid={fids.by}><Icon name="layers" />{by}</span></p>
  {/if}
</figure>
