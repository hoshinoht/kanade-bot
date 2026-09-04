"""Who is allowed to talk to the API.

Two paths, and a request needs exactly one of them (DESIGN.md §5,
"Access control"):

1. **Bearer token.**  ``Authorization: Bearer <ADMIN_TOKEN>``, compared in
   constant time.  This is what ``bossctl`` and ``curl`` use.
2. **Tailscale identity.**  A request that came through `tailscale serve` on the
   host carries ``Tailscale-User-Login``.  It is accepted only when
   ``TRUST_TAILSCALE_HEADERS=true`` *and* the login is in
   ``ALLOWED_TAILSCALE_LOGINS`` *and* the connection came from the host itself.

The header is a plain string, so trusting it is only safe because of where the
socket is: compose publishes the port as ``127.0.0.1:8080:8080``, so the only
thing that can reach it is a process on the host, and the only thing there
that sets the header is `tailscale serve`.  Anyone who can already open
``127.0.0.1:8080`` on the host can run arbitrary code as that user anyway.  The
flag defaults to off so a deployment without `tailscale serve` in front cannot
be walked into by sending the header by hand.

Browsers then carry a **signed session cookie** instead of the token, so the
portal does not need it on every navigation.  The cookie is an HMAC over the
identity, keyed with ``ADMIN_TOKEN``: rotating the token invalidates every
session, and a cookie is worthless to anyone who does not already have the token.
"""

from __future__ import annotations

import hmac
import ipaddress
import logging
from dataclasses import dataclass
from datetime import timedelta

from fastapi import Request
from itsdangerous import BadSignature, SignatureExpired, URLSafeTimedSerializer

from bot.infrastructure.config import Settings

from .errors import NotConfigured, Unauthorized

log = logging.getLogger(__name__)

#: Cookie name and lifetime for a browser session.
SESSION_COOKIE = "boss_portal"
SESSION_MAX_AGE = timedelta(days=7)
#: Namespaces the HMAC, so a signature can never be replayed as another payload.
SESSION_SALT = "boss-scheduler-portal-session"

HEADER_LOGIN = "Tailscale-User-Login"
HEADER_NAME = "Tailscale-User-Name"


@dataclass(frozen=True)
class Identity:
    """Who the caller is, and which of the three paths let them in."""

    who: str
    via: str  # "token" | "tailscale" | "cookie"

    @property
    def display(self) -> str:
        return self.who if self.via != "token" else "admin token"


def _serializer(secret: str) -> URLSafeTimedSerializer:
    return URLSafeTimedSerializer(secret, salt=SESSION_SALT)


def issue_session(secret: str, identity: Identity) -> str:
    """Sign ``identity`` into the cookie value."""
    return _serializer(secret).dumps({"who": identity.who, "via": identity.via})


def read_session(secret: str, raw: str | None) -> Identity | None:
    """Verify a cookie value; ``None`` if it is missing, forged or expired."""
    if not raw:
        return None
    try:
        data = _serializer(secret).loads(raw, max_age=int(SESSION_MAX_AGE.total_seconds()))
    except SignatureExpired:
        return None
    except BadSignature:
        log.warning("rejected a portal cookie with a bad signature")
        return None
    if not isinstance(data, dict) or not data.get("who"):
        return None
    return Identity(who=str(data["who"]), via="cookie")


def bearer_token(request: Request) -> str | None:
    """The token out of ``Authorization: Bearer <token>``, if that is the scheme."""
    header = request.headers.get("authorization", "")
    scheme, _, value = header.partition(" ")
    return value.strip() if scheme.lower() == "bearer" and value.strip() else None


def token_matches(supplied: str | None, expected: str) -> bool:
    """Constant-time comparison, so a wrong token leaks nothing by timing."""
    if not supplied or not expected:
        return False
    return hmac.compare_digest(supplied.encode("utf-8"), expected.encode("utf-8"))


def is_local_peer(host: str | None) -> bool:
    """True for loopback and private addresses -- i.e. "this machine or its bridge".

    Inside the container a request forwarded by the compose port mapping arrives
    from the Docker gateway (a private address), and when the bot is run
    natively it arrives from ``127.0.0.1``.  Anything globally routable did not
    come through the loopback publish and must not be trusted with a header.
    """
    if not host:
        return False
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        return False
    return address.is_loopback or address.is_private or address.is_link_local


def tailscale_identity(request: Request, settings: Settings) -> Identity | None:
    """The caller's tailnet login, if `tailscale serve` vouched for it."""
    if not settings.trust_tailscale_headers:
        return None
    login = (request.headers.get(HEADER_LOGIN) or "").strip().lower()
    if not login:
        return None
    peer = request.client.host if request.client else None
    if not is_local_peer(peer):
        log.warning("ignoring a %s header from a non-local peer %s", HEADER_LOGIN, peer)
        return None
    if login not in settings.allowed_login_list:
        log.warning("tailnet login %r is not in ALLOWED_TAILSCALE_LOGINS", login)
        return None
    return Identity(who=login, via="tailscale")


def authenticate(request: Request, settings: Settings) -> Identity:
    """Resolve the caller, or raise.

    Order matters only for the message a caller gets back: an explicit bearer
    token that is wrong should say so rather than falling through to "no
    credentials".
    """
    if not settings.admin_token:
        raise NotConfigured()

    supplied = bearer_token(request)
    if supplied is not None:
        if token_matches(supplied, settings.admin_token):
            return Identity(who="admin token", via="token")
        raise Unauthorized("that bearer token is not ADMIN_TOKEN")

    identity = tailscale_identity(request, settings)
    if identity is not None:
        return identity

    identity = read_session(settings.admin_token, request.cookies.get(SESSION_COOKIE))
    if identity is not None:
        return identity

    raise Unauthorized("send `Authorization: Bearer $ADMIN_TOKEN`, or sign in at /login")


__all__ = [
    "HEADER_LOGIN",
    "HEADER_NAME",
    "SESSION_COOKIE",
    "SESSION_MAX_AGE",
    "Identity",
    "authenticate",
    "bearer_token",
    "is_local_peer",
    "issue_session",
    "read_session",
    "tailscale_identity",
    "token_matches",
]
