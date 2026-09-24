"""Boss catalog provenance is available through the authenticated portal."""

from __future__ import annotations

from bot.domain.boss_knowledge import BossKnowledgeBase

from .conftest import REPO_ROOT


def test_boss_catalog_links_to_knowledge_and_detail_preserves_urls(auth, fake_bot):
    fake_bot.boss_knowledge = BossKnowledgeBase.load(
        REPO_ROOT / "boss" / "knowledge", fake_bot.bosses
    )

    catalog = auth.get("/bosses")
    detail = auth.get("/bosses/hstar/knowledge")

    assert catalog.status_code == 200
    assert "/bosses/" in catalog.text and "/knowledge" in catalog.text
    assert detail.status_code == 200
    assert "Radiant Malefic Star" in detail.text
    assert "Repository provenance" in detail.text
    assert 'href="https://' in detail.text


def test_boss_knowledge_page_requires_auth(client):
    response = client.get("/bosses/hstar/knowledge", follow_redirects=False)
    assert response.status_code == 303
    assert response.headers["location"].startswith("/login")


def test_boss_knowledge_page_handles_missing_guides(auth):
    assert auth.get("/bosses/hstar/knowledge").status_code == 404
