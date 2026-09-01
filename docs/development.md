# Development

```sh
uv sync                 # includes dev deps
uv run pytest           # unit tests, no Discord connection needed
uv run ruff check .
uv run ruff format .
```

If the project directory is ever moved or renamed, run `uv sync --reinstall`
once: the venv's console scripts (`.venv/bin/pytest` and friends) carry absolute
shebang paths, and a stale one fails as *bad interpreter* — after which the
shell quietly falls through to whatever `pytest` is next on `PATH`, which then
can't import the project's dependencies.

Tests cover the pure layers — boss-week arithmetic, token parsing,
materialisation idempotency, reminder generation and pruning, the
reaction → RSVP → status transitions, and every stage of the extractor (gate,
resolve, merge, match, commit, cards). Discord I/O is kept thin on purpose so it
needs no mocking.

The portal and CLI are covered the same way: `tests/fake_bot.py` is a stand-in
client that records every Discord side effect, so the API routes, the auth paths,
each page's rendering (empty and populated) and the `bossctl` commands are all
exercised without a gateway. `tests/test_api_server.py` starts the real uvicorn
server on a real port to prove it shares the loop rather than stealing it.

`uv run pytest` never touches the model. The fixture suite that does is marked
`ollama` and excluded by default; run it with `uv run pytest -m ollama`, which
skips itself if Ollama is unreachable.

## Layout

```
bot/
  __main__.py    entrypoint, login backoff, signal handling
  config.py      env settings (pydantic-settings)
  db.py          SQLite schema + repository
  weeks.py       boss-week arithmetic (pure)
  bosses.py      alias table + token parsing (pure)
  materialise.py fixed runs -> runs -> reminder rows (pure-ish)
  rsvp.py        reaction -> RSVP -> run status (pure)
  formatting.py  message and embed text (pure)
  watch.py       which channels the bot listens to (pure)
  client.py      discord.py client, tick loop, reactions
  commands.py    slash commands
  export.py      `python -m bot.export` -- channel history -> JSONL
  health.py      container healthcheck (heartbeat + /healthz)
  cli.py         `bossctl` -- the Typer CLI, over the same HTTP API
  extract/       the chat extractor
    gate.py      keyword gate + boss-token finder (pure, no model)
    prompt.py    the prompt: boss table, channel runs, roster, messages
    schema.py    the JSON schema the model is constrained to (pydantic)
    llm.py       one guarded, serialised call to Ollama
    merge.py     per-message pieces -> one candidate per run (pure)
    resolve.py   "weds"/"1030~11+pm" -> a datetime (pure)
    match.py     amendment -> the run it is about (pure)
    pipeline.py  per-channel buffering and the whole flow
    commit.py    ✅ on a card -> the schedule change (pure repo work)
    __main__.py  `python -m bot.extract` -- offline dry run over an export
  chat/          the chatbot (see chatbot.md)
    gate.py      answer or ignore: channel, mention, role, rate limit (pure)
    ratelimit.py per-person sliding window (pure)
    persona.py   PERSONA_PATH + the hard rules -> the system prompt
    tools.py     the tool schemas and the dispatcher over api/service.py
    agent.py     context assembly and the Ollama tool loop
  api/           the portal + CLI API, served on the bot's own loop
    server.py    uvicorn as a task next to discord.py; start/stop
    app.py       the FastAPI app, built around the live client
    auth.py      bearer token, tailnet identity, signed session cookie
    service.py   what the API does, in repository terms (no FastAPI here)
    routes_api.py  JSON under /api
    routes_web.py  the portal's pages and HTMX partials
    models.py    request/response shapes
    templating.py the Jinja environment
    templates/   Jinja pages and partials
    static/      portal.css
config/bosses.yaml
tests/
```
