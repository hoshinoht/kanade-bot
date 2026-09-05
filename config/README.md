# Configuration

Deployment-specific guide prose and persona settings live here. The canonical
boss catalog, game assets, and chat knowledge live in [`../boss/`](../boss/README.md).

## What's here

| File | Purpose | Committed? |
|---|---|---|
| `guide.yaml` | Messages posted by `bossctl guide` | No — git-ignored |
| `personas/` | Deployment-specific chat identity and behaviour | Templates only |

## Guide setup

`bossctl guide` reads this directory's prose and derives the boss entries from
the canonical [`boss/bosses.yaml`](../boss/bosses.yaml) catalog before posting to
Discord.

### Quick start

```bash
cp config/guide.example.yaml config/guide.yaml
# edit the copy to match your server
bossctl guide --channel <channel-id>
```

### `config/guide.yaml`

A list of messages posted in order. Each entry is a markdown string. Use
Discord-flavoured markdown: `**bold**`, `` `code` ``, `> blockquotes`,
`-# spoiler lines`, `<#channel_id>` mentions, and emoji.

The special key `bosses: true` marks the boss list message. The CLI replaces
that line with per-boss entries generated from `boss/bosses.yaml` and attaches
available portrait files as uploads.

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

### Adding a new boss

1. Add the catalog entry and optional portrait under [`boss/`](../boss/README.md).
2. If the chatbot is configured, add its required knowledge document too.
3. Restart the bot so it loads the changed catalog and knowledge, then run
   `bossctl guide --channel <id>` to re-post.

Use `bossctl guide --bosses PATH` to derive guide entries from an explicit
catalog instead of `BOSSES_PATH`.

### Channel mention syntax

Discord channel mentions use `<#channel_id>`. Find the ID by right-clicking
the channel in Discord (with Developer Mode enabled) and copying it.
