from bot.portal_styles import GENERATED_HEADER, build_stylesheet, partials


def test_the_stylesheet_is_built_from_every_ordered_partial():
    sources = partials()
    stylesheet = build_stylesheet()

    assert sources
    assert stylesheet.startswith(GENERATED_HEADER)
    assert stylesheet == GENERATED_HEADER + "".join(
        source.read_text(encoding="utf-8") for source in sources
    )


def test_memory_subject_tabs_have_all_targets_and_one_scroll_owner_per_viewport():
    stylesheet = build_stylesheet()

    for panel in (
        "memory-enrollment",
        "memory-preference",
        "memory-records",
        "memory-activity",
    ):
        assert f'.tabs:has(#{panel}:target) [href="#{panel}"]' in stylesheet
    assert "body.memory-subject-frame .memory-tabs {\n  flex: 1 1 auto;" in stylesheet
    assert "  display: flex;\n  flex-direction: column;\n  overflow: hidden;" in stylesheet
    assert "body.memory-subject-frame .memory-tabs .tabs__panel {\n  flex: 1 1 auto;" in stylesheet
    assert "  min-height: 0;\n  overflow-y: auto;" in stylesheet
    assert "body.memory-subject-frame .memory-tabs .tabs__panel {\n    display: none;" in stylesheet
