# Boss data guide

- `bosses.yaml` is the canonical catalog for boss keys, display names, difficulties, and aliases. Runtime and tests load it through `bot/domain/bosses.py`.
- `knowledge/_meta.yaml` defines shared sources. When chatbot channels are configured, `knowledge/` must also contain exactly one lowercase YAML file per catalog boss and no extras.
- Keep strategy facts source-backed; validation/rendering belongs to `bot/domain/boss_knowledge.py`.
- Catalog or knowledge changes require a bot restart. Portrait and entry-art changes require only a portal page reload.
- `portraits/` and `artwork/entry/` are deployment assets and intentionally git-ignored apart from their READMEs; follow boss-key naming documented there.
- Validate data changes with `uv run pytest -q tests/test_bosses.py tests/test_boss_knowledge.py`; image behavior is covered by portal asset tests without depending on local images.
