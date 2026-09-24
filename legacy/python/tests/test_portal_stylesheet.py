from bot.portal_styles import GENERATED_HEADER, build_stylesheet, partials


def test_the_stylesheet_is_built_from_every_ordered_partial():
    sources = partials()
    stylesheet = build_stylesheet()

    assert sources
    assert stylesheet.startswith(GENERATED_HEADER)
    assert stylesheet == GENERATED_HEADER + "".join(
        source.read_text(encoding="utf-8") for source in sources
    )


def test_boss_knowledge_styles_remain_after_memory_page_removal():
    stylesheet = build_stylesheet()

    assert ".knowledge-sources {" in stylesheet
    assert ".knowledge-meta dl {" in stylesheet
    assert "memory-" not in stylesheet
    assert all("memory" not in source.name for source in partials())
