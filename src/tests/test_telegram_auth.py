"""Unit tests for Telegram initData HMAC verification - no DB needed."""
from __future__ import annotations

import hashlib
import hmac
import json
import time

import pytest

from yoink.core.auth.telegram import verify_init_data

BOT_TOKEN = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"


def _sign(params: dict, token: str = BOT_TOKEN) -> str:
    """Build a signed initData query string from params dict."""
    data_check = "\n".join(f"{k}={v}" for k, v in sorted(params.items()))
    secret = hmac.new(b"WebAppData", token.encode(), hashlib.sha256).digest()
    hash_val = hmac.new(secret, data_check.encode(), hashlib.sha256).hexdigest()
    parts = [f"{k}={v}" for k, v in sorted(params.items())]
    parts.append(f"hash={hash_val}")
    return "&".join(parts)


class TestVerifyInitData:

    def test_valid_data(self):
        params = {
            "auth_date": str(int(time.time())),
            "user": json.dumps({"id": 12345, "username": "test"}),
        }
        init_data = _sign(params)
        result = verify_init_data(init_data, BOT_TOKEN)
        assert result["auth_date"] == params["auth_date"]

    def test_missing_hash(self):
        with pytest.raises(ValueError, match="missing hash"):
            verify_init_data("auth_date=123&user=%7B%7D", BOT_TOKEN)

    def test_wrong_hash(self):
        params = {
            "auth_date": str(int(time.time())),
            "user": json.dumps({"id": 1}),
        }
        init_data = _sign(params) + "abc"
        with pytest.raises(ValueError, match="mismatch"):
            verify_init_data(init_data, BOT_TOKEN)

    def test_tampered_data(self):
        params = {
            "auth_date": str(int(time.time())),
            "user": json.dumps({"id": 1}),
        }
        init_data = _sign(params)
        # Tamper the auth_date value while keeping the old hash
        init_data = init_data.replace(params["auth_date"], "9999999999", 1)
        with pytest.raises(ValueError, match="mismatch"):
            verify_init_data(init_data, BOT_TOKEN)

    def test_expired(self):
        params = {
            "auth_date": str(int(time.time()) - 90000),
            "user": json.dumps({"id": 1}),
        }
        init_data = _sign(params)
        with pytest.raises(ValueError, match="expired"):
            verify_init_data(init_data, BOT_TOKEN)

    def test_custom_max_age_raises(self):
        params = {
            "auth_date": str(int(time.time()) - 10),
            "user": json.dumps({"id": 1}),
        }
        init_data = _sign(params)
        with pytest.raises(ValueError, match="expired"):
            verify_init_data(init_data, BOT_TOKEN, max_age=5)

    def test_wrong_bot_token(self):
        params = {
            "auth_date": str(int(time.time())),
            "user": json.dumps({"id": 1}),
        }
        init_data = _sign(params, token="111:wrong-token")
        with pytest.raises(ValueError, match="mismatch"):
            verify_init_data(init_data, BOT_TOKEN)

    def test_returns_all_params(self):
        params = {
            "auth_date": str(int(time.time())),
            "user": json.dumps({"id": 42, "username": "alice"}),
            "query_id": "AAHdF6IQAAAAAN0",
        }
        init_data = _sign(params)
        result = verify_init_data(init_data, BOT_TOKEN)
        assert "query_id" in result
        assert result["query_id"] == "AAHdF6IQAAAAAN0"
        assert "hash" not in result
