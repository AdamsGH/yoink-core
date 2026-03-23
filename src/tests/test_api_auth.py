"""Tests for /auth/token endpoint and JWT verification."""
from __future__ import annotations

import hashlib
import hmac
import json
import time

import pytest

from tests.conftest import API_SECRET, BOT_TOKEN, OWNER_ID, make_jwt


def _build_init_data(
    user_id: int = 100099,
    username: str = "testuser",
    first_name: str = "Test",
    bot_token: str = BOT_TOKEN,
    auth_date: int | None = None,
) -> str:
    """Build a valid Telegram initData string with correct HMAC."""
    if auth_date is None:
        auth_date = int(time.time())
    user_json = json.dumps(
        {"id": user_id, "username": username, "first_name": first_name},
        separators=(",", ":"),
    )
    params = {"auth_date": str(auth_date), "user": user_json}
    data_check = "\n".join(f"{k}={v}" for k, v in sorted(params.items()))
    secret = hmac.new(b"WebAppData", bot_token.encode(), hashlib.sha256).digest()
    hash_val = hmac.new(secret, data_check.encode(), hashlib.sha256).hexdigest()
    parts = [f"{k}={v}" for k, v in sorted(params.items())]
    parts.append(f"hash={hash_val}")
    return "&".join(parts)


@pytest.mark.asyncio
async def test_auth_token_valid(api_client):
    init_data = _build_init_data(user_id=200001, username="alice", first_name="Alice")
    resp = await api_client.post("/api/v1/auth/token", json={"init_data": init_data})
    assert resp.status_code == 200
    body = resp.json()
    assert "access_token" in body
    assert body["token_type"] == "bearer"


@pytest.mark.asyncio
async def test_auth_token_creates_user(api_client, session_factory):
    user_id = 200002
    init_data = _build_init_data(user_id=user_id, username="bob")
    resp = await api_client.post("/api/v1/auth/token", json={"init_data": init_data})
    assert resp.status_code == 200

    from yoink.core.db.models import User
    async with session_factory() as sess:
        user = await sess.get(User, user_id)
        assert user is not None
        assert user.username == "bob"


@pytest.mark.asyncio
async def test_auth_token_invalid_hmac(api_client):
    init_data = _build_init_data() + "&tampered=yes"
    resp = await api_client.post("/api/v1/auth/token", json={"init_data": init_data})
    assert resp.status_code == 401


@pytest.mark.asyncio
async def test_auth_token_missing_hash(api_client):
    resp = await api_client.post(
        "/api/v1/auth/token",
        json={"init_data": "auth_date=12345&user=%7B%7D"},
    )
    assert resp.status_code == 401


@pytest.mark.asyncio
async def test_auth_token_expired(api_client):
    old_date = int(time.time()) - 90000
    init_data = _build_init_data(auth_date=old_date)
    resp = await api_client.post("/api/v1/auth/token", json={"init_data": init_data})
    assert resp.status_code == 401


@pytest.mark.asyncio
async def test_auth_token_owner_gets_owner_role(api_client, session_factory):
    init_data = _build_init_data(user_id=OWNER_ID, username="owner")
    resp = await api_client.post("/api/v1/auth/token", json={"init_data": init_data})
    assert resp.status_code == 200

    from yoink.core.db.models import User
    async with session_factory() as sess:
        user = await sess.get(User, OWNER_ID)
        assert user is not None
        assert user.role.value == "owner"


@pytest.mark.asyncio
async def test_blocked_user_cannot_access_api(api_client, banned_user):
    token = make_jwt(banned_user.id)
    resp = await api_client.get(
        "/api/v1/users/me",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 403
    assert "blocked" in resp.json()["detail"].lower()


@pytest.mark.asyncio
async def test_valid_jwt_returns_user(api_client, regular_user):
    token = make_jwt(regular_user.id)
    resp = await api_client.get(
        "/api/v1/users/me",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["id"] == regular_user.id
    assert body["username"] == "regular"
