# Personas

The document that tells the chatbot who it is. Everything the bot *knows* comes
from tools; everything it *sounds like* comes from here.

```
personas/persona.example.md   the tracked template — placeholders, no real names
personas/persona.md           yours, git-ignored, and what the bot loads
```

Copy the template, fill it in, and restart:

```
cp personas/persona.example.md personas/persona.md
docker compose restart bot
```

This directory is bind-mounted read-only into the container, so editing a
persona on the Mac and restarting is the whole loop — no rebuild, and nothing
to copy into a volume.

**Switching between voices needs no restart.** Keep as many files here as you
like; the Config page's Chatbot panel lists every `.md` in this directory and
the one you pick takes effect on the bot's next answer. Adding a voice is still
a file drop — it is in the dropdown on the next page load.

`PERSONA_PATH` in `.env` is the seed rather than the switch: `compose.yaml`
points it at `/app/personas/persona.md`, and its filename is what the setting
starts on. From then on the choice is stored with the rest of the runtime
config, so a restart does not undo it.

**Only the README and the template are tracked.** A real persona names a
character, a community and its in-jokes, none of which belongs in a public
repository — the same reason `config/portraits/` keeps only its README. Keep
the template neutral if you edit it.

**Nothing here is required.** With no persona file the bot loads the template
and logs a WARNING, and the Config page's Chatbot panel says *fallback:
persona.example.md* — a deployment answering in the placeholder voice is a
misconfiguration, so it is made visible rather than left in the logs.

The whole file goes into the system prompt verbatim, so keep it to a few
thousand tokens: a short sharp persona outperforms a long one. The template
explains which sections actually steer the model, and the `**Voice:**` line is
the one the bot repeats last, immediately before it composes each reply.

The persona never contains a rule about what the bot may *do*. Those are in
`bot/chat/persona.py` as hard rules appended after this document, and the
channel and role gates are enforced before a prompt is built — a model cannot
leak a rule it was never shown.
