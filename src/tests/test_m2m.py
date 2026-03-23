"""Tests for M2M (internal) API endpoints and API key management."""
from __future__ import annotations

import pytest

from tests.conftest import make_jwt
from yoink.core.auth.apikey import generate_api_key
from yoink.core.db.models import ApiKey


@pytest.fixture
async def api_key_row(session_factory) -> tuple[str, ApiKey]:
    """Create an API key directly in DB, return (raw_key, row)."""
    raw, key_hash, prefix = generate_api_key()
    async with session_factory() as sess:
        row = ApiKey(
            name="test-key",
            key_hash=key_hash,
            prefix=prefix,
            scopes=["*"],
            created_by=100001,
        )
        sess.add(row)
        await sess.commit()
        await sess.refresh(row)
    yield raw, row
    async with session_factory() as sess:
        r = await sess.get(ApiKey, row.id)
        if r:
            await sess.delete(r)
            await sess.commit()


@pytest.fixture
async def scoped_key(session_factory) -> tuple[str, ApiKey]:
    """API key with only users:read scope."""
    raw, key_hash, prefix = generate_api_key()
    async with session_factory() as sess:
        row = ApiKey(
            name="scoped-key",
            key_hash=key_hash,
            prefix=prefix,
            scopes=["users:read"],
            created_by=100001,
        )
        sess.add(row)
        await sess.commit()
        await sess.refresh(row)
    yield raw, row
    async with session_factory() as sess:
        r = await sess.get(ApiKey, row.id)
        if r:
            await sess.delete(r)
            await sess.commit()


class TestApiKeyManagement:
    async def test_create_key_requires_admin(self, api_client, regular_user):
        token = make_jwt(regular_user.id, role="user")
        resp = await api_client.post(
            "/api/v1/api-keys",
            json={"name": "my-key"},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 403

    async def test_create_key_as_owner(self, api_client, owner):
        token = make_jwt(owner.id, role="owner")
        resp = await api_client.post(
            "/api/v1/api-keys",
            json={"name": "ci-key", "scopes": ["users:read", "health:read"]},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 201
        data = resp.json()
        assert data["name"] == "ci-key"
        assert data["key"].startswith("yoink_")
        assert data["scopes"] == ["users:read", "health:read"]

    async def test_create_key_invalid_scope(self, api_client, owner):
        token = make_jwt(owner.id, role="owner")
        resp = await api_client.post(
            "/api/v1/api-keys",
            json={"name": "bad", "scopes": ["nonexistent:scope"]},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 400

    async def test_list_keys(self, api_client, owner, api_key_row):
        token = make_jwt(owner.id, role="owner")
        resp = await api_client.get(
            "/api/v1/api-keys",
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 200
        keys = resp.json()
        assert any(k["name"] == "test-key" for k in keys)
        assert not any("key" in k for k in keys)

    async def test_revoke_key(self, api_client, owner, api_key_row):
        raw, row = api_key_row
        token = make_jwt(owner.id, role="owner")
        resp = await api_client.delete(
            f"/api/v1/api-keys/{row.id}",
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 204

    async def test_list_scopes(self, api_client, owner):
        token = make_jwt(owner.id, role="owner")
        resp = await api_client.get(
            "/api/v1/api-keys/scopes",
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 200
        assert "users:read" in resp.json()["scopes"]


class TestM2MEndpoints:
    async def test_missing_key_401(self, api_client):
        resp = await api_client.get("/api/internal/v1/users")
        assert resp.status_code == 401

    async def test_invalid_key_401(self, api_client):
        resp = await api_client.get(
            "/api/internal/v1/users",
            headers={"X-Api-Key": "bad-key"},
        )
        assert resp.status_code == 401

    async def test_revoked_key_401(self, api_client, session_factory):
        raw, key_hash, prefix = generate_api_key()
        async with session_factory() as sess:
            row = ApiKey(
                name="revoked", key_hash=key_hash, prefix=prefix,
                scopes=["*"], created_by=100001, revoked=True,
            )
            sess.add(row)
            await sess.commit()
            key_id = row.id
        resp = await api_client.get(
            "/api/internal/v1/users",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 401
        async with session_factory() as sess:
            r = await sess.get(ApiKey, key_id)
            if r:
                await sess.delete(r)
                await sess.commit()

    async def test_list_users(self, api_client, api_key_row, owner):
        raw, _ = api_key_row
        resp = await api_client.get(
            "/api/internal/v1/users",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 200
        users = resp.json()
        assert any(u["id"] == owner.id for u in users)

    async def test_get_user(self, api_client, api_key_row, owner):
        raw, _ = api_key_row
        resp = await api_client.get(
            f"/api/internal/v1/users/{owner.id}",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 200
        assert resp.json()["id"] == owner.id

    async def test_get_user_not_found(self, api_client, api_key_row):
        raw, _ = api_key_row
        resp = await api_client.get(
            "/api/internal/v1/users/999999999",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 404

    async def test_list_groups(self, api_client, api_key_row, test_group):
        raw, _ = api_key_row
        resp = await api_client.get(
            "/api/internal/v1/groups",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 200
        groups = resp.json()
        assert any(g["id"] == test_group.id for g in groups)

    async def test_create_event(self, api_client, api_key_row):
        raw, _ = api_key_row
        resp = await api_client.post(
            "/api/internal/v1/events",
            json={
                "plugin": "ci",
                "event_type": "deploy",
                "payload": {"version": "1.2.3"},
            },
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 201
        assert resp.json()["plugin"] == "ci"
        assert resp.json()["event_type"] == "deploy"

    async def test_list_events(self, api_client, api_key_row):
        raw, _ = api_key_row
        resp = await api_client.get(
            "/api/internal/v1/events",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 200

    async def test_scope_denied(self, api_client, scoped_key):
        raw, _ = scoped_key
        resp = await api_client.get(
            "/api/internal/v1/groups",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 403
        assert "groups:read" in resp.json()["detail"]

    async def test_scope_allowed(self, api_client, scoped_key, owner):
        raw, _ = scoped_key
        resp = await api_client.get(
            "/api/internal/v1/users",
            headers={"X-Api-Key": raw},
        )
        assert resp.status_code == 200
