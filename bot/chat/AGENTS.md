# Chatbot guide

## Boundaries

- `agent.py` owns the gated conversational loop, channel-scoped bounded memory, rate limits, per-channel busy state, and shared model lock.
- Keep mention/channel/role refusal logic pure in `gate.py`. Revalidate every model-selected tool and argument against trusted runtime context.
- Chatbot schedule writes are proposals only. They reuse the extractor card/approval path and become authoritative only after confirmation.
- `followup.py` handles rejection clarification for chatbot-created cards and must use read-only tools. `strategy.py` resolves checked-in boss references deterministically before generation.

## Prompts and personas

- `persona.py` assembles prompt components in a deliberate order; code-owned policy files live in `prompts/` and are validated at construction.
- `persona_catalog.py` validates complete bundles, aliases, safe filenames, staging configuration, and whole-bundle fallback behavior.
- Live persona/behavior files under `config/personas/` are deployment-private. Change tracked examples or the checked-in fallback unless private deployment state is explicitly in scope.

## Checks

- Focused suite: `uv run pytest -q tests/test_chat_agent.py tests/test_chat_gate.py tests/test_chat_tools.py tests/test_chat_persona.py tests/test_persona_catalog.py tests/test_chat_coordination.py tests/test_chat_followup.py tests/test_chat_injection.py tests/test_chat_authority.py`.
- Real-model smoke test: `uv run pytest -m ollama -k chat_live`.
