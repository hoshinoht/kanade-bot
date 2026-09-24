"""Authenticated HTTP client for the Kanata OpenAI-compatible gateway."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import TYPE_CHECKING, Any

import httpx

from .capabilities import (
    CAPABILITY_TIMEOUT_S,
    FIELD_CAPABILITY,
    Capabilities,
    CapabilityCache,
    parse_models,
)
from .errors import (
    ModelAuthError,
    ModelConfigError,
    ModelFieldRejected,
    ModelResponseError,
    ModelTimeout,
    ModelUnavailable,
)

if TYPE_CHECKING:
    from bot.infrastructure.config import Settings

log = logging.getLogger(__name__)

CHAT_PATH = "/v1/chat/completions"
MODELS_PATH = "/v1/models"


#: Printable ASCII without spaces: anything else cannot travel in a bearer header.
_KEY_CHARS = frozenset(chr(code) for code in range(0x21, 0x7F))


def _read_key(path: str) -> str:
    # Messages name the setting only: a mistakenly pasted key must not be echoed.
    if not path:
        raise ModelConfigError("KANATA_API_KEY_FILE is not set")
    try:
        key = Path(path).read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        raise ModelConfigError("KANATA_API_KEY_FILE points to a missing file") from None
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        raise ModelConfigError(
            f"KANATA_API_KEY_FILE is unreadable ({type(exc).__name__})"
        ) from None
    if not key:
        raise ModelConfigError("KANATA_API_KEY_FILE holds an empty key")
    if not set(key) <= _KEY_CHARS:
        raise ModelConfigError(
            "KANATA_API_KEY_FILE holds a key with spaces, control or non-ASCII characters"
        )
    return key


def key_file_error(path: str) -> str | None:
    """Why the key file is unusable, or ``None``. Never returns key material."""
    try:
        _read_key(path)
    except ModelConfigError as exc:
        return str(exc)
    return None


def _status_error(status: int, path: str) -> ModelUnavailable:
    if status in (401, 403):
        return ModelAuthError(
            f"model gateway rejected the API key (HTTP {status}); check KANATA_API_KEY_FILE"
        )
    if status == 404:
        hint = "check KANATA_BASE_URL" + (" and the model alias" if path == CHAT_PATH else "")
        return ModelUnavailable(f"model gateway returned HTTP 404 for {path}; {hint}")
    if status == 429:
        return ModelUnavailable("model gateway is rate limiting (HTTP 429)")
    if status >= 500:
        return ModelUnavailable(f"model gateway failed (HTTP {status})")
    return ModelUnavailable(f"model gateway refused the request (HTTP {status})")


def _rejected_param(response: httpx.Response) -> str | None:
    """``error.param`` of a 400 body when it names a known optional field."""
    try:
        error = response.json().get("error")
    except (ValueError, AttributeError):
        return None
    param = error.get("param") if isinstance(error, dict) else None
    return param if param in FIELD_CAPABILITY else None


class KanataClient:
    """One httpx client with the bearer key; TLS verification is always on."""

    def __init__(
        self,
        base_url: str,
        key_file: str,
        timeout: float,
        *,
        transport: httpx.AsyncBaseTransport | None = None,
    ):
        # The settings validator trims this too; a directly assigned URL must not double it.
        self.base_url = base_url.rstrip("/").removesuffix("/v1").rstrip("/")
        self._capabilities = CapabilityCache(self.model_entries)
        self._timeout = timeout
        self._http = httpx.AsyncClient(
            base_url=self.base_url,
            timeout=timeout,
            headers={"Authorization": f"Bearer {_read_key(key_file)}"},
            transport=transport,
            verify=True,
            follow_redirects=False,
        )

    @classmethod
    def from_settings(cls, settings: Settings, timeout: float, **kwargs: Any) -> KanataClient:
        return cls(settings.kanata_base_url, settings.kanata_api_key_file, timeout, **kwargs)

    def __repr__(self) -> str:
        return f"KanataClient(base_url={self.base_url!r})"

    async def _request(
        self, method: str, path: str, body: dict | None = None, timeout: float | None = None
    ) -> Any:
        extra = {"timeout": timeout} if timeout is not None else {}
        try:
            response = await self._http.request(method, path, json=body, **extra)
        except httpx.TimeoutException:
            raise ModelTimeout("model gateway timed out") from None
        except httpx.HTTPError as exc:
            # Includes TLS verification and connection failures.
            raise ModelUnavailable(f"model gateway unreachable ({type(exc).__name__})") from None
        if response.status_code == 400:
            param = _rejected_param(response)
            if param is not None:
                raise ModelFieldRejected(
                    f"model gateway refused the request (HTTP 400, field `{param}`)", param
                )
        if response.status_code != 200:
            raise _status_error(response.status_code, path)
        try:
            return response.json()
        except ValueError:
            raise ModelResponseError("model gateway returned non-JSON") from None

    async def chat(self, **body: Any) -> dict:
        """POST one non-streaming chat completion and return its JSON object.

        A 400 naming an optional field we sent revokes that capability for the
        alias for one cache TTL and retries once without the affected fields.
        """
        try:
            data = await self._request("POST", CHAT_PATH, body)
        except ModelFieldRejected as exc:
            alias = body.get("model")
            capability = FIELD_CAPABILITY.get(exc.param)
            if capability is None or exc.param not in body or not isinstance(alias, str):
                raise
            self._capabilities.revoke(alias, capability)
            dropped = [f for f, c in FIELD_CAPABILITY.items() if c == capability and f in body]
            log.warning(
                "model gateway rejected `%s` for %s; retrying once without %s",
                exc.param,
                alias,
                ", ".join(dropped),
            )
            retry = {key: value for key, value in body.items() if key not in dropped}
            data = await self._request("POST", CHAT_PATH, retry)
        if not isinstance(data, dict):
            raise ModelResponseError("model gateway returned a non-object completion")
        return data

    async def models(self) -> list[str]:
        """Aliases listed by ``GET /v1/models``."""
        return list(await self.model_entries())

    async def model_entries(self) -> dict[str, Capabilities]:
        """Listed aliases with their declared capabilities, in gateway order."""
        data = await self._request(
            "GET", MODELS_PATH, timeout=min(CAPABILITY_TIMEOUT_S, self._timeout)
        )
        entries = parse_models(data)
        if entries is None:
            raise ModelResponseError("model gateway returned an unexpected model list")
        return entries

    async def profile(self, alias: str) -> Capabilities:
        """Cached capabilities for one alias; never raises."""
        return await self._capabilities.lookup(alias)

    async def close(self) -> None:
        await self._http.aclose()
