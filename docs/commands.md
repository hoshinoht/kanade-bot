# Commands

The slash commands, how ids and boss tokens work, and the `/debug` group
for testing the whole flow on demand.

```
/fixed add bosses:hstar, hfa day:Mon time:21:30 member1:@Alvin member2:@Priya
                                # run this in your party's channel - that becomes
                                # the run's home channel, where its pings go.
                                # member1..member6 open Discord's member picker;
                                # you are added automatically.
/fixed list
/fixed edit id:a1b2c3d4 time:22:00
/fixed edit id:a1b2 member1:@Alvin member2:@kanon   # replaces the participant list
/fixed edit id:1 channel:#other-party   # move where its pings go
/fixed remove id:a1b2c3d4

/schedule                       # in a party channel: that channel's runs
                                # elsewhere: your runs (on them, or you own the
                                # timing). Public, never pings. Runs that have
                                # already happened are hidden.
/schedule scope:mine|all|channel week:this|next show_past:True

/amend run_id:a1b2 to:wed 21:30  # understands "tomorrow 9:45pm", "in 2 hours"
/status run_id:a1b2 state:...   # planned · confirmed · own time · done · cancelled
/cancel run_id:a1b2c3d4         # shortcuts for the same thing
/otot run_id:a1b2c3d4           # own time: stays in the morning ping, no countdowns
/restore run_id:a1b2c3d4        # put a cancelled/own-time/finished run back on
/done run_id:a1b2c3d4           # cleared early
/swap run_id:a1b2 out:@Priya in:@kanon  # this week only; the timing is unchanged
/rsvp run_id:a1b2c3d4 answer:no

/nick user:@harbour4417 alias:MY  # chat nickname, used by the extractor
/pingtime time:08:30            # move the morning ping, reschedules pending ones
/bot pause | /bot resume        # stop/resume chat watching
/rescan hours:24                # re-read this channel's recent chat and propose

/say message:... [channel:#x]   # admins only: the bot posts your words verbatim
```

**`/say`** is the one thing the bot writes that really notifies people: the
allow-list is built from the `@mentions` you type, so it reaches exactly who you
name (roles included) and nobody else. `@everyone`/`@here` is always blocked, and
quiet mode silences it like everything else. It only posts where the bot can
already post, and it never falls back to another channel.


**About ids.** Runs and fixed runs are identified by a UUID, shown as its first
eight characters — `#a1b2c3d4`. You almost never type one: every command that
takes an id has a **dropdown** listing your runs as
`HStar + HFA · Mon 21:30 · #hstar-alvin-kanon · a1b2c3d4`. If you do type one,
any unique prefix of four characters or more works, case-insensitively, with or
without the `#` — so you can paste `#a1b2c3d4` straight out of `/schedule`. An
ambiguous prefix comes back with the candidates listed.

Boss tokens always need a difficulty prefix: `e`asy, `n`ormal, `h`ard, `c`haos,
e`x`treme. `hstar`, `HFA`, `xkalos`, `ncarling`, `hbaldguy` all work. Two things
are rejected rather than guessed, both with the valid forms listed:

- a bare name — `kalos` → *missing a difficulty prefix (e/n/h/c/x) — try EKalos,
  NKalos, CKalos, XKalos*
- a difficulty that boss does not have — `hkalos` → *Gatekeeper Kalos has no Hard
  difficulty — did you mean EKalos, NKalos, CKalos, XKalos?* (so `cseren`, which
  looks like "Chosen Seren", is caught too)

`participants:` still accepts typed names as a fallback (`MY, alvin` — matched
against display names, server nicknames and `/nick` aliases); anything it can't
match, or that could mean two people, comes back as an error naming the problem.
The pickers are more reliable.

Only members with the bossing role can use the commands. Only a run's
participants, its owner, or `ADMIN_ROLE_ID` members can change it. `ADMIN_ROLE_ID`
is the "who runs the bot" role: it also gates `/say` and `/debug`, alongside
Discord's own Administrator permission and the server owner, either of which
works even when the setting is empty. Bot accounts
are never rostered and cannot be participants, even if they hold the role.


### Testing it

`/debug` posts the *real* reminder messages on demand so you can check the whole
flow without waiting for 09:00. Restricted to the server owner, `ADMIN_ROLE_ID`
members, server administrators, and ids in `DEBUG_USER_IDS` — and hidden from
everyone else's command picker.

```
/debug ping run_id:a1b2 kind:day_of   # posts "🧪 TEST — ..." in the run's home
                                      # channel with real ✅/❌ reactions
/debug reminders [run_id:a1b2]        # reminder rows: fire_at, sent, message ids
/debug tick                           # run the reminder tick right now
/debug materialise                    # force current+next week materialisation
/debug upcoming hours:24              # dry run: what would fire, nothing sent
/debug status                         # uptime, heartbeat, week, Ollama reachability
/debug extract hours:6                # run the extractor here and show its raw JSON
                                      # (ephemeral, and never posts a card)
/debug clear_test                     # delete this channel's 🧪 TEST messages (24h)
```

A `/debug ping` **never touches the run's reminder rows** — the scheduled ping
still goes out on time. Test messages are tracked in a separate `debug_messages`
table, so reacting ✅/❌ to one drives the real RSVP flow and you can watch a run
go `planned → confirmed` or `→ at_risk` end to end.
