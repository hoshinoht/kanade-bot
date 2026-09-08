# Boss entry artwork (optional)

The splash MapleStory shows in a boss's **entry UI** — the wide picture behind
the "enter" prompt, not the square portrait. Drop an image in here named after
the **boss key** in [`../../bosses.yaml`](../../bosses.yaml) — not the canonical
run name — and the Week page lays it *behind* that run's cards: faded to a wash,
cropped to the right of the card, and masked away from the clock, the bosses and
the party, so it is scenery rather than something anybody has to read past. Both
surfaces get it — the board's compact cards and the run card itself, which is
also the run sheet's body and the whole of the phone list — and neither card
grows by a pixel, because the picture sits behind the words rather than above
them.

```
boss/artwork/entry/Lotus.png      Lotus
boss/artwork/entry/Seren.png      Chosen Seren
boss/artwork/entry/Kalos.png      Gatekeeper Kalos
boss/artwork/entry/FA.png         The First Adversary
boss/artwork/entry/Carling.png    Carling
boss/artwork/entry/BM.png         Black Mage
boss/artwork/entry/MaleficStar.png       Radiant Malefic Star
boss/artwork/entry/Bellona.png    Bellona
boss/artwork/entry/Limbo.png      Limbo
boss/artwork/entry/Baldrix.png    Baldrix
boss/artwork/entry/Jupiter.png    Jupiter
```

`.png`, `.webp`, `.jpg` and `.jpeg` all work, tried in that order — the same
list portraits use. Unlike a portrait there is no `bosses.yaml` override to go
with it: one splash per boss, named by its key, is the whole rule.

One image per boss, shared by every difficulty. A run with two bosses shows
both: the run card splits its right edge diagonally, the first boss taking the
top corner and the second the bottom. The board's compact cards have room for
one picture and use the first boss's — the boss the run is named after, which is
the same rule the thumbnail on a Discord card follows. A boss with no file is
skipped rather than left as a gap, and a third boss is not shown at all.

**Kaling files go in as `Carling`.** The guild's boss key for the boss the game
calls Kaling is `Carling`, so its artwork is `Carling.png` and nothing else will
be found.

**Three of these are animated in game.** The First Adversary, Jupiter and
Radiant Malefic Star play a webm behind their entry prompt rather than showing a
still; `FA.png`, `Jupiter.png` and `MaleficStar.png` are poster frames pulled out of
those. Replacing one means extracting a new frame, not converting the video.

**Most of them have a name plate across the top.** The portal's crop opens below
that band on purpose, so the caption is never part of what shows. A replacement
whose plate sits lower than usual will need that crop moved — it is one number
per surface in `portal.css`, next to the wash's opacity.

**Nothing here is required.** A run whose lead boss has no file renders exactly
as it did before this directory existed — no layer at all, and nothing holding
space for one.

Not to be confused with [`../../portraits/`](../../portraits/README.md), which
is the boss's face: a portrait is drawn as a badge beside a name and attached to
the bot's Discord cards, and this is the backdrop a whole run card wears.

`boss/` is bind-mounted read-only into the container, so adding a file takes
effect on the next page load — no rebuild, and no restart for the portal. These
are wide rather than square (roughly 778×556) and are served straight from disk
at full size, so keep them to what a backdrop needs.

This directory is committed so the README survives; the images themselves are
git-ignored, since they are game assets.
