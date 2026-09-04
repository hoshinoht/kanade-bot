# Personas

Prompt content is split by responsibility:

```text
identities/example.md                 tracked identity template
identities/persona.md                 live identity, git-ignored
behaviours/default.example.md         tracked default-behaviour template
behaviours/default.md                 live default behaviour, git-ignored
behaviours/profiles/example.md        tracked reply-profile template
behaviours/profiles/<name>.md         live profiles, git-ignored
```

Create the live files and restart:

```sh
cp personas/identities/example.md personas/identities/persona.md
cp personas/behaviours/default.example.md personas/behaviours/default.md
docker compose restart bot
```

The directory is bind-mounted at `/app/personas`. `PERSONA_PATH` seeds the
identity filename for a fresh database; later identity choices are stored in
SQLite and can be changed from **Config → Chatbot** without restarting. New
paths take precedence, but legacy root identities and `behaviour-plugins/`
profiles remain readable during migration.

Identity says who the assistant is. Default behaviour defines normal delivery.
Profiles replace that delivery for one reply. Code-owned files under
`bot/chat/prompts/` define assistant scope, scheduler authority, grounding, and
privacy; deployment files cannot override them.

Put a one-line `**Voice:** ...` cue and any `**Good**` worked examples in the
default behaviour or profile. An active profile's examples replace the default
examples rather than combining with them.

## Reply profiles

Create and edit profiles in **Config → Chatbot**. Publishing adds a profile to
the `/style` catalog. Members can save only published, readable profiles, and
`/style default` clears their choice.

Role assignments are ordered. The first readable profile whose Discord role the
member currently holds applies to the next reply; otherwise the saved choice or
default behaviour applies. Role assignments never grant chatbot access.

The portal shows both the saved choice and the style that would apply next.
Member-facing replies describe a choice only as saved and do not reveal role
assignment metadata.

Only templates and this README are tracked. Keep live identities, behaviours,
profiles, and maintainer reference notes private.
