# Test suite guide

## Layout and fakes

- Tests are flat and grouped by feature prefixes: `test_extract_*`, `test_chat_*`, `test_api_*`, and `test_portal_*`; other files cover domain, agent, storage, CLI, and deployment behavior.
- `conftest.py` supplies in-memory repositories, portrait-free catalog copies, model-lock isolation, API clients/auth, and seeded schedules.
- Use `fake_bot.py` to record Discord side effects without a gateway and `chat_support.py` for scripted model responses and synthetic guild objects.
- Keep chatbot schedules and all imported clock seams on the same fixture-scoped aware instant. Many async chatbot tests use `pytest.mark.anyio`.

## Fixtures and commands

- `fixtures/extract/*.json` contains anonymized message fixtures. `fixture_loader.py` strictly rejects missing or invented amendments; never copy private exports or PII into fixtures.
- Default `uv run pytest` excludes the `ollama` marker. Mark only real local-model coverage with `pytest.mark.ollama`; those tests should skip when Ollama is unavailable.
- Use `uv run pytest -q tests/test_<area>.py::test_<case>` while iterating and `uv run pytest -q -m "not ollama"` for the CI-equivalent test pass.
- Schema changes require aligned creation/upgrade assertions in `test_migration.py`. Asset tests must remain isolated from git-ignored deployment images.
