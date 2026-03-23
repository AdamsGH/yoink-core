"""Health check and metrics endpoints."""
from __future__ import annotations

import logging
import shutil
import time

from fastapi import APIRouter, Depends, Request
from sqlalchemy import text

from yoink.core.api.deps import get_current_user
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import User, UserRole
from yoink.core.metrics import metrics

logger = logging.getLogger(__name__)

router = APIRouter(tags=["health"])


@router.get("/health", include_in_schema=False)
async def health(request: Request) -> dict:
    checks: dict[str, dict] = {}
    overall = "ok"

    checks["db"] = await _check_db(request)
    checks["bot"] = _check_bot(request)
    checks["disk"] = _check_disk()

    for name, result in checks.items():
        if result["status"] != "ok":
            overall = "degraded"

    return {"status": overall, "checks": checks}


@router.get("/metrics", include_in_schema=False)
async def get_metrics(
    current_user: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> dict:
    return metrics.snapshot()


async def _check_db(request: Request) -> dict:
    sf = getattr(request.app.state, "session_factory", None)
    if sf is None:
        return {"status": "error", "message": "no session factory"}
    try:
        t0 = time.monotonic()
        async with sf() as session:
            await session.execute(text("SELECT 1"))
        latency_ms = round((time.monotonic() - t0) * 1000, 1)
        return {"status": "ok", "latency_ms": latency_ms}
    except Exception as exc:
        logger.warning("Health check: DB probe failed: %s", exc)
        return {"status": "error", "message": str(exc)}


def _check_bot(request: Request) -> dict:
    bot = getattr(request.app.state, "bot", None)
    if bot is None:
        return {"status": "ok", "message": "standalone API mode"}
    try:
        # bot.bot is the cached User object from get_me()
        me = getattr(bot, "bot", None)
        if me and getattr(me, "username", None):
            return {"status": "ok", "username": f"@{me.username}"}
        return {"status": "ok", "message": "bot present, no cached info"}
    except Exception as exc:
        return {"status": "error", "message": str(exc)}


def _check_disk() -> dict:
    try:
        usage = shutil.disk_usage("/app/data")
        free_gb = round(usage.free / (1024 ** 3), 2)
        total_gb = round(usage.total / (1024 ** 3), 2)
        used_pct = round((usage.used / usage.total) * 100, 1)
        status = "ok" if used_pct < 90 else "warning"
        return {
            "status": status,
            "free_gb": free_gb,
            "total_gb": total_gb,
            "used_pct": used_pct,
        }
    except Exception as exc:
        return {"status": "error", "message": str(exc)}
