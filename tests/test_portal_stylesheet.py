from bot.portal_styles import GENERATED_HEADER, build_stylesheet, partials


def test_the_stylesheet_is_built_from_every_ordered_partial():
    sources = partials()
    stylesheet = build_stylesheet()

    assert sources
    assert stylesheet.startswith(GENERATED_HEADER)
    assert stylesheet == GENERATED_HEADER + "".join(
        source.read_text(encoding="utf-8") for source in sources
    )
