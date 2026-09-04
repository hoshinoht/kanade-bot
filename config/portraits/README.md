# Boss portraits (optional)

Drop an image in here named after the **boss key** in
[`../bosses.yaml`](../bosses.yaml) — not the canonical run name — and the portal
shows it next to that boss everywhere, and the bot attaches it as the thumbnail
on that run's day-of and countdown pings.

```
config/portraits/Lotus.png      Lotus
config/portraits/Seren.png      Chosen Seren
config/portraits/Kalos.png      Gatekeeper Kalos
config/portraits/FA.png         The First Adversary
config/portraits/Carling.png    Carling
config/portraits/BM.png         Black Mage
config/portraits/Star.png       Radiant Malefic Star
config/portraits/Bellona.png    Bellona
config/portraits/Limbo.png      Limbo
config/portraits/Baldrix.png    Baldrix
config/portraits/Jupiter.png    Jupiter
```

`.png`, `.webp`, `.jpg` and `.jpeg` all work, tried in that order. One image per
boss, shared by every difficulty — the difficulty is shown as a pill next to it.

## Two sizes

`config/portraits/icon/Star.png` is the **small render** — the 64×64 files the
portal drew before the pictures above became full-size art. Everything the
portal draws is a badge (26px beside a boss's name, 38px in the boss grid), so
it asks for these; the full picture is what the bot attaches to a card in
Discord, and what `/static/portraits/<key>` still serves without `?size=icon`.

```
config/portraits/Star.png       the full picture — Discord's card thumbnail
config/portraits/icon/Star.png  the badge the portal draws
```

A missing icon falls back to the full picture, so a boss added today looks right
before anybody has cropped one — and an `icon/` directory that does not exist at
all, which is what a fresh clone has, simply means every badge draws the full
file. Filename-by-boss-key only in here: the `portrait:` override below names
the full picture and has no counterpart for the small one.

To use a different filename for the full picture, name it in `bosses.yaml`:

```yaml
  Star:
    full: Radiant Malefic Star
    portrait: radiant-malefic-star.webp
```

**Nothing here is required.** A boss with no portrait gets a coloured monogram
badge instead, so the layout is the same either way.

`config/` is bind-mounted read-only into the container, so adding a file takes
effect on the next page load — no rebuild, and no restart for the portal. Keep
them small (a 128×128 square is plenty); they are served straight from disk.

This directory is committed so the README survives; the images themselves are
git-ignored, since they are game assets.
