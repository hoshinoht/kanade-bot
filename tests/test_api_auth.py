"""Who gets in, and who doesn't (DESIGN.md §5, "Access control")."""

from __future__ import annotations

import pytest

from bot.api import create_app
from bot.api.auth import (
    HEADER_LOGIN,
    SESSION_COOKIE,
    Identity,
    is_local_peer,
    issue_session,
    read_session,
    token_matches,
)

from .fake_bot import ADMIN_TOKEN, FakeBot, make_settings


def build(fake_bot, **settings):
    fake_bot.settings = make_settings(**settings)
    return create_app(fake_bot)


def client_for(app, peer: str = "127.0.0.1"):
    """A client whose connection appears to come from ``peer``.

    The default TestClient peer is the literal string "testclient", which is not
    an address at all -- and is correctly refused, which
    :func:`test_a_header_from_a_peer_that_is_not_this_machine_is_ignored` relies on.
    """
    from fastapi.testclient import TestClient

    return TestClient(app, client=(peer, 51234))


# --- the bearer token -------------------------------------------------------


def test_healthz_needs_nothing(client):
    response = client.get("/healthz")
    assert response.status_code == 200
    assert response.text.strip() == "ok"


def test_bearer_token_is_accepted(auth):
    assert auth.get("/api/schedule").status_code == 200


def test_missing_credentials_is_401_json_on_the_api(client):
    response = client.get("/api/schedule")
    assert response.status_code == 401
    assert "ADMIN_TOKEN" in response.json()["error"]


def test_wrong_bearer_token_says_so(client):
    response = client.get("/api/schedule", headers={"Authorization": "Bearer nope"})
    assert response.status_code == 401
    assert "not ADMIN_TOKEN" in response.json()["error"]


def test_a_browser_is_redirected_to_login_rather_than_shown_json(client):
    response = client.get("/", follow_redirects=False)
    assert response.status_code == 303
    assert response.headers["location"] == "/login?next=/"


def test_token_comparison_is_exact():
    assert token_matches(ADMIN_TOKEN, ADMIN_TOKEN)
    assert not token_matches(ADMIN_TOKEN + " ", ADMIN_TOKEN)
    assert not token_matches("", ADMIN_TOKEN)
    assert not token_matches(ADMIN_TOKEN, "")


# --- no ADMIN_TOKEN at all --------------------------------------------------


def test_without_an_admin_token_everything_but_health_is_503(fake_bot):
    app = build(fake_bot, admin_token="")
    with client_for(app) as client:
        assert client.get("/healthz").status_code == 200
        response = client.get("/api/schedule")
        assert response.status_code == 503
        assert "set ADMIN_TOKEN" in response.json()["error"]
        # Even the sign-in page: there is nothing to sign in against.
        assert client.get("/login").status_code == 503


def test_an_admin_token_that_is_only_a_comment_reads_as_unset(monkeypatch):
    """`ADMIN_TOKEN=   # generate with ...` must not become the token itself."""
    from bot.config import Settings

    monkeypatch.setenv("ADMIN_TOKEN", "#generate with: openssl rand -hex 32")
    settings = Settings(
        _env_file=None,
        discord_token="t",
        guild_id=1,
        bossing_role_id=2,
        chat_channel_ids="3",
    )
    assert settings.admin_token == ""


# --- the tailscale identity header -----------------------------------------


def test_tailscale_header_is_ignored_unless_the_flag_is_on(fake_bot):
    app = build(fake_bot, trust_tailscale_headers=False, allowed_tailscale_logins="me@example.com")
    with client_for(app) as client:
        response = client.get("/api/schedule", headers={HEADER_LOGIN: "me@example.com"})
        assert response.status_code == 401


def test_tailscale_header_is_accepted_when_trusted_and_allowed(fake_bot):
    app = build(fake_bot, trust_tailscale_headers=True, allowed_tailscale_logins="me@example.com")
    with client_for(app) as client:
        response = client.get("/api/schedule", headers={HEADER_LOGIN: "me@example.com"})
        assert response.status_code == 200


def test_a_header_from_a_peer_that_is_not_this_machine_is_ignored(fake_bot):
    """Only the host (or the Docker bridge) can vouch; a tailnet peer cannot."""
    app = build(fake_bot, trust_tailscale_headers=True, allowed_tailscale_logins="me@example.com")
    with client_for(app, peer="100.64.1.2") as client:
        response = client.get("/api/schedule", headers={HEADER_LOGIN: "me@example.com"})
        assert response.status_code == 401


def test_tailscale_login_must_be_on_the_allow_list(fake_bot):
    app = build(fake_bot, trust_tailscale_headers=True, allowed_tailscale_logins="me@example.com")
    with client_for(app) as client:
        response = client.get("/api/schedule", headers={HEADER_LOGIN: "someone@else.com"})
        assert response.status_code == 401


def test_tailscale_login_matching_ignores_case(fake_bot):
    app = build(fake_bot, trust_tailscale_headers=True, allowed_tailscale_logins="Me@Example.com")
    with client_for(app) as client:
        response = client.get("/api/schedule", headers={HEADER_LOGIN: "me@EXAMPLE.com"})
        assert response.status_code == 200


@pytest.mark.parametrize(
    "host,expected",
    [
        ("127.0.0.1", True),
        ("::1", True),
        ("172.17.0.1", True),  # the Docker bridge gateway
        ("192.168.65.1", True),  # Docker Desktop's gateway
        ("10.0.0.4", True),
        ("100.64.1.2", False),  # a tailnet address is not "this machine"
        ("8.8.8.8", False),
        ("", False),
        ("nonsense", False),
    ],
)
def test_only_this_machine_may_assert_an_identity(host, expected):
    assert is_local_peer(host) is expected


# --- the browser session cookie --------------------------------------------


def test_a_session_round_trips():
    identity = Identity(who="admin token", via="cookie")
    raw = issue_session(ADMIN_TOKEN, identity)
    assert read_session(ADMIN_TOKEN, raw) == identity


def test_a_session_signed_with_another_token_is_refused():
    raw = issue_session("a-different-token", Identity(who="x", via="cookie"))
    assert read_session(ADMIN_TOKEN, raw) is None


def test_a_missing_or_junk_cookie_is_just_no_session():
    assert read_session(ADMIN_TOKEN, None) is None
    assert read_session(ADMIN_TOKEN, "") is None
    assert read_session(ADMIN_TOKEN, "not-a-signature") is None


def test_signing_in_sets_a_session_that_then_works(client):
    response = client.post(
        "/login", data={"token": ADMIN_TOKEN, "next": "/fixed"}, follow_redirects=False
    )
    assert response.status_code == 303
    assert response.headers["location"] == "/fixed"
    cookie = client.cookies.get(SESSION_COOKIE)
    assert cookie
    assert client.get("/").status_code == 200


def test_the_session_cookie_is_httponly_and_samesite_strict(client):
    """SameSite=Strict is what stops another site POSTing to 127.0.0.1:8080."""
    response = client.post("/login", data={"token": ADMIN_TOKEN}, follow_redirects=False)
    header = response.headers["set-cookie"]
    assert "HttpOnly" in header
    assert "samesite=strict" in header.lower()


def test_a_wrong_token_on_the_login_form_re_renders_with_an_error(client):
    response = client.post("/login", data={"token": "nope"})
    assert response.status_code == 401
    assert "isn&#39;t ADMIN_TOKEN" in response.text or "isn't ADMIN_TOKEN" in response.text
    assert SESSION_COOKIE not in client.cookies


def test_signing_out_drops_the_session(client):
    client.post("/login", data={"token": ADMIN_TOKEN}, follow_redirects=False)
    client.get("/logout", follow_redirects=False)
    assert client.get("/", follow_redirects=False).status_code == 303


def test_rotating_the_token_invalidates_existing_sessions(fake_bot, client):
    client.post("/login", data={"token": ADMIN_TOKEN}, follow_redirects=False)
    assert client.get("/").status_code == 200
    fake_bot.settings = make_settings(admin_token="a-brand-new-token")
    assert client.get("/", follow_redirects=False).status_code == 303


def test_login_page_renders_when_signed_out(client):
    response = client.get("/login")
    assert response.status_code == 200
    assert "Admin token" in response.text


def test_login_page_bounces_on_when_already_signed_in(auth):
    response = auth.get("/login", follow_redirects=False)
    assert response.status_code == 303


def test_fake_bot_is_the_only_thing_the_api_needs(fake_bot):
    """A guard: the API must keep duck-typing the client, not import it."""
    assert isinstance(fake_bot, FakeBot)
    assert create_app(fake_bot) is not None
