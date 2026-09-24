from __future__ import annotations

import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

try:
    from .discovery import discover_configured_healthchecks
    from .validation import (
        validate_entrypoint_inventory,
        validate_env_inventory,
        validate_healthcheck_inventory,
    )
except ImportError:  # pragma: no cover - unittest discovery loads this as top-level
    from discovery import discover_configured_healthchecks
    from validation import (
        validate_entrypoint_inventory,
        validate_env_inventory,
        validate_healthcheck_inventory,
    )


class InventoryValidationTests(unittest.TestCase):
    @staticmethod
    def _healthcheck_item() -> list[dict[str, object]]:
        return [
            {
                "id": "python -m bot.health",
                "disposition": "Retain",
                "owner": "operations/health",
                "source": "bot/health.py#main",
                "tests": ["scripts/v5_inventory/test_validation.py"],
                "configured_sources": {
                    "deploy/Dockerfile#HEALTHCHECK": "python -m bot.health",
                    "deploy/compose.yaml#services.bot.healthcheck": "python -m bot.health",
                },
            }
        ]

    @staticmethod
    def _healthcheck_root(
        directory: Path, docker_command: str | None, compose_command: str | None
    ) -> None:
        (directory / "deploy").mkdir(exist_ok=True)
        docker = "FROM python:3.12-slim\n"
        if docker_command is not None:
            docker += f'HEALTHCHECK CMD ["python", "-m", "{docker_command}"]\n'
        else:
            docker += "# python -m bot.health is only a comment, not a healthcheck\n"
        (directory / "deploy" / "Dockerfile").write_text(docker, encoding="utf-8")
        compose = "services:\n  bot:\n"
        if compose_command is not None:
            compose += (
                f'    healthcheck:\n      test: ["CMD", "python", "-m", "{compose_command}"]\n'
            )
        else:
            compose += "    # python -m bot.health is only a comment, not a healthcheck\n"
        (directory / "deploy" / "compose.yaml").write_text(compose, encoding="utf-8")

    def _assert_healthcheck_source_change(
        self, docker_command: str | None, compose_command: str | None, source: str
    ) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            self._healthcheck_root(root, docker_command, compose_command)
            discovered = discover_configured_healthchecks(root)
            errors = validate_healthcheck_inventory(
                self._healthcheck_item(), {"python -m bot.health"}, discovered
            )
        self.assertTrue(any(source in error for error in errors), errors)

    def test_omitted_entrypoint_fails(self) -> None:
        errors = validate_entrypoint_inventory(
            [
                {
                    "id": "python -m bot",
                    "disposition": "Retain",
                    "owner": "runtime",
                    "source": "bot/__main__.py#main",
                    "tests": ["tests/test_fatal_exit.py"],
                }
            ],
            {"python -m bot", "python -m bot.health"},
        )
        self.assertTrue(any("entrypoint set mismatch" in error for error in errors))

    def test_missing_env_disposition_fails(self) -> None:
        errors = validate_env_inventory(
            ["BOSSCTL_URL"],
            [{"key": "BOSSCTL_URL", "policy": "cli"}],
            {"cli": {"disposition": "Defer", "owner": "api-cli", "tests": ["tests/test_cli.py"]}},
        )
        self.assertTrue(any("missing/invalid env disposition" in error for error in errors))

    def test_missing_dockerfile_healthcheck_fails(self) -> None:
        self._assert_healthcheck_source_change(None, "bot.health", "deploy/Dockerfile#HEALTHCHECK")

    def test_changed_dockerfile_healthcheck_fails(self) -> None:
        self._assert_healthcheck_source_change("bot.other", "bot.health", "deploy/Dockerfile#HEALTHCHECK")

    def test_missing_compose_healthcheck_fails(self) -> None:
        self._assert_healthcheck_source_change(
            "bot.health", None, "deploy/compose.yaml#services.bot.healthcheck"
        )

    def test_changed_compose_healthcheck_fails(self) -> None:
        self._assert_healthcheck_source_change(
            "bot.health", "bot.other", "deploy/compose.yaml#services.bot.healthcheck"
        )

    def test_echo_and_shell_forms_fail(self) -> None:
        cases = (
            ("deploy/Dockerfile", 'HEALTHCHECK CMD ["echo", "python -m bot.health"]\n'),
            ("deploy/Dockerfile", "HEALTHCHECK CMD echo python -m bot.health\n"),
            ("deploy/Dockerfile", "HEALTHCHECK CMD python -m bot.health\n"),
            (
                "deploy/compose.yaml",
                "services:\n  bot:\n    healthcheck:\n"
                '      test: ["CMD", "echo", "python -m bot.health"]\n',
            ),
            (
                "deploy/compose.yaml",
                "services:\n  bot:\n    healthcheck:\n"
                '      test: ["CMD-SHELL", "echo python -m bot.health"]\n',
            ),
        )
        for filename, content in cases:
            with self.subTest(content=content), TemporaryDirectory() as temporary:
                root = Path(temporary)
                self._healthcheck_root(root, "bot.health", "bot.health")
                (root / filename).write_text(content, encoding="utf-8")
                errors = validate_healthcheck_inventory(
                    self._healthcheck_item(),
                    {"python -m bot.health"},
                    discover_configured_healthchecks(root),
                )
                self.assertTrue(any(filename in error for error in errors), errors)


if __name__ == "__main__":
    unittest.main()
