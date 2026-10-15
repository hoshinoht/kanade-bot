<!--
  "Who carries" (boards in docs/research/2026-10-15-estimate-share-chart,
  option A): one 100% bar per boss of the run, each member's part their
  effective % over the party's combined %, in party order. Every part carries
  its number ("1·28%"; narrower parts drop the %, then the number, which the
  title and the bar's accessible name keep); an assumed critical rate is
  hatched and starred. Members without an estimate are counted and named,
  never drawn as 0%. A member's colour follows their number, i.e. their row
  in the admin's members table; the portal, which has no table, adds the key.
  Shares come from `shares.ts`; the app passes the members and bosses.
-->
<script lang="ts">
  import '@kanade/ui/styles/screens/share-chart.scss';
  import { joinNames, shareBar, shareLabel, shareNotes, whoColour, type ShareMember } from './shares';

  let {
    bosses,
    members,
    keys = false,
    you = null,
    fids = {},
    ...rest
  }: {
    /** The run's bosses in order: catalog token and its full name ("Karling Hard"). */
    bosses: { token: string; title: string }[];
    /** Party order; `n` is each member's number. */
    members: ShareMember[];
    /** The portal: a key of number, colour and name under the bars. */
    keys?: boolean;
    /** The member the key calls "You". */
    you?: string | null;
    fids?: { missing?: string; keys?: string };
    [data: `data-${string}`]: string | undefined;
  } = $props();

  const bars = $derived(bosses.map((boss) => ({ boss, bar: shareBar(boss.token, members) })));
  const notes = $derived(
    shareNotes(
      bars.map((b) => b.bar),
      members,
    ),
  );
  const keyed = $derived(members.filter((m) => m.n !== null && !notes.none.includes(m)));
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
                title="{part.name} · {part.share}% of {bar.total?.toFixed(1)}%"
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
  {#if keys && keyed.length}
    <ul class="share__keys" aria-label="Who is which number" data-fid={fids.keys}>
      {#each keyed as member (member.id)}
        <li>
          <span class="share__sw share--who-{whoColour(member.n ?? 1)}" class:share__sw--assumed={member.assumed} aria-hidden="true">{member.n}</span>{member.id === you
            ? 'You'
            : member.name}
        </li>
      {/each}
    </ul>
  {/if}
  {#if notes.assumed.length || notes.none.length}
    <p class="share__notes">
      {#if notes.assumed.length}
        <span class="share__note"
          ><span class="share__hatch" aria-hidden="true"></span>* {joinNames(notes.assumed.map((m) => m.ign ?? m.name))} at an assumed 100% critical rate</span
        >
      {/if}
      {#if notes.none.length}
        <span class="share__note" data-fid={fids.missing}
          ><span class="share__sw share__sw--none" aria-hidden="true"></span>{`${notes.none.length} with no estimate: ${joinNames(notes.none.map((m) => m.name))} (not in the total, never 0%)`}</span
        >
      {/if}
    </p>
  {/if}
</figure>
