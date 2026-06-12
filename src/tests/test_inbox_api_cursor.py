"""Tests for the cursor encoder in yoink_inbox.api.router.

The cursor is base64url(created_at_iso|id). We do NOT want it
self-describing on the wire, but we DO want roundtrip safety so paging
through a feed never silently skips a row.
"""
from __future__ import annotations

from datetime import UTC, datetime

import pytest
from fastapi import HTTPException
from yoink_inbox.api.router import _decode_cursor, _encode_cursor


class TestCursorRoundtrip:
    def test_basic_roundtrip(self) -> None:
        dt = datetime(2026, 1, 2, 3, 4, 5, 6, tzinfo=UTC)
        c = _encode_cursor(dt, 42)
        out = _decode_cursor(c)
        assert out == (dt, 42)

    def test_naive_datetime_roundtrips(self) -> None:
        # Naive datetimes still roundtrip; the encoder does not strip tz.
        dt = datetime(2026, 1, 2, 3, 4, 5)
        c = _encode_cursor(dt, 1)
        out = _decode_cursor(c)
        assert out == (dt, 1)

    def test_empty_decodes_to_none(self) -> None:
        assert _decode_cursor(None) is None
        assert _decode_cursor("") is None

    @pytest.mark.parametrize(
        "bogus",
        ["not-base64!!", "===", "Zm9vYmFy", "0|", "|0"],
    )
    def test_garbage_raises_400(self, bogus: str) -> None:
        with pytest.raises(HTTPException) as exc:
            _decode_cursor(bogus)
        assert exc.value.status_code == 400

    def test_cursor_is_opaque(self) -> None:
        # No raw timestamp in the encoded payload.
        dt = datetime(2026, 6, 12, tzinfo=UTC)
        c = _encode_cursor(dt, 999)
        assert "2026" not in c
        assert "999" not in c
