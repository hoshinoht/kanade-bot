"""The API really does serve, on the loop the bot is already using.

DESIGN.md §5 puts the API *inside* the bot process. That is easy to get subtly
wrong -- ``uvicorn.run`` would try to own the loop -- so this starts the real
server on a real port next to a running task, checks the unauthenticated health
endpoint and an authenticated one, and shuts it down.
"""

from __future__ import annotations

import asyncio
import socket

import httpx

from bot.api.server import ApiServer

from .fake_bot import ADMIN_TOKEN, make_settings


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


async def _wait_until_up(url: str, tries: int = 100) -> httpx.Response:
    async with httpx.AsyncClient(timeout=2.0) as client:
        for _ in range(tries):
            try:
                return await client.get(url)
            except httpx.HTTPError:
                await asyncio.sleep(0.02)
    raise AssertionError(f"{url} never came up")


def test_the_server_starts_serves_and_stops_on_the_bots_own_loop(fake_bot):
    port = free_port()
    fake_bot.settings = make_settings(api_port=port, api_host="127.0.0.1")
    server = ApiServer(fake_bot)
    base = f"http://127.0.0.1:{port}"

    async def scenario():
        # Something else running on the same loop, exactly like the tick loop.
        beats = 0

        async def heartbeat():
            nonlocal beats
            while True:
                beats += 1
                await asyncio.sleep(0.01)

        ticker = asyncio.create_task(heartbeat())
        await server.start()
        assert server.running
        try:
            health = await _wait_until_up(f"{base}/healthz")
            assert health.status_code == 200
            assert health.text.strip() == "ok"

            async with httpx.AsyncClient(timeout=2.0) as client:
                unauthorised = await client.get(f"{base}/api/schedule")
                assert unauthorised.status_code == 401

                ok = await client.get(
                    f"{base}/api/schedule",
                    headers={"Authorization": f"Bearer {ADMIN_TOKEN}"},
                )
                assert ok.status_code == 200
                assert ok.json()["timezone"] == "Asia/Kuala_Lumpur"
        finally:
            await server.stop()
            ticker.cancel()
        assert not server.running
        assert beats > 1  # the loop kept running while the API served

    asyncio.run(scenario())


def test_stopping_a_server_that_never_started_is_fine(fake_bot):
    asyncio.run(ApiServer(fake_bot).stop())


def test_it_still_serves_health_without_an_admin_token(fake_bot):
    """A half-configured deployment must still look healthy to compose."""
    port = free_port()
    fake_bot.settings = make_settings(api_port=port, admin_token="")
    server = ApiServer(fake_bot)

    async def scenario():
        await server.start()
        try:
            health = await _wait_until_up(f"http://127.0.0.1:{port}/healthz")
            assert health.status_code == 200
            async with httpx.AsyncClient(timeout=2.0) as client:
                refused = await client.get(f"http://127.0.0.1:{port}/api/schedule")
            assert refused.status_code == 503
            assert "set ADMIN_TOKEN" in refused.json()["error"]
        finally:
            await server.stop()

    asyncio.run(scenario())


def test_the_bind_address_is_configurable_and_defaults_to_loopback():
    """Compose sets ``API_HOST=0.0.0.0``; a native run must not."""
    assert make_settings().api_host == "127.0.0.1"
    assert make_settings(api_host="0.0.0.0").api_host == "0.0.0.0"  # noqa: S104


def test_the_server_does_not_trust_forwarded_headers(fake_bot):
    """We decide who is local ourselves; a header must not be able to say so."""
    assert ApiServer(fake_bot)._config().proxy_headers is False
