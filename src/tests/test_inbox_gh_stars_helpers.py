"""Pure-helper tests for yoink_inbox.services.gh_stars.

We do not stand up a fake GitHub here; that needs an httpx MockTransport
and a real DB, both heavy for unit-test scope. Coverage focuses on the
header/payload parsers, which is where past bugs in similar sync code
have lived.
"""
from __future__ import annotations

from datetime import UTC, datetime

import pytest
from yoink_inbox.services.gh_stars import (
    _parse_next_link,
    _parse_starred_at,
)


class TestParseNextLink:
    def test_extracts_next_url(self) -> None:
        h = (
            '<https://api.github.com/user/starred?page=2>; rel="next", '
            '<https://api.github.com/user/starred?page=10>; rel="last"'
        )
        assert _parse_next_link(h) == "https://api.github.com/user/starred?page=2"

    def test_no_next_returns_none(self) -> None:
        h = '<https://api.github.com/user/starred?page=1>; rel="first"'
        assert _parse_next_link(h) is None

    def test_empty_or_none(self) -> None:
        assert _parse_next_link("") is None
        assert _parse_next_link(None) is None

    def test_multi_link_with_prev_and_next(self) -> None:
        # GitHub on a middle page returns prev, next, first, last.
        h = (
            '<https://api.github.com/user/starred?page=2>; rel="prev", '
            '<https://api.github.com/user/starred?page=4>; rel="next", '
            '<https://api.github.com/user/starred?page=1>; rel="first", '
            '<https://api.github.com/user/starred?page=10>; rel="last"'
        )
        assert _parse_next_link(h) == "https://api.github.com/user/starred?page=4"


class TestParseStarredAt:
    def test_z_suffix_iso(self) -> None:
        out = _parse_starred_at("2025-01-02T03:04:05Z")
        assert out == datetime(2025, 1, 2, 3, 4, 5, tzinfo=UTC)

    def test_already_offset(self) -> None:
        out = _parse_starred_at("2025-01-02T03:04:05+00:00")
        assert out == datetime(2025, 1, 2, 3, 4, 5, tzinfo=UTC)

    @pytest.mark.parametrize("garbage", [None, "", "not a date", 12345, {}])
    def test_invalid_returns_none(self, garbage: object) -> None:
        assert _parse_starred_at(garbage) is None
