"""Synthetic persona scenarios; expected results are produced by the v4 oracle."""

from __future__ import annotations

from typing import Any

BUNDLE_ID = "kanade"

CLOCK = {
    "now": "2026-08-30T20:15:00+08:00",
    "timezone": "Asia/Kuala_Lumpur",
    "week_start": "2026-08-27T00:00:00+08:00",
}

VOICED = """# Synthetic reply profile

**Voice:** Brisk and upbeat, like a synthetic test announcer.

## Delivery

- Keep one upbeat aside per reply.
- Preserve identity boundaries and exact facts.

**Good**

> `Synthetic run is at 21:00. Card's up, go react!`
> `Nothing on tonight. Rest up!`

**Bad**

> `I moved it already.`"""

UNVOICED = """# Plain synthetic profile

Answer a little more formally for this member.

Keep every fact exact."""

PLACEHOLDER = """# Placeholder synthetic profile

**Voice:** <A short voice cue for this profile.>

**Good**

> `<A short reply in this profile.>`"""

BUDGET = """# Busy synthetic profile

_Voice_: Rapid-fire synthetic commentary

```text
**Good**
> `fenced example must be ignored`
```

**Goodbye**

> `goodbye heading must be ignored`

**Good replies**

> `First section one.`
> `First section two.`
> `First section three.`
> `First section four.`
> `First section five.`
> `First section six.`

## Next heading

> `after a heading this is not an example`

Good:

""" + "\n".join(
    f"> `Second section {n}, deliberately long enough that four of these, with the short "
    "lines, overflow the shared example character budget of the prompt.`"
    for n in ("one", "two", "three", "four")
)


COUNT = "**Good**\n\n" + "\n".join(f"> `Short synthetic line {n}.`" for n in range(1, 11))


def _case(
    case_id: str,
    profile: dict[str, Any] | None,
    model: str = "synthetic-model",
    focus_card: str = "",
) -> dict[str, Any]:
    return {
        "case_id": case_id,
        "input": {
            "bundle_id": BUNDLE_ID,
            "profile": profile,
            "clock": CLOCK,
            "model": model,
            "focus_card": focus_card,
        },
    }


def _profile(profile_id: str, markdown: str, staging: dict[str, str] | None) -> dict[str, Any]:
    return {"id": profile_id, "markdown": markdown, "staging": staging}


def cases() -> list[dict[str, Any]]:
    return [
        _case("bundle-default-unnamed-model", None, model=""),
        _case("bundle-default-with-focus", None, focus_card="`abc123` Synthetic Boss Sunday 21:00"),
        _case(
            "profile-voice-examples-partial-staging",
            _profile(
                "synthetic-voiced",
                VOICED,
                {"generic": "Synthetic thinking…", "guide_named": "{boss}: synthetic notes"},
            ),
        ),
        _case("profile-without-voice-or-examples", _profile("synthetic-plain", UNVOICED, None)),
        _case(
            "profile-placeholder-voice-and-example",
            _profile("synthetic-placeholder", PLACEHOLDER, {}),
        ),
        _case(
            "profile-example-budget-and-section-rules",
            _profile(
                "synthetic-busy",
                BUDGET,
                {
                    "schedule": "Synthetic schedule",
                    "guide": "Synthetic guide",
                    "guide_named": "Synthetic {boss}",
                    "write": "Synthetic write",
                    "generic": "Synthetic generic",
                },
            ),
        ),
        _case("profile-example-count-limit", _profile("synthetic-count", COUNT, None)),
    ]
