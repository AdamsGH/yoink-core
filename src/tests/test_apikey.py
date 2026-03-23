"""Tests for API key generation, hashing, scope checks."""
from __future__ import annotations

import pytest

from yoink.core.auth.apikey import (
    ALL_SCOPES,
    generate_api_key,
    has_scope,
    hash_key,
    is_expired,
)


class TestGenerateApiKey:
    def test_returns_three_parts(self):
        raw, key_hash, prefix = generate_api_key()
        assert raw.startswith("yoink_")
        assert len(key_hash) == 64
        assert prefix == raw[:8]

    def test_hash_matches(self):
        raw, key_hash, _ = generate_api_key()
        assert hash_key(raw) == key_hash

    def test_unique_keys(self):
        keys = {generate_api_key()[0] for _ in range(20)}
        assert len(keys) == 20


class TestHasScope:
    def test_exact_match(self):
        assert has_scope(["users:read"], "users:read")

    def test_no_match(self):
        assert not has_scope(["users:read"], "users:write")

    def test_wildcard_all(self):
        assert has_scope(["*"], "anything:here")

    def test_category_wildcard(self):
        assert has_scope(["users:*"], "users:write")
        assert has_scope(["users:*"], "users:read")
        assert not has_scope(["users:*"], "groups:read")

    def test_empty_scopes(self):
        assert not has_scope([], "users:read")


class TestIsExpired:
    def test_none_never_expires(self):
        assert not is_expired(None)

    def test_future_not_expired(self):
        from datetime import datetime, timedelta, timezone
        future = datetime.now(timezone.utc) + timedelta(hours=1)
        assert not is_expired(future)

    def test_past_expired(self):
        from datetime import datetime, timedelta, timezone
        past = datetime.now(timezone.utc) - timedelta(hours=1)
        assert is_expired(past)


class TestAllScopes:
    def test_scopes_not_empty(self):
        assert len(ALL_SCOPES) > 0

    def test_scopes_format(self):
        for scope in ALL_SCOPES:
            assert ":" in scope, f"Scope {scope} missing colon separator"
