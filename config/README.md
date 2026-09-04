# Configuration

Game assets and deployment-specific settings live here. This directory is
bind-mounted into the container read-only, so any file you add or edit takes
effect on the next page load or command — no rebuild required.

## What's here

| File | Purpose | Committed? |
|---|---|---|
| `bosses.yaml` | Boss table — names, levels, difficulties, aliases | Yes |
| `portraits/` | Boss portrait images (icon + full-size) | Dir only; images git-ignored |
| `artwork/` | Entry splash art per boss | Dir only; images git-ignored |
| `guide.yaml` | Messages posted by `bossctl guide` | No — git-ignored |
| `guide_bosses.yaml` | Boss entries for the guide section | No — git-ignored |

## Guide setup

`bossctl guide` reads two YAML files and posts the server's guide channel
messages to Discord.

### Quick start

```bash
cp config/guide.example.yaml config/guide.yaml
cp config/guide_bosses.example.yaml config/guide_bosses.yaml
# edit the copies to match your server
bossctl guide --channel <channel-id>
```

### `config/guide.yaml`

A list of messages posted in order. Each entry is a markdown string. Use
Discord-flavoured markdown: `**bold**`, `` `code` ``, `> blockquotes`,
`-# spoiler lines`, `<#channel_id>` mentions, and emoji.

The special key `bosses: true` marks the boss list message. The CLI replaces
that line with per-boss entries generated from `guide_bosses.yaml` and
attaches portrait PNGs as file uploads.

```yaml
messages:
  - |
    ## First message
    Markdown content here.

  - |
    ## Second message
    More content.

  - |
    ## 📖 The bosses
    Text before the boss list.

    bosses: true
```

### `config/guide_bosses.yaml`

Boss display entries for the guide's boss section. Each entry needs:

```yaml
bosses:
  - name: Chosen Seren      # display name
    level: 260              # boss level
    tokens: [nseren, hseren, xseren]
    named: Normal · Hard · Extreme   # human-readable difficulties
    aliases: seren, serene, sereen, chosenseren
    portrait: Seren          # matches config/portraits/icon/Seren.png
```

The `portrait` field is the filename stem — the CLI looks for
`config/portraits/icon/<stem>.png` and attaches it to the Discord message.

### Adding a new boss

1. Add the portrait to `config/portraits/icon/`
2. Add an entry to `guide_bosses.yaml`
3. Run `bossctl guide --channel <id>` to re-post

### Channel mention syntax

Discord channel mentions use `<#channel_id>`. Find the ID by right-clicking
the channel in Discord (with Developer Mode enabled) and copying it.
