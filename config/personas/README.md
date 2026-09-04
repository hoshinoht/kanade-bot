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
cp config/personas/identities/example.md config/personas/identities/persona.md
cp config/personas/behaviours/default.example.md config/personas/behaviours/default.md
docker compose restart bot
```

The directory is bind-mounted at `/app/config/personas`. `PERSONA_PATH` seeds the
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

# Designing an Identity File

`default.md` defines the bot's baseline conversational behaviour. Profiles layer
on top of it for individual replies, so the default should be compact, strong,
and broadly compatible with every profile.

The goal is not to describe the character exhaustively. The goal is to give the
model a small set of behavioural instincts that reliably affect how it phrases
every reply.

## 1. Define an attitude, not a character biography

Prefer:

> Add a shy, easily flustered, slightly clumsy gamer cat-maid edge to every reply.

Over:

> Yuuki is introverted, socially awkward, competitive, clumsy, warm, reserved,
> expressive, confident under pressure...

A short behavioural direction is easier to apply consistently than a long list
of personality attributes.

Profiles work well for the same reason: they establish a clear stance such as
“doting older sister”, “smug kouhai”, or “defensive tsundere”, then let that
stance shape the reply.

## 2. Give the character a conversational stance

Personality traits alone are often too abstract.

Define how the character approaches ordinary requests:

> Act like a reluctant but proud first-class maid who was probably busy gaming
> before being summoned.

This gives routine replies an obvious source of personality:

* mild complaints
* awkward reactions
* dry teasing
* gamer remarks
* quiet pride after doing something correctly

Without a stance, tool replies tend to collapse into generic assistant or system
language.

## 3. Keep the core prompt short

Long prompts with many micro-rules are not necessarily followed more reliably.

Too many instructions can cause the model to satisfy only obvious surface cues
such as `Yosh`, `Fine`, or `Mou...` while the underlying sentence remains
generic.

Prefer roughly:

1. core personality / stance
2. recurring mannerisms
3. a few representative examples
4. accuracy rule

Avoid turning `default.md` into a full character specification unless the extra
detail demonstrably improves behaviour.

## 4. Personality must affect the sentence itself

This is weak:

> `Yosh! Card's up for Extreme FA tonight 23:00. Grab it with ✅ to lock it in!`

It is still a generic notification with `Yosh` attached.

This is stronger:

> `Fine, Extreme FA tonight 23:30 for hoshi. Card's up — hit ✅ already~ I have dailies.`

The second reply expresses the character through word choice, rhythm, attitude,
and the aside.

A catchphrase, greeting, emoji, or Japanese expression added to neutral prose
does **not** count as successful persona application. This matches the failure
mode already identified in the original default examples.

## 5. Tool results are facts, not dialogue

Scheduler and tool output strongly encourage the model to simply paraphrase the
returned structure.

For example, a result containing:

> Extreme FA · 23:00 · hoshi · card created · needs ✅

naturally encourages:

> `Extreme FA tonight 23:00 for hoshi. Card is up and needs ✅ to lock it in.`

That is accurate but characterless.

The behaviour prompt should explicitly tell the model to preserve the facts while
expressing them naturally in character.

The existing stronger examples demonstrate the intended difference:

> `Fine, fine, card's posted for Normal Baldrix Tue 22:00. React ✅ already~ I have dailies to run.`

> `Moved. Wednesday 21:30 → Thursday 22:00. You owe me an omurice for fixing this schedule.`

## 6. Use examples that match real production replies

Few-shot examples should resemble the replies the bot actually produces:

* card creation
* confirmation
* amendments
* cancellation
* schedule lookup
* empty schedule results
* error / missing information

A generic personality example such as:

> `Hah?! That's not what I meant!`

helps establish voice, but it does much less for scheduler output than:

> `Fine, Extreme FA tonight 23:30 for hoshi. Card's up — hit ✅ already~ I have dailies.`

Keep only a few examples, but make them high-value.

## 7. Avoid generic assistant closure

Phrases such as:

* `Hope that helps!`
* `Hope that works!`
* `Let me know if you need anything else.`
* `Sure thing!`

strongly pull the reply toward generic assistant/customer-service register.

Prefer endings that arise naturally from the character:

> `Go hit ✅ before you forget.`

> `My part's done~`

> `Don't make me move it again.`

> `Reliable maid, see?`

Do not maintain a huge blacklist; a short instruction against generic assistant
politeness is usually enough.

## 8. Do not confuse shyness with incompetence

A shy or clumsy character does not need low self-esteem.

Avoid:

> `...guess I'm finally useful.`

> `Mou... hope that works.`

Prefer:

> `There. Reliable enough, see?`

> `Yosh, fixed. Nobody saw that.`

The useful contrast is:

**socially awkward / mundane clumsiness → competent when it matters**

not:

**insecure → seeking reassurance**

## 9. Let personality intensity vary naturally

The default should not force maximum character expression into every sentence.

Useful baseline:

* ordinary request → mild attitude
* comfortable conversation → more playful
* embarrassment → brief fluster
* gaming / optimisation → focused confidence
* success → brief smugness
* mistake → awkward correction, then move on

The original default already aimed for concise Discord-style replies and sparse
Japanese flavour; those constraints are useful as long as they do not suppress
the core attitude.

## 10. Profiles should amplify or redirect, not repair the default

The default should already produce recognisable character voice.

Profiles then add a stronger relationship or trope:

* kouhai → smug junior / `Senpai`
* onee-san → doting older sister
* tsundere → defensive caring
* imouto → playful younger sister

Do not rely on profiles to make otherwise neutral output expressive.

## 11. Preserve factual grounding

Characterisation never overrides correctness.

Only tease using facts from:

1. the current message,
2. established conversation context, or
3. tool results.

Never invent failures, DPS, deaths, schedules, people, dates, or other facts for
a joke. Preserve names, IDs, dates, times, and scheduler facts exactly. This is
also the core safety rule used by the existing profile system.

## Recommended shape

A reliable behaviour file usually looks like:

```md
# Default behaviour

Add a <clear personality edge> to every reply.

Act like <strong conversational stance>. Explain how that affects ordinary
requests, embarrassment, success, and relevant domain-specific situations.

Natural recurring reactions include <small set of mannerisms>. Keep them
sparse; personality should come from the wording itself rather than catchphrases
attached to neutral responses.

The target tone includes lines like:
- "<high-value real reply example>"
- "<high-value real reply example>"
- "<high-value real reply example>"

For tool and scheduler replies, preserve all facts exactly but express them
naturally in character rather than as system output or generic assistant prose.

Accuracy overrides the joke. <grounding rule>
```

## Practical rule of thumb

If removing the catchphrase from a reply makes it sound like any generic bot,
the persona was not applied strongly enough.

If the reply remains recognisably in character after removing `Yosh`,
`Kon-sakuna~`, emojis, and other surface markers, the behaviour prompt is doing
its job.
