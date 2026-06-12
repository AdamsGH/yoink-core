"""Inbox FastAPI router (mounted at /api/v1/inbox by yoink-core).

Endpoint implementations land alongside services in subsequent commits. This
file exposes the router shell so InboxPlugin.get_routes() returns something
mountable from day one.
"""
from __future__ import annotations

from fastapi import APIRouter

router = APIRouter(prefix="/inbox", tags=["inbox"])


@router.get("/health")
async def health() -> dict[str, str]:
    """Liveness probe used by `just verify` smoke tests."""
    return {"status": "ok", "plugin": "inbox"}
