"""``bossctl`` -- the command-line front end.

It talks to the same HTTP API the portal does (DESIGN.md §5), so it works from
the host (``uv run bossctl ...``) and from inside the container
(``docker compose exec bot bossctl ...``) without a second copy of the
scheduling logic and without touching the SQLite file the bot has open.

Configuration is two values: ``BOSSCTL_URL`` (default ``http://127.0.0.1:8080``)
and ``ADMIN_TOKEN``, read from the environment or from the project's ``.env`` --
the same file the bot reads, so there is nothing extra to set up.

Ids may be given as any unique prefix; the server resolves them, and says so
when a prefix is ambiguous.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any

import httpx
import typer
from dotenv import dotenv_values
from rich.console import Console
from rich.table import Table
from rich.text import Text

DEFAULT_URL = "http://127.0.0.1:8080"
#: Ordinary calls are local and instant; the read budget covers a `digest` or a
#: `ping` waiting on Discord.
TIMEOUT = httpx.Timeout(10.0, read=60.0)
#: A week-wide rescan is one model call per conversation, and `gpt-oss:20b`
#: takes 10-40 s each on this Mac. Ten minutes is deliberately generous -- the
#: alternative is the CLI giving up on work the bot then finishes anyway.
RESCAN_TIMEOUT = httpx.Timeout(10.0, read=900.0)

console = Console()
err = Console(stderr=True)

#: Run status -> how it reads in a terminal.
STATUS_STYLE = {
    "planned": "yellow",
    "confirmed": "green",
    "at_risk": "red",
    "otot": "cyan",
    "cancelled": "dim",
    "done": "dim",
}


class ApiFailed(Exception):
    """The API said no; the message is meant to be printed as-is."""


# ---------------------------------------------------------------------------
# configuration
# ---------------------------------------------------------------------------


def find_env(start: Path | None = None) -> Path | None:
    """The nearest ``.env`` at or above the working directory."""
    here = (start or Path.cwd()).resolve()
    for directory in (here, *here.parents):
        candidate = directory / ".env"
        if candidate.is_file():
            return candidate
    return None


def _clean(value: str | None) -> str:
    """A value that is nothing but a comment means "unset", not that literal text."""
    text = (value or "").strip()
    return "" if text.startswith("#") else text


def load_token() -> str:
    token = _clean(os.environ.get("ADMIN_TOKEN"))
    if token:
        return token
    env_file = find_env()
    if env_file is not None:
        token = _clean(dotenv_values(env_file).get("ADMIN_TOKEN"))
    if not token:
        raise ApiFailed(
            "no ADMIN_TOKEN. Set it in the environment, or run bossctl from the project "
            "directory so it can read .env. Generate one with `openssl rand -hex 32`."
        )
    return token


def base_url() -> str:
    url = _clean(os.environ.get("BOSSCTL_URL"))
    if not url:
        env_file = find_env()
        if env_file is not None:
            url = _clean(dotenv_values(env_file).get("BOSSCTL_URL"))
    return (url or DEFAULT_URL).rstrip("/")


class Api:
    """A thin httpx wrapper that turns an error response into :class:`ApiFailed`."""

    def __init__(
        self, url: str | None = None, token: str | None = None, timeout: httpx.Timeout | None = None
    ):
        self.url = url or base_url()
        self.token = token if token is not None else load_token()
        self.timeout = timeout or TIMEOUT

    def _client(self) -> httpx.Client:
        return httpx.Client(
            base_url=self.url,
            headers={"Authorization": f"Bearer {self.token}"},
            timeout=self.timeout,
        )

    def request(self, method: str, path: str, **kwargs: Any) -> Any:
        try:
            with self._client() as client:
                response = client.request(method, path, **kwargs)
        except httpx.HTTPError as exc:
            raise ApiFailed(
                f"can't reach the bot at {self.url}: {exc}. Is the container running "
                "(`docker compose ps`)?"
            ) from None
        if response.status_code >= 400:
            raise ApiFailed(self._message(response))
        if not response.content:
            return None
        return response.json()

    @staticmethod
    def _message(response: httpx.Response) -> str:
        try:
            payload = response.json()
        except ValueError:
            return f"HTTP {response.status_code}: {response.text.strip()[:300]}"
        if isinstance(payload, dict) and "error" in payload:
            return str(payload["error"])
        return f"HTTP {response.status_code}: {payload}"

    def get(self, path: str, **params: Any) -> Any:
        return self.request("GET", path, params={k: v for k, v in params.items() if v is not None})

    def post(self, path: str, body: dict | None = None) -> Any:
        return self.request("POST", path, json=body)

    def patch(self, path: str, body: dict) -> Any:
        return self.request("PATCH", path, json=body)

    def delete(self, path: str) -> Any:
        return self.request("DELETE", path)

    def stream_lines(self, path: str, **params: Any):
        """Yield an NDJSON response line by line, for the message export."""
        cleaned = {k: v for k, v in params.items() if v is not None}
        try:
            with self._client() as client, client.stream("GET", path, params=cleaned) as response:
                if response.status_code >= 400:
                    response.read()
                    raise ApiFailed(self._message(response))
                for line in response.iter_lines():
                    if line.strip():
                        yield line
        except httpx.HTTPError as exc:
            raise ApiFailed(f"can't reach the bot at {self.url}: {exc}") from None


def api() -> Api:
    return Api()


# ---------------------------------------------------------------------------
# rendering
# ---------------------------------------------------------------------------


def status_text(status: str, label: str | None = None) -> Text:
    return Text(label or status, style=STATUS_STYLE.get(status, ""))


def print_runs(schedule: dict) -> None:
    # A boss week is materialised whole, so the API hides runs that have already
    # happened; say how many rather than letting the count look wrong.
    hidden = schedule.get("hidden") or 0
    note = f" · {hidden} past/cancelled hidden, --all-statuses to see them" if hidden else ""
    if not schedule["days"]:
        console.print(f"[dim]Nothing left in the week of {schedule['week_label']}{note}.[/dim]")
        return
    console.print(
        f"[bold]Boss week of {schedule['week_label']}[/bold] "
        f"[dim]({schedule['count']} run(s), all times {schedule['timezone']}){note}[/dim]"
    )
    for day in schedule["days"]:
        table = Table(title=day["heading"], title_justify="left", title_style="bold", box=None)
        table.add_column("id", style="dim", no_wrap=True)
        table.add_column("time", no_wrap=True)
        table.add_column("bosses")
        table.add_column("party")
        table.add_column("status", no_wrap=True)
        table.add_column("rsvp", no_wrap=True)
        for run in day["runs"]:
            table.add_row(
                run["short_id"],
                "own time" if run["status"] == "otot" else run["local_time"],
                " + ".join(run["bosses"]),
                ", ".join(p["name"] for p in run["participants"]),
                status_text(run["status"], run["status_label"]),
                f"{run['yes']}/{len(run['participants'])}"
                + (f" · {run['no']}✗" if run["no"] else ""),
            )
        console.print(table)
        console.print()


def print_table(title: str, columns: list[str], rows: list[list[Any]]) -> None:
    table = Table(title=title, title_justify="left", title_style="bold", box=None)
    for index, name in enumerate(columns):
        table.add_column(name, style="dim" if index == 0 else "", no_wrap=index == 0)
    for row in rows:
        table.add_row(*row)
    console.print(table)


def fail(message: str) -> None:
    err.print(f"[red]✗[/red] {message}")
    raise typer.Exit(code=1)


# ---------------------------------------------------------------------------
# the app
# ---------------------------------------------------------------------------

app = typer.Typer(
    help="Control the boss-scheduler bot over its local HTTP API.",
    no_args_is_help=True,
    add_completion=False,
)
fixed_app = typer.Typer(help="The weekly baseline timings.", no_args_is_help=True)
config_app = typer.Typer(help="Runtime settings.", no_args_is_help=True)
app.add_typer(fixed_app, name="fixed")
app.add_typer(config_app, name="config")


@app.command()
def schedule(
    week: str = typer.Option("this", "--week", "-w", help="`this` or `next`."),
    all_: bool = typer.Option(False, "--all", help="Every party (the default)."),
    channel: str | None = typer.Option(None, "--channel", help="Only this home channel."),
    user: str | None = typer.Option(None, "--user", help="Runs this member is on or owns."),
    boss: str | None = typer.Option(None, "--boss", help="Substring of a boss token."),
    all_statuses: bool = typer.Option(
        False, "--all-statuses", help="Include runs that already happened, and cancelled ones."
    ),
) -> None:
    """Show a boss week's runs, grouped by day. Past and cancelled runs are hidden."""
    del all_  # showing everything is already the default; the flag reads better
    print_runs(
        api().get(
            "/api/schedule",
            week=week,
            channel=channel,
            user=user,
            boss=boss,
            show_past=all_statuses or None,
        )
    )


@app.command()
def pending() -> None:
    """Changes the extractor proposed that nobody has answered."""
    rows = api().get("/api/pending")
    if not rows:
        console.print("[dim]Nothing waiting.[/dim]")
        return
    for item in rows:
        names = item["bosses"] or (item["run"]["bosses"] if item["run"] else [])
        console.print(
            f"[bold]{item['short_id']}[/bold]  {item['kind_label']}"
            f"{'  ' + ' + '.join(names) if names else ''}"
            f"  [dim]confidence {item['confidence'] if item['confidence'] is not None else '?'}"
            f" · {item['channel_name'] or item['channel_id'] or 'no channel'}[/dim]"
        )
        if item["run"]:
            console.print(f"    {item['run']['local_day']} {item['run']['local_time']} → ", end="")
        console.print(f"    [bold]{item['when']}[/bold]" if not item["run"] else item["when"])
        for line in item["evidence"]:
            if line["missing"]:
                continue
            console.print(
                f"      [dim]{line['local_time']}[/dim] {line['author_name']}: {line['content']}"
            )
        console.print()


@app.command()
def approve(
    amendment_id: str = typer.Argument(help="Proposal id, or any unique prefix."),
    actor: str | None = typer.Option(None, "--actor", help="Discord id to credit."),
) -> None:
    """Apply a proposed change, exactly as ✅ on its Discord card would."""
    result = api().post(
        f"/api/amendments/{amendment_id}/approve", {"actor_id": actor} if actor else {}
    )
    console.print(f"[green]✓[/green] Applied the {result['kind']} ({result['short_id']}).")


@app.command()
def reject(amendment_id: str = typer.Argument(help="Proposal id, or any unique prefix.")) -> None:
    """Reject a proposed change."""
    result = api().post(f"/api/amendments/{amendment_id}/reject")
    console.print(f"[green]✓[/green] Rejected {result['short_id']}.")


@app.command()
def amend(
    run: str = typer.Argument(help="Run id, or any unique prefix."),
    to: str = typer.Option(..., "--to", help='e.g. "wed 21:30", "tomorrow 9:45pm".'),
) -> None:
    """Move a run to a new day/time."""
    result = api().post(f"/api/runs/{run}/amend", {"to": to})
    console.print(
        f"[green]✓[/green] {' + '.join(result['bosses'])} → "
        f"{result['local_day']} {result['local_time']}."
    )


@app.command()
def cancel(run: str = typer.Argument(help="Run id, or any unique prefix.")) -> None:
    """Cancel a run for this week."""
    result = api().post(f"/api/runs/{run}/cancel")
    console.print(f"[green]✓[/green] Cancelled {' + '.join(result['bosses'])}.")


@app.command()
def otot(run: str = typer.Argument(help="Run id, or any unique prefix.")) -> None:
    """Own time: keeps the morning ping, drops the countdowns."""
    result = api().post(f"/api/runs/{run}/otot")
    console.print(f"[green]✓[/green] {' + '.join(result['bosses'])} is own-time.")


@app.command()
def restore(run: str = typer.Argument(help="Run id, or any unique prefix.")) -> None:
    """Put a cancelled, own-time or finished run back on the schedule."""
    result = api().post(f"/api/runs/{run}/restore")
    console.print(
        f"[green]✓[/green] {' + '.join(result['bosses'])} is back on for "
        f"{result['local_day']} {result['local_time']}."
    )


@app.command()
def status(
    run: str = typer.Argument(help="Run id, or any unique prefix."),
    state: str = typer.Argument(help="planned, confirmed, otot, done or cancelled."),
) -> None:
    """Set a run's status. `at_risk` is derived from answers and cannot be set."""
    result = api().patch(f"/api/runs/{run}/status", {"status": state})
    console.print(
        f"[green]✓[/green] {' + '.join(result['bosses'])} is now "
        f"{status_text(result['status'], result['status_label'])}."
    )


@app.command()
def rsvp(
    run: str = typer.Argument(help="Run id, or any unique prefix."),
    answer: str = typer.Argument(help="yes, no or maybe."),
    user: str = typer.Option(..., "--user", "-u", help="Discord user id."),
) -> None:
    """Record someone's answer for a run."""
    result = api().post(f"/api/runs/{run}/rsvp", {"user_id": user, "answer": answer})
    console.print(
        f"[green]✓[/green] Noted: {answer} — {result['yes']}/{len(result['participants'])} on, "
        f"now {result['status_label']}."
    )


@app.command()
def access() -> None:
    """What the bot may actually do in each watched channel."""
    rows = api().get("/api/access")
    if not rows:
        fail("the bot isn't connected to the guild, so its permissions can't be checked")
    tick = {True: "[green]✅[/green]", False: "[red]❌[/red]"}
    print_table(
        f"{len(rows)} channel(s)",
        ["channel", "id", "see", "post", "history", "embeds", "react"],
        [
            [
                row["name"] + (" (digest)" if row["is_digest_channel"] else ""),
                row["id"],
                tick[row["view"]],
                tick[row["send"]],
                tick[row["history"]],
                tick[row["embed"]],
                tick[row["react"]],
            ]
            for row in rows
        ],
    )
    missing = [r["name"] for r in rows if not (r["view"] and r["send"])]
    if missing:
        console.print(
            f"[red]{', '.join(missing)}[/red]: the bot cannot post there, so those runs get "
            "no reminders. Fix it in Edit Channel → Permissions."
        )


@app.command()
def members() -> None:
    """The roster, as synced from the bossing role."""
    rows = api().get("/api/members")
    print_table(
        f"{len(rows)} member(s)",
        ["user id", "name", "server nickname", "chat aliases", "runs this week"],
        [
            [
                m["user_id"],
                m["display_name"],
                m["nickname"] or "—",
                ", ".join(m["aliases"]) or "—",
                str(m["runs_this_week"]),
            ]
            for m in rows
        ],
    )


@app.command()
def nick(
    user: str = typer.Argument(help="Discord user id."),
    alias: str = typer.Argument(help="What they get called in chat, e.g. MY."),
) -> None:
    """Attach a chat alias to a member, so the extractor recognises the name."""
    result = api().post(f"/api/members/{user}/nick", {"alias": alias})
    console.print(
        f"[green]✓[/green] {result['name']} is also known as: {', '.join(result['aliases'])}"
    )


@app.command()
def reminders(run: str | None = typer.Option(None, "--run", help="Limit to one run.")) -> None:
    """Reminder rows: what has fired, and what is queued."""
    rows = api().get("/api/reminders", run_id=run)
    print_table(
        f"{len(rows)} reminder(s)",
        ["run", "kind", "fires", "sent", "bosses"],
        [
            [
                r["run_short_id"],
                r["kind"],
                r["local_fire_at"],
                "yes" if r["sent_at"] else "queued",
                " + ".join(r["bosses"]) or "—",
            ]
            for r in rows
        ],
    )


@app.command()
def rescan(
    channel: list[str] = typer.Option(
        None, "--channel", "-c", help="Channel id; repeatable. Default: every watched channel."
    ),
    window: str = typer.Option(
        "week", "--window", "-w", help="week, 2weeks, 48h or 24h (default: this boss week)."
    ),
    post: bool = typer.Option(True, "--post/--dry-run", help="Post a card for what it finds."),
) -> None:
    """Pull the party channels' history from Discord, re-read it, and propose changes."""
    channels = list(channel or [])
    console.print(
        f"[dim]Reading {window} of "
        f"{', '.join(channels) if channels else 'every watched channel'}. "
        "One model call per conversation, one channel at a time — this can take a few "
        "minutes…[/dim]"
    )
    result = Api(timeout=RESCAN_TIMEOUT).post(
        "/api/rescan", {"channels": channels, "window": window, "post": post}
    )
    print_table(
        f"{len(result['channels'])} channel(s) in {result['elapsed_ms'] / 1000:.1f}s",
        ["channel", "pulled", "worth reading", "conversations", "cards", "time", ""],
        [
            [
                row["channel_name"],
                str(row["backfilled"]),
                str(row["gated"]),
                str(row["bursts"]),
                str(row["proposals"]),
                f"{row['elapsed_ms'] / 1000:.1f}s",
                row["error"]
                or ("checked last week too" if row["widened"] else "")
                or (f"{row['stale']} already passed" if row["stale"] else ""),
            ]
            for row in result["channels"]
        ],
    )
    if result["errors"]:
        fail(f"the model didn't answer: {result['errors'][0]}")
    if not result["asked"]:
        console.print("[dim]Nothing looked like scheduling, so the model wasn't asked.[/dim]")
        return
    console.print(
        f"{result['proposals']} card(s) posted, {result['dropped']} dropped "
        f"({result['stale']} already passed)."
    )
    for item in result["proposed"]:
        console.print(
            f"  • {item['kind']} {' + '.join(item['bosses']) or ''} "
            f"[dim]({item['confidence']:.2f})[/dim]"
        )


@app.command()
def channels() -> None:
    """The channels a rescan covers, party channels first."""
    rows = api().get("/api/rescan/targets")
    print_table(
        f"{len(rows)} watched channel(s)",
        ["id", "channel", "has runs"],
        [[row["id"], row["name"], "yes" if row["has_runs"] else "—"] for row in rows],
    )


@app.command()
def digest(
    channel: str | None = typer.Option(None, "--channel", help="Defaults to POST_CHANNEL_ID."),
    week: str = typer.Option("this", "--week", help="`this` or `next`."),
) -> None:
    """Post the whole guild's week to Discord now."""
    result = api().post("/api/digest", {"channel_id": channel, "week": week})
    console.print(f"[green]✓[/green] Posted the {result['week']}-week digest: {result['url']}")


@app.command()
def ping(
    run: str = typer.Argument(help="Run id, or any unique prefix."),
    kind: str = typer.Argument(
        "day_of", help="day_of, countdown_60, countdown_15, amend, decline."
    ),
) -> None:
    """Post one real reminder now, marked 🧪 TEST, without touching the schedule."""
    result = api().post("/api/debug/ping", {"run_id": run, "kind": kind})
    console.print(f"[green]✓[/green] Posted a {kind} test: {result['url']}")


@app.command()
def extractions(limit: int = typer.Option(25, "--limit", "-n")) -> None:
    """The model call log -- the prompt-tuning tool."""
    rows = api().get("/api/extractions", limit=limit)
    print_table(
        f"{len(rows)} call(s)",
        ["id", "when", "model", "latency", "messages", "changes"],
        [
            [
                r["short_id"],
                r["local_time"],
                r["model"],
                f"{r['latency_ms']} ms" if r["latency_ms"] is not None else "—",
                str(len(r["message_ids"])),
                str(r["amendment_count"]),
            ]
            for r in rows
        ],
    )


@app.command()
def extraction(
    extraction_id: str = typer.Argument(help="Extraction id, or any unique prefix."),
    show_prompt: bool = typer.Option(True, "--prompt/--no-prompt", help="Include the prompt."),
) -> None:
    """One model call in full: prompt, raw JSON, and what it produced."""
    row = api().get(f"/api/extractions/{extraction_id}")
    console.print(
        f"[bold]{row['short_id']}[/bold]  {row['local_time']}  {row['model']}  "
        f"[dim]{row['latency_ms']} ms[/dim]"
    )
    for item in row["amendments"]:
        console.print(
            f"  • {item['kind']} {' + '.join(item['bosses'])} → {item['when']} "
            f"[dim]({item['status']})[/dim]"
        )
    if show_prompt:
        console.print("\n[bold]Prompt[/bold]")
        console.print(row["prompt"], highlight=False, markup=False)
    console.print("\n[bold]Raw response[/bold]")
    console.print(row["raw_response"], highlight=False, markup=False)


@app.command()
def export(
    channel: str = typer.Option(..., "--channel", "-c", help="Channel id; must be watched."),
    since: str = typer.Option(..., "--since", help="YYYY-MM-DD or an ISO timestamp."),
    until: str | None = typer.Option(None, "--until"),
    out: Path | None = typer.Option(None, "--out", help="Write here instead of stdout."),
) -> None:
    """Dump a watched channel's messages as JSONL, for fixture building."""
    lines = api().stream_lines("/api/messages", channel=channel, since=since, until=until)
    if out is None:
        for line in lines:
            print(line)
        return
    out.parent.mkdir(parents=True, exist_ok=True)
    count = 0
    with out.open("w", encoding="utf-8") as handle:
        for line in lines:
            handle.write(line + "\n")
            count += 1
    console.print(f"[green]✓[/green] {count} message(s) → {out}")


# --- bossctl fixed ----------------------------------------------------------


@fixed_app.command("list")
def fixed_list(user: str | None = typer.Option(None, "--user", help="Only this member's.")) -> None:
    """The weekly baseline timings."""
    rows = api().get("/api/fixed", user=user)
    print_table(
        f"{len(rows)} timing(s)",
        ["id", "when", "bosses", "party", "home channel", "owner"],
        [
            [
                row["short_id"],
                f"{row['weekday_name']} {row['time']}",
                " + ".join(row["bosses"]),
                ", ".join(p["name"] for p in row["participants"]),
                row["channel_name"] or row["channel_id"] or "—",
                row["owner_name"],
            ]
            for row in rows
        ],
    )


@fixed_app.command("add")
def fixed_add(
    bosses: str = typer.Option(..., "--bosses", "-b", help='e.g. "hstar, hfa".'),
    day: str = typer.Option(..., "--day", "-d", help="mon..sun."),
    time: str = typer.Option(..., "--time", "-t", help="HH:MM in the guild timezone."),
    channel: str = typer.Option(..., "--channel", "-c", help="Home channel id; must be watched."),
    member: list[str] = typer.Option(..., "--member", "-m", help="Discord user id (repeatable)."),
    owner: str | None = typer.Option(None, "--owner", help="Defaults to PORTAL_ACTOR_ID."),
    note: str | None = typer.Option(None, "--note"),
) -> None:
    """Add a weekly timing. It materialises into runs for this week and next."""
    row = api().post(
        "/api/fixed",
        {
            "bosses": bosses,
            "day": day,
            "time": time,
            "channel_id": channel,
            "participants": member,
            "owner_id": owner,
            "note": note,
        },
    )
    console.print(
        f"[green]✓[/green] {row['short_id']}: {' + '.join(row['bosses'])} · "
        f"{row['weekday_name']} {row['time']} · {row['channel_name'] or row['channel_id']}"
    )


@fixed_app.command("edit")
def fixed_edit(
    fixed_id: str = typer.Argument(help="Timing id, or any unique prefix."),
    bosses: str | None = typer.Option(None, "--bosses", "-b"),
    day: str | None = typer.Option(None, "--day", "-d"),
    time: str | None = typer.Option(None, "--time", "-t"),
    channel: str | None = typer.Option(None, "--channel", "-c"),
    member: list[str] = typer.Option(None, "--member", "-m", help="Replaces the whole party."),
    note: str | None = typer.Option(None, "--note"),
) -> None:
    """Change a weekly timing. Only the fields you pass are touched."""
    body: dict[str, Any] = {}
    for key, value in (
        ("bosses", bosses),
        ("day", day),
        ("time", time),
        ("channel_id", channel),
        ("note", note),
    ):
        if value is not None:
            body[key] = value
    if member:
        body["participants"] = list(member)
    if not body:
        fail("nothing to change - pass at least one of --bosses/--day/--time/--channel/--member")
    row = api().patch(f"/api/fixed/{fixed_id}", body)
    console.print(
        f"[green]✓[/green] {row['short_id']}: {' + '.join(row['bosses'])} · "
        f"{row['weekday_name']} {row['time']}"
    )


@fixed_app.command("rm")
def fixed_rm(fixed_id: str = typer.Argument(help="Timing id, or any unique prefix.")) -> None:
    """Remove a weekly timing and cancel the runs it already produced."""
    result = api().delete(f"/api/fixed/{fixed_id}")
    console.print(
        f"[green]✓[/green] Removed {result['short_id']} "
        f"({result['cancelled_runs']} upcoming run(s) cancelled)."
    )


# --- bossctl config ---------------------------------------------------------


@config_app.command("get")
def config_get(key: str | None = typer.Argument(None, help="One setting, or all of them.")) -> None:
    """Show the runtime settings (and the read-only deployment ones)."""
    values = api().get("/api/config")
    if key:
        if key not in values:
            fail(f"unknown setting `{key}` - one of {', '.join(values)}")
        console.print(json.dumps(values[key]) if not isinstance(values[key], str) else values[key])
        return
    print_table(
        "config",
        ["setting", "value"],
        [
            [name, ", ".join(value) if isinstance(value, list) else str(value)]
            for name, value in values.items()
        ],
    )


@config_app.command("set")
def config_set(
    key: str = typer.Argument(
        help="day_of_ping_time, countdown_minutes, paused or extract_enabled."
    ),
    value: str = typer.Argument(help="The new value."),
) -> None:
    """Change one runtime setting. Takes effect at once and survives a restart."""
    flags = {"paused", "extract_enabled"}
    if key in flags:
        parsed: Any = value.strip().lower() in ("1", "true", "yes", "on")
    else:
        parsed = value
    values = api().request("PUT", "/api/config", json={key: parsed})
    console.print(f"[green]✓[/green] {key} = {values.get(key)}")


# ---------------------------------------------------------------------------
# entry point
# ---------------------------------------------------------------------------


def main() -> int:
    """Run the CLI, turning an API refusal into a message and a non-zero exit."""
    try:
        app(standalone_mode=False)
    except ApiFailed as exc:
        err.print(f"[red]✗[/red] {exc}")
        return 1
    except typer.Exit as exc:
        return exc.exit_code
    except click_exceptions() as exc:  # pragma: no cover - argument errors
        exc.show()
        return getattr(exc, "exit_code", 2)
    except KeyboardInterrupt:  # pragma: no cover
        return 130
    return 0


def click_exceptions():
    import click

    return (click.ClickException, click.exceptions.Abort, click.exceptions.UsageError)


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
