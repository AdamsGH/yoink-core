"""Tests for /health and /metrics endpoints."""
from __future__ import annotations

import pytest

from tests.conftest import make_jwt


@pytest.mark.asyncio
async def test_health_returns_checks(api_client):
    resp = await api_client.get("/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] in ("ok", "degraded")
    assert "checks" in body
    assert "db" in body["checks"]
    assert "disk" in body["checks"]


@pytest.mark.asyncio
async def test_health_db_latency(api_client):
    resp = await api_client.get("/health")
    db = resp.json()["checks"]["db"]
    assert db["status"] == "ok"
    assert "latency_ms" in db


@pytest.mark.asyncio
async def test_metrics_requires_auth(api_client):
    resp = await api_client.get("/metrics")
    assert resp.status_code in (401, 403)


@pytest.mark.asyncio
async def test_metrics_requires_admin(api_client, regular_user):
    token = make_jwt(regular_user.id)
    resp = await api_client.get(
        "/metrics",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 403


@pytest.mark.asyncio
async def test_metrics_admin_access(api_client, admin):
    token = make_jwt(admin.id)
    resp = await api_client.get(
        "/metrics",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 200
    body = resp.json()
    assert "uptime_seconds" in body
    assert "counters" in body
    assert "histograms" in body
