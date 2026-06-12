"""Unit tests for the LLM response parser in yoink_inbox.services.classify.

We do not exercise the full `run_classify` here (it needs a DB session and an
insight LLM route). Coverage focuses on the parser's tolerance to messy LLM
output, which is the most likely source of regressions.
"""
from __future__ import annotations

from yoink_inbox.services.classify import _parse_response


class TestParseResponse:
    def test_strict_json(self) -> None:
        raw = (
            '{"categories": [{"name": "Editors", "kind": "existing", '
            '"confidence": 0.9}], "summary": "neovim config", '
            '"is_github_repo": true}'
        )
        out = _parse_response(raw)
        assert len(out.categories) == 1
        assert out.categories[0].name == "Editors"
        assert out.categories[0].kind == "existing"
        assert out.categories[0].confidence == 0.9
        assert out.summary == "neovim config"
        assert out.is_github_repo is True

    def test_json_inside_prose(self) -> None:
        raw = (
            "Sure, here is the classification:\n"
            '```json\n{"categories": [{"name": "Postgres", "kind": "new"}], '
            '"summary": "uuid v7", "is_github_repo": false}\n```\n'
            "Let me know if you need anything else."
        )
        out = _parse_response(raw)
        assert len(out.categories) == 1
        assert out.categories[0].name == "Postgres"
        assert out.categories[0].kind == "new"
        assert out.summary == "uuid v7"
        assert out.is_github_repo is False

    def test_garbage_returns_empty(self) -> None:
        out = _parse_response("not json at all, just words")
        assert out.categories == []
        assert out.summary is None
        assert out.is_github_repo is None

    def test_empty_categories_array_is_valid(self) -> None:
        # \"nothing fits\" is a legal response.
        raw = '{"categories": [], "summary": "", "is_github_repo": false}'
        out = _parse_response(raw)
        assert out.categories == []
        assert out.summary is None  # empty string normalises to None
        assert out.is_github_repo is False

    def test_bogus_kind_falls_back_to_existing(self) -> None:
        raw = '{"categories": [{"name": "X", "kind": "weird"}]}'
        out = _parse_response(raw)
        assert out.categories[0].kind == "existing"

    def test_non_numeric_confidence_drops_to_none(self) -> None:
        raw = '{"categories": [{"name": "X", "kind": "existing", "confidence": "high"}]}'
        out = _parse_response(raw)
        assert out.categories[0].confidence is None

    def test_caps_at_three_categories(self) -> None:
        raw = '{"categories": [' + ",".join(
            f'{{"name": "C{i}", "kind": "existing"}}' for i in range(8)
        ) + "]}"
        out = _parse_response(raw)
        assert len(out.categories) == 3

    def test_drops_blank_names(self) -> None:
        raw = '{"categories": [{"name": "", "kind": "existing"}, {"name": "Real"}]}'
        out = _parse_response(raw)
        assert len(out.categories) == 1
        assert out.categories[0].name == "Real"
